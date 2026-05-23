//! Shared FEL bootstrap: run DRAM init blob, then download & start u-boot.
//!
//! Used by both the IMAGEWTY flasher (blobs from the packer) and the raw.img
//! flasher (blobs sliced from the image).

use crate::flash::fel_handler::{DramInit, UbootDownload};
use crate::utils::{FlashResult, Logger};

pub struct FelBootstrap<'a> {
    logger: &'a Logger,
}

impl<'a> FelBootstrap<'a> {
    pub fn new(logger: &'a Logger) -> Self {
        Self { logger }
    }

    /// `dram_init_blob` is an eGON.BT0 image (fes1 or boot0). `uboot_blob`
    /// is a sunxi u-boot image; `sys_config` is optional.
    pub async fn run(
        &self,
        ctx: &mut libefex::Context,
        dram_init_blob: &[u8],
        uboot_blob: &[u8],
        dtb: Option<&[u8]>,
        sys_config: Option<&[u8]>,
        board_config: Option<&[u8]>,
    ) -> FlashResult<()> {
        let dram = DramInit::new(self.logger);
        dram.execute(ctx, dram_init_blob).await?;

        let uboot = UbootDownload::new(self.logger);
        uboot
            .execute(ctx, uboot_blob, dtb, sys_config, board_config)
            .await?;
        Ok(())
    }
}
