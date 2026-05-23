//! Read the primary GPT (header + entries) from a device in FES mode.

use crate::config::gpt_parser::{self, Gpt};
use crate::utils::{FlashError, FlashResult, Logger};
use libefex::FesDataType;

const SECTOR_SIZE: usize = 512;

/// Read sectors [start_sector, start_sector + count) from the device.
fn read_sectors(ctx: &libefex::Context, start_sector: u32, count: usize) -> FlashResult<Vec<u8>> {
    let mut buf = vec![0u8; count * SECTOR_SIZE];
    ctx.fes_up(&mut buf, start_sector, FesDataType::Flash)
        .map_err(|e| FlashError::UsbTransferError(e.to_string()))?;
    Ok(buf)
}

/// Read and parse the device's primary GPT.
pub fn read_gpt(ctx: &libefex::Context, logger: &Logger) -> FlashResult<Gpt> {
    // LBA1: GPT header.
    let lba1 = read_sectors(ctx, 1, 1)?;
    let mut gpt = gpt_parser::parse_header(&lba1)?;

    let entries_bytes = (gpt.num_entries as usize) * (gpt.entry_size as usize);
    let entry_sectors = entries_bytes.div_ceil(SECTOR_SIZE);
    let entries = read_sectors(ctx, gpt.partition_entries_lba as u32, entry_sectors)?;
    gpt_parser::parse_entries(&mut gpt, &lba1, &entries)?;

    logger.info(&format!("Device GPT: {} partitions", gpt.partitions.len()));
    Ok(gpt)
}
