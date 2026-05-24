//! Read the primary GPT (header + entries) from a device in FES mode.

use crate::config::gpt_parser::{self, Gpt};
use crate::flash::logic_offset::fes_logical_sector;
use crate::utils::{FlashError, FlashResult, Logger};
use libefex::FesDataType;

const SECTOR_SIZE: usize = 512;

/// Read `count` sectors at physical sector `physical`, applying the FES
/// logical-sector compensation (`logic_offset`) so a physical LBA reads the
/// right media location. The GPT lives at physical LBA 1.
fn read_sectors(
    ctx: &libefex::Context,
    physical: u32,
    count: usize,
    logic_offset: u32,
) -> FlashResult<Vec<u8>> {
    let mut buf = vec![0u8; count * SECTOR_SIZE];
    let sector = fes_logical_sector(physical, logic_offset);
    ctx.fes_up(&mut buf, sector, FesDataType::Flash)
        .map_err(|e| FlashError::UsbTransferError(e.to_string()))?;
    Ok(buf)
}

/// Read and parse the device's primary GPT. `logic_offset` is the SD/eMMC
/// logical-sector compensation (0 for NAND); see `flash::logic_offset`.
pub fn read_gpt(ctx: &libefex::Context, logger: &Logger, logic_offset: u32) -> FlashResult<Gpt> {
    // Physical LBA1: GPT header.
    let lba1 = read_sectors(ctx, 1, 1, logic_offset)?;
    let mut gpt = gpt_parser::parse_header(&lba1)?;

    let entries_bytes = (gpt.num_entries as usize) * (gpt.entry_size as usize);
    let entry_sectors = entries_bytes.div_ceil(SECTOR_SIZE);
    let entries = read_sectors(ctx, gpt.partition_entries_lba as u32, entry_sectors, logic_offset)?;
    gpt_parser::parse_entries(&mut gpt, &lba1, &entries)?;

    logger.info(&format!("Device GPT: {} partitions", gpt.partitions.len()));
    Ok(gpt)
}
