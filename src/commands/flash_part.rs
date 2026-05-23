//! flash-part command entry.

use crate::flash::partition_flash::{flash_partition, PartitionFlashOptions};
use crate::utils::logger::Logger;
use memmap2::Mmap;
use std::fs::File;

#[allow(clippy::too_many_arguments)]
pub async fn execute(
    partition: String,
    image: String,
    bus: Option<u8>,
    port: Option<u8>,
    verify: bool,
    post_action: String,
    bootstrap: Option<String>,
    verbose: bool,
) -> anyhow::Result<()> {
    let logger = Logger::with_verbose(verbose);
    let path = std::path::Path::new(&image);
    if !path.exists() {
        return Err(anyhow::anyhow!("Partition image not found: {}", image));
    }
    if let Some(ref fw) = bootstrap {
        if !std::path::Path::new(fw).exists() {
            return Err(anyhow::anyhow!("Bootstrap firmware not found: {}", fw));
        }
    }
    let file = File::open(path)?;
    let mmap = unsafe { Mmap::map(&file)? };
    logger.info(&format!("Loaded partition image: {} ({} bytes)", image, mmap.len()));

    let opts = PartitionFlashOptions { bus, port, verify, post_action, bootstrap };
    if let Err(e) = flash_partition(&logger, &partition, &mmap, &opts).await {
        logger.error(&format!("flash-part failed: {}", e));
        return Err(anyhow::anyhow!("{}", e));
    }
    Ok(())
}
