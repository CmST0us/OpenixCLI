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

/// Max attempts to write+verify a single chunk before giving up.
const MAX_CHUNK_ATTEMPTS: u32 = 3;

/// Write `data` to the device starting at `start_sector`, interleaving write and
/// read-back verification per chunk.
///
/// Each chunk is written then immediately read back (`fes_up`) and compared
/// byte-for-byte; a mismatch rewrites the chunk (up to MAX_CHUNK_ATTEMPTS). This
/// mirrors how working tools (e.g. OpenixSuit) flash raw images: writing the
/// whole image before verifying lets the sprite's write cache overflow and
/// corrupt data (while the on-device CRC, read from cache, falsely passes).
/// Verifying each chunk forces it to the media and keeps the in-flight data
/// small. `storage_type` is currently unused but kept for API symmetry.
pub async fn write_raw(
    ctx: &libefex::Context,
    logger: &Logger,
    data: &[u8],
    start_sector: u32,
    _storage_type: u32,
    verify: bool,
) -> FlashResult<()> {
    let total = data.len() as u64;
    let pb = progress_bar(total);
    let mut done = 0u64;

    for (offset, len) in chunk_ranges(total, WRITE_CHUNK) {
        let chunk = &data[offset as usize..(offset + len) as usize];
        let sector = start_sector.wrapping_add((offset / SECTOR_SIZE) as u32);
        let base = done;

        let mut attempt = 0;
        loop {
            attempt += 1;
            ctx.fes_down_with_progress(chunk, sector, FesDataType::Flash, |transferred, _total| {
                pb.set_position(base + transferred)
            })
            .map_err(|e| FlashError::UsbTransferError(e.to_string()))?;

            if !verify {
                break;
            }

            let mut buf = vec![0u8; len as usize];
            ctx.fes_up(&mut buf, sector, FesDataType::Flash)
                .map_err(|e| FlashError::UsbTransferError(e.to_string()))?;
            if buf.as_slice() == chunk {
                break;
            }

            if attempt >= MAX_CHUNK_ATTEMPTS {
                pb.finish_and_clear();
                let first = buf.iter().zip(chunk).position(|(a, b)| a != b).unwrap_or(0);
                return Err(FlashError::UsbTransferError(format!(
                    "chunk at sector {} (image offset {}) still mismatched after {} attempts (first diff at 0x{:x})",
                    sector,
                    offset,
                    attempt,
                    offset + first as u64
                )));
            }
            logger.debug(&format!(
                "Chunk at sector {} mismatched, retry {}/{}",
                sector, attempt, MAX_CHUNK_ATTEMPTS
            ));
        }

        done += len;
        pb.set_position(done);
    }

    pb.finish_and_clear();
    if verify {
        logger.info(&format!(
            "Wrote and verified {} bytes ({} MB)",
            total,
            total / 1024 / 1024
        ));
    } else {
        logger.info(&format!("Wrote {} bytes ({} MB)", total, total / 1024 / 1024));
    }
    Ok(())
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
