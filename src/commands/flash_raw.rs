//! flash-raw command entry.

use crate::flash::raw_image::layout::UBOOT_START_SECTOR;
use crate::flash::raw_image::{flash_raw_image, RawImageOptions};
use crate::utils::logger::Logger;
use memmap2::Mmap;
use std::fs::File;

#[allow(clippy::too_many_arguments)]
pub async fn execute(
    image: String,
    bus: Option<u8>,
    port: Option<u8>,
    verify: bool,
    post_action: String,
    uboot_offset: Option<usize>,
    bootstrap: Option<String>,
    logic_offset: u32,
    verbose: bool,
) -> anyhow::Result<()> {
    let logger = Logger::with_verbose(verbose);
    let path = std::path::Path::new(&image);
    if !path.exists() {
        return Err(anyhow::anyhow!("Raw image not found: {}", image));
    }
    if let Some(ref fw) = bootstrap {
        if !std::path::Path::new(fw).exists() {
            return Err(anyhow::anyhow!("Bootstrap firmware not found: {}", fw));
        }
    }
    let file = File::open(path)?;
    let mmap = unsafe { Mmap::map(&file)? };
    logger.info(&format!("Loaded raw image: {} ({} bytes)", image, mmap.len()));

    let opts = RawImageOptions {
        bus,
        port,
        verify,
        post_action,
        uboot_sector: uboot_offset.unwrap_or(UBOOT_START_SECTOR),
        bootstrap,
        logic_offset,
    };
    if let Err(e) = flash_raw_image(&logger, &mmap, &opts).await {
        logger.error(&format!("flash-raw failed: {}", e));
        return Err(anyhow::anyhow!("{}", e));
    }
    Ok(())
}
