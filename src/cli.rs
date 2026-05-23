//! Command-line interface definitions
//!
//! Defines the CLI structure using clap for argument parsing

use clap::{Parser, Subcommand};

/// Main CLI structure
///
/// # Fields
/// * `command` - The subcommand to execute (scan, flash, or tui). Defaults to TUI if none given.
/// * `verbose` - Enable verbose output
#[derive(Parser)]
#[command(name = "openixcli")]
#[command(about = "Firmware flashing CLI tool for Allwinner chips", long_about = None)]
#[command(version)]
pub struct Cli {
    /// The subcommand to execute (defaults to TUI if omitted)
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Enable verbose output
    #[arg(short, long, global = true, help = "Enable verbose output")]
    pub verbose: bool,
}

/// Available CLI commands
#[derive(Subcommand)]
pub enum Commands {
    /// Scan for connected devices
    Scan {
        /// Get detailed device information (requires device initialization)
        #[arg(short = 'l', long, help = "Get detailed device information")]
        detailed: bool,
    },

    /// Flash firmware to device
    Flash {
        /// Path to firmware file
        #[arg(help = "Path to firmware file")]
        firmware: String,

        /// USB bus number
        #[arg(short, long, help = "USB bus number")]
        bus: Option<u8>,

        /// USB port number
        #[arg(short = 'P', long, help = "USB port number")]
        port: Option<u8>,

        /// Enable verification after write
        #[arg(
            short = 'V',
            long,
            default_value = "true",
            help = "Enable verification after write"
        )]
        verify: bool,

        /// Flash mode
        /// - partition: Flash only specified partitions
        /// - keep_data: Keep existing data
        /// - partition_erase: Erase partitions before flashing
        /// - full_erase: Erase all data before flashing
        #[arg(
            short,
            long,
            default_value = "full_erase",
            help = "Flash mode: partition, keep_data, partition_erase, full_erase"
        )]
        mode: String,

        /// Partitions to flash (comma-separated)
        #[arg(short = 'p', long, help = "Partitions to flash (comma-separated)")]
        partitions: Option<String>,

        /// Post-flash action
        /// - reboot: Reboot device after flashing
        /// - poweroff: Power off device after flashing
        /// - shutdown: Shutdown device after flashing
        #[arg(
            short = 'a',
            long,
            default_value = "reboot",
            help = "Post-flash action: reboot, poweroff, shutdown"
        )]
        post_action: String,
    },

    /// Flash a whole raw.img (GPT + boot0 + toc1) to the device
    FlashRaw {
        /// Path to raw.img
        #[arg(help = "Path to raw disk image")]
        image: String,

        #[arg(short, long, help = "USB bus number")]
        bus: Option<u8>,

        #[arg(short = 'P', long, help = "USB port number")]
        port: Option<u8>,

        #[arg(short = 'V', long, default_value = "true", help = "Enable verification after write")]
        verify: bool,

        #[arg(short = 'a', long, default_value = "reboot", help = "Post-flash action: reboot, poweroff")]
        post_action: String,

        /// LiveSuit/IMAGEWTY firmware to bootstrap FEL->FES (required for newer SoCs, e.g. A733)
        #[arg(long, help = "LiveSuit firmware (.img) to bootstrap from when device is in FEL")]
        bootstrap: Option<String>,

        /// Override the u-boot start sector used for FEL bootstrap (advanced)
        #[arg(long, hide = true)]
        uboot_offset: Option<usize>,
    },

    /// Flash one partition image into the device's existing GPT
    FlashPart {
        /// Target partition name (as in the device GPT)
        #[arg(help = "Partition name")]
        partition: String,

        /// Path to the partition image (raw or Android sparse)
        #[arg(help = "Path to partition image")]
        image: String,

        #[arg(short, long, help = "USB bus number")]
        bus: Option<u8>,

        #[arg(short = 'P', long, help = "USB port number")]
        port: Option<u8>,

        #[arg(short = 'V', long, default_value = "true", help = "Enable verification after write")]
        verify: bool,

        #[arg(short = 'a', long, default_value = "none", help = "Post-flash action: none, reboot, poweroff")]
        post_action: String,
    },

    /// Build a minimal bootstrap firmware from a full LiveSuit firmware
    Mkbootstrap {
        /// Path to the full LiveSuit/IMAGEWTY firmware
        #[arg(help = "Path to full LiveSuit firmware (.img)")]
        input: String,

        /// Output path for the minimal bootstrap firmware
        #[arg(help = "Output path for the minimal bootstrap firmware")]
        output: String,
    },

    /// Launch interactive TUI mode
    Tui,
}
