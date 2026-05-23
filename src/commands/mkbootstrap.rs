//! mkbootstrap command entry.
//!
//! Repacks a full LiveSuit/IMAGEWTY firmware into a small bootstrap firmware
//! containing only the FEL->FES bootstrap entries, for use with
//! `flash-raw --bootstrap`.

use crate::firmware::bootstrap_pack::build_bootstrap;
use crate::utils::logger::Logger;
use std::path::Path;

pub fn execute(input: String, output: String, verbose: bool) -> anyhow::Result<()> {
    let logger = Logger::with_verbose(verbose);

    let in_path = Path::new(&input);
    if !in_path.exists() {
        return Err(anyhow::anyhow!("Firmware not found: {}", input));
    }

    logger.info(&format!("Reading firmware: {}", input));
    let summary = build_bootstrap(in_path, Path::new(&output)).map_err(|e| anyhow::anyhow!("{}", e))?;

    for (name, size) in &summary.entries {
        logger.info(&format!("  + {} ({} bytes)", name, size));
    }
    logger.stage_complete(&format!(
        "Bootstrap firmware written: {} ({} bytes)",
        output, summary.output_size
    ));
    Ok(())
}
