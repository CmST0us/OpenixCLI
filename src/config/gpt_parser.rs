//! GPT (GUID Partition Table) parser
//!
//! Parses a GPT primary header (LBA1) plus its partition entry array.
//! Used by flash-part to locate a partition on the device by name.

#![allow(dead_code)]

use crate::utils::FlashError;

pub const GPT_SIGNATURE: &[u8; 8] = b"EFI PART";
pub const GPT_HEADER_SIZE: usize = 92;
pub const SECTOR_SIZE: u64 = 512;

/// CRC-32 (IEEE 802.3, reflected, poly 0xEDB88320).
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// A single parsed GPT partition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GptPartition {
    pub name: String,
    pub first_lba: u64,
    pub last_lba: u64,
}

impl GptPartition {
    /// Inclusive sector count [first_lba, last_lba].
    pub fn sector_count(&self) -> u64 {
        self.last_lba.saturating_sub(self.first_lba).saturating_add(1)
    }

    /// Capacity in bytes.
    pub fn size_bytes(&self) -> u64 {
        self.sector_count() * SECTOR_SIZE
    }
}

/// Parsed GPT: header fields needed to read entries + the partitions.
#[derive(Debug, Clone)]
pub struct Gpt {
    pub partition_entries_lba: u64,
    pub num_entries: u32,
    pub entry_size: u32,
    pub partitions: Vec<GptPartition>,
}

impl Gpt {
    /// Find a partition by exact name (case-sensitive).
    pub fn find(&self, name: &str) -> Option<&GptPartition> {
        self.partitions.iter().find(|p| p.name == name)
    }

    /// All partition names, for error messages.
    pub fn names(&self) -> Vec<String> {
        self.partitions.iter().map(|p| p.name.clone()).collect()
    }
}

fn read_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

fn read_u64(b: &[u8], off: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[off..off + 8]);
    u64::from_le_bytes(a)
}

/// Parse the GPT header (LBA1, 512 bytes) without entries.
/// Validates signature and header CRC-32.
pub fn parse_header(lba1: &[u8]) -> Result<Gpt, FlashError> {
    if lba1.len() < GPT_HEADER_SIZE {
        return Err(FlashError::GptInvalid("header buffer too small".into()));
    }
    if &lba1[0..8] != GPT_SIGNATURE {
        return Err(FlashError::GptInvalid("bad signature (expected 'EFI PART')".into()));
    }
    let header_size = read_u32(lba1, 12) as usize;
    if header_size < GPT_HEADER_SIZE || header_size > lba1.len() {
        return Err(FlashError::GptInvalid(format!("bad header_size {}", header_size)));
    }
    let stored_crc = read_u32(lba1, 16);
    let mut hdr = lba1[..header_size].to_vec();
    hdr[16..20].fill(0); // CRC field zeroed for computation
    let calc_crc = crc32(&hdr);
    if calc_crc != stored_crc {
        return Err(FlashError::GptInvalid(format!(
            "header CRC mismatch: stored=0x{:08x} calc=0x{:08x}",
            stored_crc, calc_crc
        )));
    }
    Ok(Gpt {
        partition_entries_lba: read_u64(lba1, 72),
        num_entries: read_u32(lba1, 80),
        entry_size: read_u32(lba1, 84),
        partitions: Vec::new(),
    })
}

