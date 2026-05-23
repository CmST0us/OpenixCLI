pub mod layout;

// flash-raw: write a whole raw.img to the device verbatim from sector 0.

use crate::flash::device_session;
use crate::flash::fel_handler::FelBootstrap;
use crate::flash::raw_writer;
use crate::utils::{FlashError, FlashResult, Logger};
use std::time::Duration;

pub struct RawImageOptions {
    pub bus: Option<u8>,
    pub port: Option<u8>,
    pub verify: bool,
    pub post_action: String,
    pub uboot_sector: usize, // default layout::UBOOT_START_SECTOR
}

/// Flash an entire raw image. `img` is the memory-mapped raw.img.
pub async fn flash_raw_image(logger: &Logger, img: &[u8], opts: &RawImageOptions) -> FlashResult<()> {
    let (mut ctx, mode) = device_session::connect(logger, opts.bus, opts.port)?;

    if mode == libefex::DeviceMode::Fel {
        logger.info("Device in FEL: bootstrapping from raw.img boot0/uboot...");
        let bs = layout::extract_bootstrap(img, opts.uboot_sector)?;
        // boot0/uboot are borrowed from img; copy to owned buffers so the
        // bootstrap can mutate (work_mode) without aliasing the input slice.
        let boot0 = bs.boot0.to_vec();
        let uboot = bs.uboot.to_vec();
        FelBootstrap::new(logger)
            .run(&mut ctx, &boot0, &uboot, None, None, None)
            .await?;
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

async fn reconnect_fes(logger: &Logger) -> FlashResult<libefex::Context> {
    tokio::time::sleep(Duration::from_secs(2)).await;
    let max_retries = 25;
    let mut retries = 0;
    while retries < max_retries {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let devices = match libefex::Context::scan_usb_devices() {
            Ok(d) => d,
            Err(_) => { retries += 1; continue; }
        };
        for dev in devices {
            let mut ctx = libefex::Context::new();
            if ctx.scan_usb_device_at(dev.bus, dev.port).is_err() { continue; }
            if ctx.usb_init().is_err() { continue; }
            if ctx.efex_init().is_err() { continue; }
            if ctx.get_device_mode() == libefex::DeviceMode::Srv {
                logger.debug(&format!("Reconnected at bus {}, port {}", dev.bus, dev.port));
                return Ok(ctx);
            }
        }
        retries += 1;
    }
    Err(FlashError::ReconnectFailed)
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
