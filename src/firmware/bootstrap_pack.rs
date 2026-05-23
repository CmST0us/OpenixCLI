//! Build a minimal IMAGEWTY "bootstrap" firmware from a full LiveSuit image.
//!
//! For newer SoCs (e.g. A733) the FEL->FES bootstrap must come from a real
//! LiveSuit firmware (fes1 + u-boot), but those firmwares are huge (GBs). This
//! repacks only the entries needed for bootstrap into a small IMAGEWTY file that
//! `flash-raw --bootstrap` can consume unchanged.

#![allow(dead_code)]

use crate::firmware::image_data::get_image_data_entry;
use crate::firmware::packer::PackerError;
use crate::firmware::types::{IMAGEWTY_FILEHDR_LEN, IMAGEWTY_MAGIC};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

/// Entry names (from the image_data table) kept for FEL bootstrap.
/// `fes` and `uboot` are required; the rest are optional.
const BOOTSTRAP_ENTRIES: &[&str] = &["fes", "uboot", "sys_config_bin", "board_config", "dtb"];

const HDR_LEN: usize = IMAGEWTY_FILEHDR_LEN; // 1024

fn read_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

fn write_u32(b: &mut [u8], off: usize, v: u32) {
    b[off..off + 4].copy_from_slice(&v.to_le_bytes());
}

fn tag_str(b: &[u8]) -> String {
    String::from_utf8_lossy(b)
        .trim_end_matches(['\0', ' '])
        .to_string()
}

/// Byte offsets that differ between IMAGEWTY header versions.
struct VersionLayout {
    /// num_files field offset within the image header
    num_files: usize,
    /// original_length field offset within a file header
    orig_len: usize,
    /// data offset field within a file header
    data_off: usize,
}

fn version_layout(header_version: u32) -> VersionLayout {
    if header_version == 0x0300 {
        // v3: image header num_files@60; file header orig_len@300, offset@308
        VersionLayout { num_files: 60, orig_len: 300, data_off: 308 }
    } else {
        // v1: image header num_files@56; file header orig_len@40, offset@44
        VersionLayout { num_files: 56, orig_len: 40, data_off: 44 }
    }
}

/// Summary of a built bootstrap firmware.
pub struct BootstrapSummary {
    /// (entry name, byte size) for each kept entry
    pub entries: Vec<(String, u32)>,
    /// total size of the written file
    pub output_size: u64,
}

/// Build a minimal bootstrap firmware at `output` from full firmware `input`.
///
/// Reads the IMAGEWTY container, keeps only the bootstrap entries, and writes a
/// new IMAGEWTY container of the same header version with recomputed offsets.
pub fn build_bootstrap(input: &Path, output: &Path) -> Result<BootstrapSummary, PackerError> {
    let mut f = File::open(input)?;

    // Image header (first 1024 bytes).
    let mut img_hdr = vec![0u8; HDR_LEN];
    f.read_exact(&mut img_hdr)?;
    if &img_hdr[0..8] != IMAGEWTY_MAGIC.as_bytes() {
        return Err(PackerError::InvalidMagic(tag_str(&img_hdr[0..8])));
    }
    let header_version = read_u32(&img_hdr, 8);
    let lay = version_layout(header_version);
    let num_files = read_u32(&img_hdr, lay.num_files) as usize;

    // Wanted (maintype, subtype, name) set, resolved from the image_data table.
    let wanted: Vec<(String, String, &str)> = BOOTSTRAP_ENTRIES
        .iter()
        .filter_map(|name| {
            get_image_data_entry(name).map(|e| (e.maintype.to_string(), e.subtype.to_string(), *name))
        })
        .collect();

    // Scan file headers; collect kept ones as (raw 1024-byte header, data, name).
    let mut kept: Vec<(Vec<u8>, Vec<u8>, String)> = Vec::new();
    for i in 0..num_files {
        f.seek(SeekFrom::Start((HDR_LEN + i * HDR_LEN) as u64))?;
        let mut fh = vec![0u8; HDR_LEN];
        f.read_exact(&mut fh)?;
        let mt = tag_str(&fh[8..16]);
        let st = tag_str(&fh[16..32]);
        if let Some((_, _, name)) = wanted.iter().find(|(m, s, _)| *m == mt && *s == st) {
            let orig_len = read_u32(&fh, lay.orig_len);
            let data_off = read_u32(&fh, lay.data_off);
            f.seek(SeekFrom::Start(data_off as u64))?;
            let mut data = vec![0u8; orig_len as usize];
            f.read_exact(&mut data)?;
            kept.push((fh, data, name.to_string()));
        }
    }

    // fes + uboot are mandatory.
    let have = |n: &str| kept.iter().any(|(_, _, k)| k == n);
    if !have("fes") {
        return Err(PackerError::FileNotFound("fes (fes1.fex)".into()));
    }
    if !have("uboot") {
        return Err(PackerError::FileNotFound("uboot (u-boot.fex)".into()));
    }

    // New layout: image header + N file headers, then data placed contiguously.
    let n = kept.len();
    let data_start = HDR_LEN * (1 + n);

    let mut out: Vec<u8> = Vec::with_capacity(data_start);
    let mut new_img_hdr = img_hdr.clone();
    write_u32(&mut new_img_hdr, lay.num_files, n as u32);
    out.extend_from_slice(&new_img_hdr);

    let mut data_blob: Vec<u8> = Vec::new();
    let mut cur = data_start;
    let mut entries = Vec::new();
    for (mut fh, data, name) in kept {
        write_u32(&mut fh, lay.data_off, cur as u32);
        out.extend_from_slice(&fh);
        entries.push((name, data.len() as u32));
        cur += data.len();
        data_blob.extend_from_slice(&data);
    }
    out.extend_from_slice(&data_blob);

    // Patch image_size (byte 24) to the total file length.
    let total = out.len() as u32;
    write_u32(&mut out, 24, total);

    let mut of = File::create(output)?;
    of.write_all(&out)?;

    Ok(BootstrapSummary { entries, output_size: out.len() as u64 })
}
