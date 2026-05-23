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
use libefex::FesDataType;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Bytes per write chunk fed to libefex (libefex further splits into 64 KB USB transfers).
pub const WRITE_CHUNK: u64 = 16 * 1024 * 1024;
const SPEED_UPDATE_INTERVAL: u64 = 64 * 1024;
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
    let written_bytes = Arc::new(AtomicU64::new(0));
    let last_speed = Arc::new(AtomicU64::new(0));
    let mut checksum: Option<IncrementalChecksum> = if verify { Some(IncrementalChecksum::new()) } else { None };

    for (offset, len) in chunk_ranges(total, WRITE_CHUNK) {
        let chunk = &data[offset as usize..(offset + len) as usize];
        if let Some(ref mut cs) = checksum {
            cs.update(chunk);
        }
        let chunk_start_sector = start_sector.wrapping_add((offset / SECTOR_SIZE) as u32);
        let base = written_bytes.load(Ordering::SeqCst);
        let wb = Arc::clone(&written_bytes);
        let ls = Arc::clone(&last_speed);

        ctx.fes_down_with_progress(chunk, chunk_start_sector, FesDataType::Flash, {
            move |transferred, _total| {
                let current = base + transferred;
                wb.store(current, Ordering::SeqCst);
                let last = ls.load(Ordering::SeqCst);
                if current.saturating_sub(last) >= SPEED_UPDATE_INTERVAL {
                    ls.store(current, Ordering::SeqCst);
                    logger.update_progress_with_speed(current);
                }
            }
        })
        .map_err(|e| FlashError::UsbTransferError(e.to_string()))?;
    }

    if let Some(mut cs) = checksum {
        let local = cs.finalize();
        let resp = ctx
            .fes_verify_value(start_sector, total)
            .map_err(|e| FlashError::UsbTransferError(e.to_string()))?;
        if resp.flag == EFEX_CRC32_VALID_FLAG {
            let device_crc = resp.media_crc as u32;
            if local != device_crc {
                logger.warn(&format!(
                    "Verify mismatch at sector {}: local=0x{:08x} device=0x{:08x}",
                    start_sector, local, device_crc
                ));
            } else {
                logger.info(&format!("Verified {} bytes at sector {}", total, start_sector));
            }
        } else {
            logger.warn(&format!("Verify failed at sector {}: flag=0x{:08x}", start_sector, resp.flag));
        }
    }
    Ok(())
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
