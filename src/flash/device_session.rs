//! Device connection helper shared by flash commands.

use crate::utils::{FlashError, FlashResult, Logger};

/// Scan/open a device (specific bus+port, or first available), init USB+efex,
/// and return the ready context together with its detected mode.
pub fn connect(
    logger: &Logger,
    bus: Option<u8>,
    port: Option<u8>,
) -> FlashResult<(libefex::Context, libefex::DeviceMode)> {
    let mut ctx = if let (Some(bus), Some(port)) = (bus, port) {
        let mut ctx = libefex::Context::new();
        ctx.scan_usb_device_at(bus, port)
            .map_err(|e| FlashError::DeviceOpenFailed(e.to_string()))?;
        ctx
    } else {
        let devices = libefex::Context::scan_usb_devices()
            .map_err(|e| FlashError::DeviceOpenFailed(e.to_string()))?;
        if devices.is_empty() {
            return Err(FlashError::DeviceNotFound);
        }
        let mut ctx = libefex::Context::new();
        ctx.scan_usb_device_at(devices[0].bus, devices[0].port)
            .map_err(|e| FlashError::DeviceOpenFailed(e.to_string()))?;
        ctx
    };

    ctx.usb_init().map_err(|e| FlashError::DeviceOpenFailed(e.to_string()))?;
    ctx.efex_init().map_err(|e| FlashError::DeviceOpenFailed(e.to_string()))?;

    let mode = ctx.get_device_mode();
    logger.info(&format!("Device mode: {:?}", mode));
    Ok((ctx, mode))
}
