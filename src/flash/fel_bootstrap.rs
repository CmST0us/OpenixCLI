//! Shared FEL->FES bootstrap helpers used by flash-raw and flash-part.
//!
//! Bootstrapping a device from FEL into FES requires a real fes1 + u-boot, taken
//! from a LiveSuit/IMAGEWTY firmware passed via `--bootstrap` (e.g. the minimal
//! `misc/a733_bootstrap.img` produced by `openixcli mkbootstrap`).

use crate::firmware::OpenixPacker;
use crate::flash::fel_handler::FelBootstrap;
use crate::utils::{FlashError, FlashResult, Logger};
use std::time::Duration;

/// Bootstrap FEL->FES using an IMAGEWTY/LiveSuit firmware's fes1 + u-boot.
/// This is the same proven path used by `openixcli flash`.
pub async fn bootstrap_from_firmware(
    logger: &Logger,
    ctx: &mut libefex::Context,
    firmware_path: &str,
) -> FlashResult<()> {
    let mut packer = OpenixPacker::new();
    packer.load(firmware_path)?;

    let fes = packer.get_fes().map_err(|_| FlashError::FesNotFound)?;
    let uboot = packer.get_uboot().map_err(|_| FlashError::UbootNotFound)?;
    let dtb = packer.get_dtb().ok();
    let sys_config = packer.get_sys_config_bin().ok();
    let board_config = packer.get_board_config().ok();

    logger.info(&format!(
        "Bootstrap firmware loaded: fes={} bytes, uboot={} bytes",
        fes.len(),
        uboot.len()
    ));

    FelBootstrap::new(logger)
        .run(
            ctx,
            &fes,
            &uboot,
            dtb.as_deref(),
            sys_config.as_deref(),
            board_config.as_deref(),
        )
        .await
}

/// Wait for the device to re-enumerate in FES (Srv) mode after bootstrap.
pub async fn reconnect_fes(logger: &Logger) -> FlashResult<libefex::Context> {
    tokio::time::sleep(Duration::from_secs(2)).await;
    let max_retries = 25;
    let mut retries = 0;
    while retries < max_retries {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let devices = match libefex::Context::scan_usb_devices() {
            Ok(d) => d,
            Err(e) => {
                retries += 1;
                logger.debug(&format!(
                    "Reconnect attempt {}/{} (scan failed: {})",
                    retries, max_retries, e
                ));
                continue;
            }
        };
        for dev in devices {
            let mut ctx = libefex::Context::new();
            if ctx.scan_usb_device_at(dev.bus, dev.port).is_err() {
                continue;
            }
            if ctx.usb_init().is_err() {
                continue;
            }
            if ctx.efex_init().is_err() {
                continue;
            }
            if ctx.get_device_mode() == libefex::DeviceMode::Srv {
                logger.debug(&format!("Reconnected at bus {}, port {}", dev.bus, dev.port));
                return Ok(ctx);
            }
        }
        retries += 1;
        logger.debug(&format!("Reconnect attempt {}/{}", retries, max_retries));
    }
    Err(FlashError::ReconnectFailed)
}
