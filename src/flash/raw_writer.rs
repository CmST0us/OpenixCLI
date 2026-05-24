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

/// Size of the device-managed metadata window at FES logical sector 0. When
/// logical-sector compensation is active, the start of the logical-flash region
/// (physical sector == logic_offset) holds the sunxi sprite's logical MBR /
/// metadata: the device rewrites it during flashing, so the bytes we sent there
/// never round-trip on read-back. Mismatches confined to this window are
/// expected and must not be treated as flash failures. 16 KiB == the classic
/// sunxi MBR size.
const SPRITE_META_BYTES: u64 = 16 * 1024;

/// Write `data` to the device starting at `start_sector`.
///
/// `storage_type` is the value from `fes_query_storage`, used to toggle flash
/// access (which also flushes/invalidates the write cache so read-back hits the
/// actual media). When `verify` is true the data is read back via `fes_up` and
/// compared byte-for-byte; mismatched regions are rewritten (up to
/// MAX_VERIFY_PASSES). Mismatches confined to the device-managed metadata
/// window at FES logical sector 0 (see `SPRITE_META_BYTES`) are expected and
/// ignored. The caller is expected to have already enabled flash access; this
/// function toggles it internally for the flush/read-back cycle.
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

/// True if the differing byte range `[first_abs, last_abs]` lies entirely
/// inside the device-managed metadata window at FES logical sector 0
/// (`wrap_off` is the image offset of that sector, or None when compensation
/// is disabled).
fn in_metadata_window(first_abs: u64, last_abs: u64, wrap_off: Option<u64>) -> bool {
    wrap_off.is_some_and(|w| first_abs >= w && last_abs < w + SPRITE_META_BYTES)
}

/// Read the written data back via `fes_up` and compare to `data`. Returns the
/// list of `(offset, len)` chunks that did not match. Differences confined to
/// the device-managed metadata window at FES logical sector 0 are reported at
/// debug level but excluded from the returned list (they are expected).
fn readback_compare(
    ctx: &libefex::Context,
    logger: &Logger,
    data: &[u8],
    start_sector: u32,
) -> FlashResult<Vec<(u64, u64)>> {
    let total = data.len() as u64;
    // Image offset at which the FES logical address wraps to 0 (the start of the
    // device's logical-flash region). Only meaningful when compensation is on.
    let wrap_off = match start_sector {
        0 => None,
        s => Some((0u32.wrapping_sub(s)) as u64 * SECTOR_SIZE),
    };
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
            let last = buf.iter().zip(src).rposition(|(a, b)| a != b).unwrap_or(first);
            let diff_count = buf.iter().zip(src).filter(|(a, b)| a != b).count();
            let (first_abs, last_abs) = (offset + first as u64, offset + last as u64);

            // Ignore mismatches that lie entirely inside the sprite metadata
            // window at FES logical sector 0 (the device owns those bytes).
            let benign = in_metadata_window(first_abs, last_abs, wrap_off);
            if benign {
                logger.debug(&format!(
                    "Ignoring device-managed metadata at image offset 0x{:x} ({} byte(s))",
                    first_abs,
                    last_abs - first_abs + 1,
                ));
            } else {
                let dump = |b: &[u8], at: usize| -> String {
                    b[at..(at + 16).min(b.len())]
                        .iter()
                        .map(|x| format!("{:02x}", x))
                        .collect::<Vec<_>>()
                        .join(" ")
                };
                logger.debug(&format!(
                    "Mismatch at sector {} (image offset {}): {} differing byte(s), first at 0x{:x}\n      src@diff: {}\n      dev@diff: {}",
                    sector,
                    offset,
                    diff_count,
                    first_abs,
                    dump(src, first),
                    dump(&buf, first),
                ));
                bad.push((offset, len));
            }
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

    #[test]
    fn metadata_window_matches_logic0_boundary() {
        // logic_offset=40960 -> start_sector 0xffff6000 -> wrap at 20 MiB.
        let wrap = Some(40960u64 * 512); // 20 MiB
        let w = wrap.unwrap();
        // The observed 22-byte diff at exactly the boundary is benign.
        assert!(in_metadata_window(w, w + 21, wrap));
        // A diff at the very end of the window is still benign.
        assert!(in_metadata_window(w + 100, w + SPRITE_META_BYTES - 1, wrap));
    }

    #[test]
    fn metadata_window_excludes_real_mismatches() {
        let wrap = Some(40960u64 * 512);
        let w = wrap.unwrap();
        // Just past the window is a real mismatch.
        assert!(!in_metadata_window(w, w + SPRITE_META_BYTES, wrap));
        // Before the boundary (e.g. GPT at 512) is a real mismatch.
        assert!(!in_metadata_window(512, 1024, wrap));
        // A diff that straddles the window end is not fully benign.
        assert!(!in_metadata_window(w + 8, w + SPRITE_META_BYTES + 8, wrap));
    }

    #[test]
    fn metadata_window_disabled_without_compensation() {
        // start_sector==0 -> no wrap window -> nothing is benign.
        assert!(!in_metadata_window(0, 21, None));
        assert!(!in_metadata_window(1024, 2048, None));
    }
}
