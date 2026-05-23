//! Generic raw sector writer
//!
//! Writes an in-memory byte slice to device storage starting at a given
//! sector, in fixed-size chunks, with progress reporting and optional
//! checksum verification. Shared by flash-raw (full image) and flash-part
//! (raw partition images).

#![allow(dead_code)]

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

/// Max write+verify passes (re-write mismatched regions, then re-check).
const MAX_VERIFY_PASSES: u32 = 3;

/// Write `data` to the device starting at `start_sector`.
///
/// `storage_type` is the value from `fes_query_storage`, used to toggle flash
/// access (which also flushes/invalidates the write cache so read-back hits the
/// actual media). When `verify` is true the data is read back via `fes_up` and
/// compared byte-for-byte; mismatched regions are rewritten (up to
/// MAX_VERIFY_PASSES). The caller is expected to have already enabled flash
/// access; this function toggles it internally for the flush/read-back cycle.
pub async fn write_raw(
    ctx: &libefex::Context,
    logger: &Logger,
    data: &[u8],
    start_sector: u32,
    storage_type: u32,
    verify: bool,
) -> FlashResult<()> {
    let total = data.len() as u64;

    // --- write pass ---
    // Write the whole image as a SINGLE fes_down transaction (one TRANS_FINISH
    // at the very end).
    let pb = progress_bar(total);
    let write_result = ctx
        .fes_down_with_progress(data, start_sector, FesDataType::Flash, |transferred, _total| {
            pb.set_position(transferred)
        })
        .map_err(|e| FlashError::UsbTransferError(e.to_string()));
    pb.finish_and_clear();
    write_result?;
    logger.info(&format!("Wrote {} bytes ({} MB)", total, total / 1024 / 1024));

    if !verify {
        return Ok(());
    }

    // --- verify by read-back, rewriting bad regions ---
    for pass in 1..=MAX_VERIFY_PASSES {
        // Flush + reopen flash access so the read-back hits the media, not cache.
        let _ = ctx.fes_flash_set_onoff(storage_type, false);
        ctx.fes_flash_set_onoff(storage_type, true)
            .map_err(|e| FlashError::UsbTransferError(e.to_string()))?;

        let bad = readback_compare(ctx, logger, data, start_sector)?;
        if bad.is_empty() {
            logger.info("Verification passed (read-back compare)");
            return Ok(());
        }
        if pass == MAX_VERIFY_PASSES {
            logger.warn(&format!(
                "Verification still failing after {} pass(es): {} bad region(s) remain",
                pass,
                bad.len()
            ));
            return Ok(());
        }
        logger.warn(&format!(
            "Read-back found {} bad region(s); rewriting (pass {}/{})",
            bad.len(),
            pass,
            MAX_VERIFY_PASSES
        ));
        for (offset, len) in bad {
            let chunk = &data[offset as usize..(offset + len) as usize];
            let sector = start_sector.wrapping_add((offset / SECTOR_SIZE) as u32);
            ctx.fes_down(chunk, sector, FesDataType::Flash)
                .map_err(|e| FlashError::UsbTransferError(e.to_string()))?;
        }
    }
    Ok(())
}

/// Read the written data back via `fes_up` and compare to `data`. Returns the
/// list of `(offset, len)` chunks that did not match.
fn readback_compare(
    ctx: &libefex::Context,
    logger: &Logger,
    data: &[u8],
    start_sector: u32,
) -> FlashResult<Vec<(u64, u64)>> {
    let total = data.len() as u64;
    let vpb = progress_bar(total);
    let mut bad = Vec::new();
    let mut done = 0u64;

    for (offset, len) in chunk_ranges(total, WRITE_CHUNK) {
        let sector = start_sector.wrapping_add((offset / SECTOR_SIZE) as u32);
        let mut buf = vec![0u8; len as usize];
        ctx.fes_up(&mut buf, sector, FesDataType::Flash)
            .map_err(|e| FlashError::UsbTransferError(e.to_string()))?;

        let src = &data[offset as usize..(offset + len) as usize];
        if buf.as_slice() != src {
            let first = buf.iter().zip(src).position(|(a, b)| a != b).unwrap_or(0);
            logger.debug(&format!(
                "Mismatch at sector {} (image offset {}): first differing byte at 0x{:x}",
                sector,
                offset,
                offset + first as u64
            ));
            bad.push((offset, len));
        }
        done += len;
        vpb.set_position(done);
    }
    vpb.finish_and_clear();
    Ok(bad)
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