/// Parse the partition entry array into `gpt.partitions`.
/// `entries` must contain at least `num_entries * entry_size` bytes.
/// Validates the entries CRC-32 against the value stored in `lba1`.
pub fn parse_entries(gpt: &mut Gpt, lba1: &[u8], entries: &[u8]) -> Result<(), FlashError> {
    if lba1.len() < GPT_HEADER_SIZE {
        return Err(FlashError::GptInvalid("lba1 buffer too small".into()));
    }
    let entry_size = gpt.entry_size as usize;
    let num = gpt.num_entries as usize;
    if entry_size < 128 {
        return Err(FlashError::GptInvalid(format!("bad entry_size {}", entry_size)));
    }
    let needed = entry_size * num;
    if entries.len() < needed {
        return Err(FlashError::GptInvalid("entry buffer too small".into()));
    }
    let stored_entries_crc = read_u32(lba1, 88);
    let calc = crc32(&entries[..needed]);
    if calc != stored_entries_crc {
        return Err(FlashError::GptInvalid(format!(
            "entries CRC mismatch: stored=0x{:08x} calc=0x{:08x}",
            stored_entries_crc, calc
        )));
    }
    for i in 0..num {
        let base = i * entry_size;
        let e = &entries[base..base + entry_size];
        // Entry is unused when type GUID is all-zero.
        if e[0..16].iter().all(|&x| x == 0) {
            continue;
        }
        let first_lba = read_u64(e, 32);
        let last_lba = read_u64(e, 40);
        // Name: 72 bytes UTF-16LE (36 code units), trailing zeros trimmed.
        let units: Vec<u16> = (0..36).map(|j| u16::from_le_bytes([e[56 + j * 2], e[57 + j * 2]])).collect();
        let name: String = char::decode_utf16(units.into_iter().take_while(|&u| u != 0))
            .map(|r| r.unwrap_or('\u{FFFD}'))
            .collect();
        gpt.partitions.push(GptPartition { name, first_lba, last_lba });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_known_vector() {
        // Standard CRC-32 of "123456789" is 0xCBF43926.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    // Build a minimal one-entry GPT (header in lba1, 1 entry of 128 bytes) with valid CRCs.
    fn build_gpt(name: &str, first: u64, last: u64) -> (Vec<u8>, Vec<u8>) {
        let entry_size = 128usize;
        let num = 1u32;
        let mut entries = vec![0u8; entry_size];
        entries[0] = 1; // non-zero type GUID
        entries[32..40].copy_from_slice(&first.to_le_bytes());
        entries[40..48].copy_from_slice(&last.to_le_bytes());
        for (j, u) in name.encode_utf16().enumerate() {
            entries[56 + j * 2..58 + j * 2].copy_from_slice(&u.to_le_bytes());
        }
        let entries_crc = crc32(&entries);

        let mut lba1 = vec![0u8; 512];
        lba1[0..8].copy_from_slice(GPT_SIGNATURE);
        lba1[12..16].copy_from_slice(&(GPT_HEADER_SIZE as u32).to_le_bytes());
        lba1[72..80].copy_from_slice(&2u64.to_le_bytes()); // entries at LBA2
        lba1[80..84].copy_from_slice(&num.to_le_bytes());
        lba1[84..88].copy_from_slice(&(entry_size as u32).to_le_bytes());
        lba1[88..92].copy_from_slice(&entries_crc.to_le_bytes());
        let hdr_crc = crc32(&lba1[..GPT_HEADER_SIZE]);
        lba1[16..20].copy_from_slice(&hdr_crc.to_le_bytes());
        (lba1, entries)
    }

    #[test]
    fn parses_partition_by_name() {
        let (lba1, entries) = build_gpt("boot", 40, 1063);
        let mut gpt = parse_header(&lba1).unwrap();
        parse_entries(&mut gpt, &lba1, &entries).unwrap();
        let p = gpt.find("boot").expect("boot present");
        assert_eq!(p.first_lba, 40);
        assert_eq!(p.last_lba, 1063);
        assert_eq!(p.sector_count(), 1024);
        assert_eq!(p.size_bytes(), 1024 * 512);
        assert!(gpt.find("missing").is_none());
        assert_eq!(gpt.names(), vec!["boot".to_string()]);
    }

    #[test]
    fn rejects_bad_signature() {
        let mut lba1 = vec![0u8; 512];
        lba1[0..8].copy_from_slice(b"NOTEFIPT");
        assert!(parse_header(&lba1).is_err());
    }

    #[test]
    fn rejects_bad_header_crc() {
        let (mut lba1, _e) = build_gpt("boot", 40, 1063);
        lba1[16] ^= 0xFF; // corrupt stored CRC
        assert!(parse_header(&lba1).is_err());
    }

    #[test]
    fn rejects_bad_entries_crc() {
        let (lba1, mut entries) = build_gpt("boot", 40, 1063);
        let mut gpt = parse_header(&lba1).unwrap();
        entries[0] ^= 0xFF; // corrupt entry data after CRC is stored in lba1
        assert!(parse_entries(&mut gpt, &lba1, &entries).is_err());
    }
}
