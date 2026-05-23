pub mod layout;

// flash-raw: write a whole raw.img to the device verbatim from sector 0.

use crate::flash::fel_bootstrap::{
    bootstrap_from_firmware, reconnect_fes, write_bundled_bootstrap,
};
use crate::flash::fel_handler::FelBootstrap;
use crate::flash::raw_writer;
use crate::flash::device_session;
use crate::utils::{FlashError, FlashResult, Logger};

pub struct RawImageOptions {
    pub bus: Option<u8>,
    pub port: Option<u8>,
    pub verify: bool,
    /// Post-flash action: "reboot" (default), "poweroff", or "shutdown".
    pub post_action: String,
    pub uboot_sector: usize, // default layout::UBOOT_START_SECTOR
    /// Optional LiveSuit/IMAGEWTY firmware to bootstrap FEL->FES from.
    /// Required for newer SoCs (e.g. A733) whose boot0 is a real SPL and whose
    /// u-boot is packed in a sunxi-package, so it cannot be sliced out of raw.img.
    pub bootstrap: Option<String>,
}

/// Flash an entire raw image. `img` is the memory-mapped raw.img.
pub async fn flash_raw_image(logger: &Logger, img: &[u8], opts: &RawImageOptions) -> FlashResult<()> {
    let (mut ctx, mode) = device_session::connect(logger, opts.bus, opts.port)?;

    if mode == libefex::DeviceMode::Fel {
        match &opts.bootstrap {
            Some(firmware_path) => {
                logger.info(&format!(
                    "Device in FEL: bootstrapping from firmware {}",
                    firmware_path
                ));
                bootstrap_from_firmware(logger, &mut ctx, firmware_path).await?;
            }
            None => match layout::extract_bootstrap(img, opts.uboot_sector) {
                Ok(bs) => {
                    logger.info("Device in FEL: bootstrapping from raw.img boot0/uboot...");
                    // boot0/uboot are borrowed from img; copy to owned buffers so the
                    // bootstrap can mutate (work_mode) without aliasing the input slice.
                    let boot0 = bs.boot0.to_vec();
                    let uboot = bs.uboot.to_vec();
                    FelBootstrap::new(logger)
                        .run(&mut ctx, &boot0, &uboot, None, None, None)
                        .await?;
                }
                Err(_) => {
                    // Newer SoCs (e.g. A733): boot0 is a real SPL and u-boot is in a
                    // sunxi-package, so it can't be sliced from raw.img. Fall back to
                    // the bundled bootstrap firmware automatically.
                    logger.info(
                        "Device in FEL: raw.img is not legacy-sunxi; using bundled A733 bootstrap",
                    );
                    let path = write_bundled_bootstrap()?;
                    bootstrap_from_firmware(logger, &mut ctx, &path.to_string_lossy()).await?;
                }
            },
        }
        ctx = reconnect_fes(logger).await?;
    } else {
        logger.info("Device already in FES; writing directly");
    }

    // Capacity check.
    let flash_size = ctx
        .fes_probe_flash_size()
        .map_err(|e| FlashError::UsbTransferError(e.to_string()))?;
    let capacity = (flash_size as u64) * 512;
    if (img.len() as u64) > capacity {
        return Err(FlashError::RawImageTooLarge { image: img.len() as u64, capacity });
    }
    logger.info(&format!("Flash capacity: {} MB", capacity / 1024 / 1024));

    ctx.fes_flash_set_onoff(0, true)
        .map_err(|e| FlashError::UsbTransferError(e.to_string()))?;
    logger.info(&format!("Writing {} bytes from sector 0...", img.len()));
    let result = raw_writer::write_raw(&ctx, logger, img, 0, opts.verify).await;
    let _ = ctx.fes_flash_set_onoff(0, false);
    result?;

    set_post_action(&ctx, &opts.post_action)?;
    logger.stage_complete(&format!("raw.img flashed; device will {}", opts.post_action));
    Ok(())
}

fn set_post_action(ctx: &libefex::Context, post_action: &str) -> FlashResult<()> {
    let tool_mode = match post_action {
        "poweroff" | "shutdown" => libefex::FesToolMode::PowerOff,
        _ => libefex::FesToolMode::Reboot,
    };
    ctx.fes_tool_mode(libefex::FesToolMode::Normal, tool_mode)
        .map_err(|e| FlashError::UsbTransferError(e.to_string()))?;
    Ok(())
}
