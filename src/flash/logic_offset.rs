//! FES "logical sector compensation" for SD/eMMC.
//!
//! Reverse-engineered from the OpenixSuit GUI flasher (`通用固件烧写` →
//! `逻辑扇区补偿模式`), which is the only tool confirmed to flash a full-disk
//! raw.img to an A733 (sun60iw2) eMMC such that it actually boots.
//!
//! ## The device-side mapping
//!
//! When flash access is on, the FES `Flash`-tagged transfers (`fes_down` /
//! `fes_up`) address a *logical* sector space whose origin is offset from the
//! physical media by a fixed reserve:
//!
//! ```text
//! physical_sector = fes_logical_sector + LOGIC_OFFSET   (mod 2^32)
//! ```
//!
//! For SD/eMMC the reserve is `LOGIC_OFFSET = 40960` sectors (20 MiB). So to
//! land data at *physical* sector `P` you must transfer it at FES logical
//! sector `P - LOGIC_OFFSET` (u32 wrapping).
//!
//! ## Evidence
//!
//! OpenixSuit's flash log for a whole-disk GPT image shows a single transfer:
//!
//! ```text
//! Starting partition download: raw
//! Partition address: 0xffff6000        # = (u32)(0 - 40960)
//! Partition size: 5408768 sectors
//! Data offset: 0, Data length: 2769289216 bytes
//! ```
//!
//! i.e. the full image (whose first byte is physical sector 0) is written
//! starting at FES logical sector `0xffff6000`. `libefex`'s chunker increments
//! the address as `uint32_t` (`src/efex-fes.c`: `addr_cur += length/512`), so
//! the first 40960 sectors land at `0xffff6000..=0xffffffff`, then the address
//! wraps to `0` and the rest follows — placing every image sector `N` at
//! physical sector `N`.
//!
//! This single linear model is consistent with every observation made while
//! debugging on real hardware:
//! - logical `0 - 40960` → physical 0  → boots (OpenixSuit, and us once fixed)
//! - logical `0`         → physical +40960 → does not boot
//! - logical `0 + 40960` → physical +81920 → does not boot (read-back still
//!   "passes" because `fes_up` reads the same logical space self-consistently)

/// Physical sectors reserved before FES logical sector 0 on SD/eMMC.
/// Matches OpenixSuit's default "偏移补偿" for the SD/eMMC storage type.
pub const DEFAULT_LOGIC_OFFSET: u32 = 40960;

/// Translate a physical sector to the FES logical sector to address it with.
///
/// `physical` is where the data must land on the media (e.g. a GPT LBA, or 0
/// for the start of a full-disk image). `logic_offset` is the device reserve
/// (`DEFAULT_LOGIC_OFFSET` for SD/eMMC, or 0 to address physical sectors
/// directly, as on NAND). The subtraction wraps at 2^32, matching the
/// `uint32_t` arithmetic in libefex's transfer loop.
#[inline]
pub fn fes_logical_sector(physical: u32, logic_offset: u32) -> u32 {
    physical.wrapping_sub(logic_offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_of_full_image_matches_openixsuit() {
        // Whole raw.img starts at physical 0; OpenixSuit logs 0xffff6000.
        assert_eq!(fes_logical_sector(0, DEFAULT_LOGIC_OFFSET), 0xffff_6000);
    }

    #[test]
    fn reserve_boundary_wraps_to_zero() {
        // Physical sector == reserve maps to logical 0 (the wrap point).
        assert_eq!(fes_logical_sector(DEFAULT_LOGIC_OFFSET, DEFAULT_LOGIC_OFFSET), 0);
        // One past the reserve is logical 1.
        assert_eq!(fes_logical_sector(DEFAULT_LOGIC_OFFSET + 1, DEFAULT_LOGIC_OFFSET), 1);
    }

    #[test]
    fn gpt_header_and_partition_lbas() {
        // GPT header at physical LBA 1.
        assert_eq!(fes_logical_sector(1, DEFAULT_LOGIC_OFFSET), 0xffff_6001);
        // A partition that starts inside the reserve (boot @ 32768) maps to a
        // negative logical sector, i.e. a high u32.
        assert_eq!(fes_logical_sector(32768, DEFAULT_LOGIC_OFFSET), 0xffff_e000);
    }

    #[test]
    fn zero_offset_is_identity() {
        // NAND / "no compensation": logical == physical.
        assert_eq!(fes_logical_sector(0, 0), 0);
        assert_eq!(fes_logical_sector(32768, 0), 32768);
    }
}
