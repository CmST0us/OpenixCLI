pub mod gpt_reader;

// flash-part: flash one partition image into the device's existing GPT layout.

use crate::firmware::sparse::{is_sparse_format, SPARSE_HEADER_SIZE};
use crate::firmware::StorageType;
use crate::flash::device_session;
use crate::flash::fel_bootstrap::{bootstrap_from_firmware, reconnect_fes};
use crate::flash::fes_handler::{SparseDownloadParams, SparseDownloader};
use crate::flash::raw_writer;
use crate::utils::{FlashError, FlashResult, Logger};
use std::io::Cursor;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

pub struct PartitionFlashOptions {
    pub bus: Option<u8>,
    pub port: Option<u8>,
    pub verify: bool,
    /// Post-flash action: "none" (default), "reboot", "poweroff", or "shutdown".
    pub post_action: String,
    /// Optional LiveSuit/IMAGEWTY firmware to bootstrap FEL->FES from. When the
    /// device is in FEL and this is None, the bundled A733 bootstrap is used.
    pub bootstrap: Option<String>,
}

/// Flash `img` (raw or sparse) into the named partition read from the device GPT.
pub async fn flash_partition(
    logger: &Logger,
    partition_name: &str,
    img: &[u8],
    opts: &PartitionFlashOptions,
) -> FlashResult<()> {
    let (mut ctx, mode) = device_session::connect(logger, opts.bus, opts.port)?;
    match mode {
        libefex::DeviceMode::Srv => {}
        libefex::DeviceMode::Fel => {
            // flash-part needs FES to read the GPT; bootstrap from FEL first.
            let firmware_path = opts.bootstrap.as_ref().ok_or_else(|| {
                FlashError::DeviceNotInFes(
                    "FEL (pass --bootstrap misc/a733_bootstrap.img to bootstrap into FES)".into(),
                )
            })?;
            logger.info(&format!(
                "Device in FEL: bootstrapping from firmware {}",
                firmware_path
            ));
            bootstrap_from_firmware(logger, &mut ctx, firmware_path).await?;
            ctx = reconnect_fes(logger).await?;
        }
        other => return Err(FlashError::DeviceNotInFes(format!("{:?}", other))),
    }

    let gpt = gpt_reader::read_gpt(&ctx, logger)?;
    let part = match gpt.find(partition_name) {
        Some(p) => p.clone(),
        None => {
            logger.error(&format!(
                "Partition '{}' not found. Available: {}",
                partition_name,
                gpt.names().join(", ")
            ));
            return Err(FlashError::PartitionNotFound(partition_name.to_string()));
        }
    };

    if (img.len() as u64) > part.size_bytes() {
        return Err(FlashError::RawImageTooLarge {
            image: img.len() as u64,
            capacity: part.size_bytes(),
        });
    }

    let storage_type = ctx
        .fes_query_storage()
        .map_err(|e| FlashError::UsbTransferError(e.to_string()))?;
    logger.info(&format!(
        "Storage type: {} ({})",
        StorageType::from(storage_type),
        storage_type
    ));

    ctx.fes_flash_set_onoff(storage_type, true)
        .map_err(|e| FlashError::UsbTransferError(e.to_string()))?;

    let is_sparse = img.len() >= SPARSE_HEADER_SIZE && is_sparse_format(&img[..SPARSE_HEADER_SIZE]);
    let result: FlashResult<()> = if is_sparse {
        logger.info(&format!("Partition {} image is sparse", partition_name));
        let downloader = SparseDownloader::new(
            logger,
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicU64::new(0)),
        );
        let mut cursor = Cursor::new(img);
        downloader
            .download_sparse_from_reader(
                &ctx,
                &mut cursor,
                &SparseDownloadParams {
                    data_offset: 0,
                    data_length: img.len() as u64,
                    start_sector: part.first_lba as u32,
                    partition_name,
                    verify_enabled: opts.verify,
                },
            )
            .await
    } else {
        logger.info(&format!(
            "Writing {} bytes to partition {} at sector {}",
            img.len(),
            partition_name,
            part.first_lba
        ));
        raw_writer::write_raw(&ctx, logger, img, part.first_lba as u32, storage_type, opts.verify).await
    };

    let _ = ctx.fes_flash_set_onoff(storage_type, false);
    result?;

    if opts.post_action != "none" {
        let tool_mode = match opts.post_action.as_str() {
            "poweroff" | "shutdown" => libefex::FesToolMode::PowerOff,
            _ => libefex::FesToolMode::Reboot,
        };
        ctx.fes_tool_mode(libefex::FesToolMode::Normal, tool_mode)
            .map_err(|e| FlashError::UsbTransferError(e.to_string()))?;
    }
    logger.stage_complete(&format!("Partition {} flashed", partition_name));
    Ok(())
}
