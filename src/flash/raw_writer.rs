//! Generic raw sector writer
//!
//! Writes an in-memory byte slice to device storage starting at a given
//! sector, in fixed-size chunks, with progress reporting and optional
//! checksum verification. Shared by flash-raw (full image) and flash-part
//! (raw partition images).

#![allow(dead_code)]

use crate::config::mbr_parser::EFEX_CRC32_VALID_FLAG;
use crate::flash::fes_handler::IncrementalChecksum;
use crate::utils::{FlashError, FlashResult, Logger};
use indicatif::{ProgressBar, ProgressStyle};
use libefex::FesDataType;
use std::time::Duration;

/// Bytes per write chunk fed to libefex (libefex further splits into 64 KB USB transfers).
pub const WRITE_CHUNK: u64 = 16 * 1024 * 1024;
const SECTOR_SIZE: u64 = 512;

/// Split `total` bytes into `(offset, len)` chunks of at most `chunk` bytes.
pub fn chunk_ranges(total: u64, chunk: u64) -> Vec<(u64, u64)> {
    let mut out = Vec::new();
    let mut off = 0u64;
    while off < total {
        let len = std::cmp::min(chunk, total - off);
        out.push((off, len));
        off += len;
    }
    out
}

/// Write `data` to the device starting at `start_sector`.
///
/// Chunks are addressed by sector (data is byte-for-byte). When `verify`
/// is true an IncrementalChecksum is accumulated and compared against the
/// device CRC over the whole written range.
pub async fn write_raw(
    ctx: &libefex::Context,
    logger: &Logger,
    data: &[u8],
    start_sector: u32,
    verify: bool,
) -> FlashResult<()> {
    let total = data.len() as u64;

    // --- write pass ---
    // Write the whole image as a SINGLE fes_down transaction (one TRANS_FINISH
    // at the very end). Splitting into many smaller transactions, each with its
    // own TRANS_FINISH, makes the sprite mis-handle flushing and corrupts data
    // (the on-device CRC reads cache and falsely passes). Per-partition tools
    // (e.g. OpenixSuit) use one transaction per region for the same reason.
    let pb = progress_bar(total);
    let write_result = ctx
        .fes_down_with_progress(data, start_sector, FesDataType::Flash, |transferred, _total| {
            pb.set_position(transferred)
        })
        .map_err(|e| FlashError::UsbTransferError(e.to_string()));
    pb.finish_and_clear();
    write_result?;
    logger.info(&format!("Wrote {} bytes ({} MB)", total, total / 1024 / 1024));

    if verify {
        verify_chunked(ctx, logger, data, start_sector).await;
    }
    Ok(())
}

/// Verify the written data chunk-by-chunk (a single full-disk verify command
/// overruns the device's USB timeout). Non-fatal: the write already succeeded,
/// so verification problems are reported as warnings rather than failing.
async fn verify_chunked(ctx: &libefex::Context, logger: &Logger, data: &[u8], start_sector: u32) {
    let total = data.len() as u64;
    let vpb = progress_bar(total);
    let mut verified = 0u64;
    let mut mismatches = 0u32;
    let mut errors = 0u32;

    for (offset, len) in chunk_ranges(total, WRITE_CHUNK) {
        let chunk = &data[offset as usize..(offset + len) as usize];
        let mut cs = IncrementalChecksum::new();
        cs.update(chunk);
        let local = cs.finalize();
        let chunk_start_sector = start_sector.wrapping_add((offset / SECTOR_SIZE) as u32);

        match ctx.fes_verify_value(chunk_start_sector, len) {
            Ok(resp) if resp.flag == EFEX_CRC32_VALID_FLAG => {
                if resp.media_crc as u32 != local {
                    mismatches += 1;
                    logger.debug(&format!(
                        "Verify mismatch at sector {}: local=0x{:08x} device=0x{:08x}",
                        chunk_start_sector, local, resp.media_crc as u32
                    ));
                }
            }
            Ok(resp) => {
                errors += 1;
                logger.debug(&format!(
                    "Verify status at sector {}: flag=0x{:08x}",
                    chunk_start_sector, resp.flag
                ));
            }
            Err(e) => {
                errors += 1;
                logger.debug(&format!("Verify error at sector {}: {}", chunk_start_sector, e));
            }
        }
        verified += len;
        vpb.set_position(verified);
    }
    vpb.finish_and_clear();

    if mismatches == 0 && errors == 0 {
        logger.info("Verification passed");
    } else {
        logger.warn(&format!(
            "Verification finished with {} mismatch(es) and {} error(s)",
            mismatches, errors
        ));
    }
}

/// Build a byte-oriented progress bar (elapsed / bar / bytes / speed / ETA).
fn progress_bar(total: u64) -> ProgressBar {
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::with_template(
            "  [{elapsed_precise}] [{bar:30.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, ETA {eta})",
        )
        .unwrap()
        .progress_chars("=>-"),
    );
    pb.enable_steady_tick(Duration::from_millis(120));
    pb
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_exact_multiple() {
        assert_eq!(chunk_ranges(20, 10), vec![(0, 10), (10, 10)]);
    }

    #[test]
    fn chunks_with_remainder() {
        assert_eq!(chunk_ranges(25, 10), vec![(0, 10), (10, 10), (20, 5)]);
    }

    #[test]
    fn chunks_empty_and_small() {
        assert_eq!(chunk_ranges(0, 10), Vec::<(u64, u64)>::new());
        assert_eq!(chunk_ranges(7, 10), vec![(0, 7)]);
    }
}
