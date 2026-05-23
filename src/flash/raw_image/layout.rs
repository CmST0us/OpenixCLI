//! Locate boot0 / u-boot inside a full-disk raw.img at standard sunxi offsets.

#![allow(dead_code)]

use crate::config::boot_header::{Boot0Header, UBootBaseHeader, BOOT0_MAGIC, UBOOT_MAGIC};
use crate::utils::FlashError;

/// boot0 sits at sector 16 (8 KiB) in the SD/eMMC sunxi layout.
pub const BOOT0_OFFSET: usize = 16 * 512;
/// u-boot / toc1 standard sunxi start sector. NOTE: verify against target SoC;
/// overridable via the hidden --uboot-offset flag (see flash_raw command).
pub const UBOOT_START_SECTOR: usize = 32800;

/// Extracted bootstrap blobs borrowed from the raw image.
pub struct RawBootstrap<'a> {
    pub boot0: &'a [u8],
    pub uboot: &'a [u8],
}

/// Slice boot0 and u-boot out of `img`. `uboot_sector` lets callers override
/// the default UBOOT_START_SECTOR.
pub fn extract_bootstrap(img: &[u8], uboot_sector: usize) -> Result<RawBootstrap<'_>, FlashError> {
    // boot0
    if img.len() < BOOT0_OFFSET + std::mem::size_of::<Boot0Header>() {
        return Err(FlashError::InvalidFirmwareFormat("image too small for boot0".into()));
    }
    let boot0_hdr = Boot0Header::parse(&img[BOOT0_OFFSET..])
        .map_err(|e| FlashError::InvalidFirmwareFormat(e.to_string()))?;
    if !boot0_hdr.magic_str().starts_with(BOOT0_MAGIC) {
        return Err(FlashError::InvalidFirmwareFormat(format!(
            "no '{}' magic at boot0 offset 0x{:x}", BOOT0_MAGIC, BOOT0_OFFSET
        )));
    }
    let boot0_len = { let l = boot0_hdr.length; l as usize };
    let boot0_end = BOOT0_OFFSET + boot0_len;
    if boot0_len == 0 || boot0_end > img.len() {
        return Err(FlashError::InvalidFirmwareFormat(format!("bad boot0 length {}", boot0_len)));
    }
    let boot0 = &img[BOOT0_OFFSET..boot0_end];

    // u-boot
    let uboot_off = uboot_sector * 512;
    if img.len() < uboot_off + std::mem::size_of::<UBootBaseHeader>() {
        return Err(FlashError::InvalidFirmwareFormat("image too small for u-boot".into()));
    }
    let uboot_hdr = UBootBaseHeader::parse(&img[uboot_off..])
        .map_err(|e| FlashError::InvalidFirmwareFormat(e.to_string()))?;
    if !uboot_hdr.magic_str().starts_with(UBOOT_MAGIC) {
        return Err(FlashError::InvalidFirmwareFormat(format!(
            "no '{}' magic at u-boot sector {} (offset 0x{:x}); try --uboot-offset",
            UBOOT_MAGIC, uboot_sector, uboot_off
        )));
    }
    let uboot_len = { let l = uboot_hdr.length; l as usize };
    let uboot_end = uboot_off + uboot_len;
    if uboot_len == 0 || uboot_end > img.len() {
        return Err(FlashError::InvalidFirmwareFormat(format!("bad u-boot length {}", uboot_len)));
    }
    let uboot = &img[uboot_off..uboot_end];

    Ok(RawBootstrap { boot0, uboot })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(buf: &mut [u8], off: usize, bytes: &[u8]) {
        buf[off..off + bytes.len()].copy_from_slice(bytes);
    }

    #[test]
    fn extracts_boot0_and_uboot() {
        let uboot_sector = 64usize; // small offset for the test image
        let mut img = vec![0u8; uboot_sector * 512 + 4096];
        // boot0 header: jump(4) + magic(8) + check_sum(4) + length(4)@offset16
        put(&mut img, BOOT0_OFFSET + 4, BOOT0_MAGIC.as_bytes());
        put(&mut img, BOOT0_OFFSET + 16, &1024u32.to_le_bytes());
        // uboot base header: jump(4) + magic(8) + check_sum(4) + align(4) + length(4)@offset20
        let uoff = uboot_sector * 512;
        put(&mut img, uoff + 4, UBOOT_MAGIC.as_bytes());
        put(&mut img, uoff + 20, &2048u32.to_le_bytes());

        let bs = extract_bootstrap(&img, uboot_sector).unwrap();
        assert_eq!(bs.boot0.len(), 1024);
        assert_eq!(bs.uboot.len(), 2048);
    }

    #[test]
    fn errors_when_uboot_magic_missing() {
        let uboot_sector = 64usize;
        let mut img = vec![0u8; uboot_sector * 512 + 4096];
        put(&mut img, BOOT0_OFFSET + 4, BOOT0_MAGIC.as_bytes());
        put(&mut img, BOOT0_OFFSET + 16, &1024u32.to_le_bytes());
        // no uboot magic written
        assert!(extract_bootstrap(&img, uboot_sector).is_err());
    }
}
