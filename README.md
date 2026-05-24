# OpenixCLI

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/Rust-2021-orange.svg)](https://www.rust-lang.org/)

A command-line & tui firmware flashing tool for Allwinner chips, written in Rust.

<img width="1090" height="583" alt="4610154f-10af-4016-afdd-cf5bdebb59e2" src="https://github.com/user-attachments/assets/39ae92c1-2ff7-45bf-95f8-361327f44ae6" />

## Overview

OpenixCLI is a powerful and user-friendly CLI tool designed for flashing firmware to devices powered by Allwinner SoCs. It supports both FEL (USB Boot) mode and FES (U-Boot) mode, providing a complete solution for firmware deployment.

## Features

- **Device Scanning**: Automatically detect connected Allwinner devices
- **Firmware Flashing**: Flash LiveSuit/IMAGEWTY firmware images with multiple modes
- **Whole-Disk Raw Images**: Flash a full `raw.img` (GPT + boot0 + toc1/u-boot) with automatic logical-sector compensation for SD/eMMC
- **Single-Partition Flashing**: Write one partition image (raw or Android sparse) into the device's existing GPT
- **FEL/FES Support**: Handles both FEL (USB Boot) and FES (U-Boot) device modes, including bootstrapping FEL → FES from a LiveSuit firmware
- **Bootstrap Builder**: Strip a full firmware down to a minimal FEL→FES bootstrap image
- **Verification**: Optional read-back verification for data integrity
- **Progress Tracking**: Visual progress indicators during flash operations
- **Verbose Logging**: Detailed debug output for troubleshooting

## Installation

### Prerequisites

- Rust toolchain (1.70 or later)
- libusb development libraries

### Build from Source

```bash
git clone https://github.com/YuzukiTsuru/OpenixCLI
cd OpenixCLI
cargo build --release
```

The compiled binary will be available at `target/release/openixcli`.

## Usage

### Scan for Devices

List all connected Allwinner devices:

```bash
openixcli scan
```

### Flash Firmware

Flash firmware to a device:

```bash
openixcli flash <firmware_file> [options]
```

#### Flash Options

| Option | Short | Description |
|--------|-------|-------------|
| `--bus` | `-b` | USB bus number |
| `--port` | `-P` | USB port number |
| `--verify` | `-V` | Enable verification after write (default: true) |
| `--mode` | `-m` | Flash mode: `partition`, `keep_data`, `partition_erase`, `full_erase` (default: full_erase) |
| `--partitions` | `-p` | Comma-separated list of partitions to flash |
| `--post-action` | `-a` | Post-flash action: `reboot`, `poweroff`, `shutdown` (default: reboot) |
| `--verbose` | `-v` | Enable verbose output |

#### Flash Examples

Flash firmware to a specific device:

```bash
openixcli flash firmware.img --bus 1 --port 5
```

Flash only specific partitions:

```bash
openixcli flash firmware.img --partitions "boot,system"
```

Flash with verification disabled:

```bash
openixcli flash firmware.img --verify false
```

Flash and power off after completion:

```bash
openixcli flash firmware.img --post-action poweroff
```

### Flash a Raw Disk Image

Write a whole-disk `raw.img` (containing the GPT partition table, boot0 and
toc1/u-boot) to the device:

```bash
openixcli flash-raw <raw.img> [options]
```

#### flash-raw Options

| Option | Short | Description |
|--------|-------|-------------|
| `--bootstrap` | | LiveSuit/IMAGEWTY firmware used to bring the device from FEL into FES (required for modern SoCs, e.g. A733) |
| `--logic-offset` | | FES logical-sector compensation, in sectors (default: `40960` for SD/eMMC; use `0` for NAND) |
| `--verify` | `-V` | Read-back verification after write (default: `true`) |
| `--post-action` | `-a` | Post-flash action: `reboot` (default), `poweroff` |
| `--bus` | `-b` | USB bus number |
| `--port` | `-P` | USB port number |
| `--verbose` | `-v` | Enable verbose output |

If the device is already in FES mode the image is written directly. If it is in
FEL mode, pass `--bootstrap` with a LiveSuit firmware for the target SoC;
OpenixCLI initializes DRAM and runs that firmware's u-boot to enter FES. A
prebuilt A733 bootstrap is bundled at `misc/a733_bootstrap.img` (see
[`mkbootstrap`](#build-a-minimal-bootstrap-firmware) to make your own).

**Logical-sector compensation:** on SD/eMMC the FES flash address space is
offset from the physical media by a reserved boot region (40960 sectors / 20
MiB). OpenixCLI writes the image starting at the compensated address so it lands
at the correct physical sectors and the device boots. The `40960` default is
correct for SD/eMMC; only change `--logic-offset` for other storage (e.g. `0`
for NAND).

Examples — flash an A733 device that is in FEL mode:

```bash
openixcli flash-raw raw.img --bootstrap misc/a733_bootstrap.img
```

Skip the read-back verification to roughly halve the flashing time:

```bash
openixcli flash-raw raw.img --bootstrap misc/a733_bootstrap.img -V false
```

### Flash a Single Partition

Flash one partition image (raw or Android sparse) into the device's existing
GPT layout:

```bash
openixcli flash-part <partition_name> <partition.img> [options]
```

#### flash-part Options

| Option | Short | Description |
|--------|-------|-------------|
| `--bootstrap` | | LiveSuit firmware to bootstrap FEL → FES (needed to read the GPT when the device is in FEL) |
| `--logic-offset` | | FES logical-sector compensation, in sectors (default: `40960` for SD/eMMC; use `0` for NAND) — must match how the device GPT was written |
| `--verify` | `-V` | Read-back verification after write (default: `true`) |
| `--post-action` | `-a` | Post-flash action: `none` (default), `reboot`, `poweroff` |
| `--bus` | `-b` | USB bus number |
| `--port` | `-P` | USB port number |

The device's GPT is read back (using the same logical-sector compensation) to
locate the partition; if the name is not found, all available partition names
are listed. Sparse images are detected automatically.

```bash
openixcli flash-part boot boot.img --bootstrap misc/a733_bootstrap.img
```

### Build a Minimal Bootstrap Firmware

Strip a full LiveSuit/IMAGEWTY firmware down to just the entries needed to
bring a device from FEL into FES (fes1, u-boot, sys_config, board config, dtb).
The result is a small image suitable for the `--bootstrap` option above:

```bash
openixcli mkbootstrap <full_livesuit.img> <bootstrap_out.img>
```

## Flash Modes

| Mode | Description |
|------|-------------|
| `partition` | Flash specific partitions only |
| `keep_data` | Flash while preserving user data |
| `partition_erase` | Erase and flash specific partitions |
| `full_erase` | Full erase before flashing (default) |

## Device Modes

OpenixCLI supports the following device modes:

- **FEL (USB Boot)**: Initial boot mode for firmware flashing
- **FES (U-Boot)**: Secondary mode after U-Boot is loaded
- **UPDATE_COOL/UPDATE_HOT**: Update modes

## Project Structure

```
OpenixCLI/
├── src/
│   ├── commands/      # CLI command implementations
│   ├── config/        # Configuration parsing (GPT, MBR, boot headers, sys_config)
│   ├── firmware/      # Firmware image handling (IMAGEWTY, sparse, bootstrap builder)
│   ├── flash/         # Flashing logic (FEL/FES handlers, raw/partition writers, logic-offset)
│   ├── utils/         # Utilities (logging, errors)
│   ├── cli.rs         # CLI argument definitions
│   ├── lib.rs         # Library exports
│   └── main.rs        # Application entry point
├── Cargo.toml
└── LICENSE
```

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## Acknowledgments

- Built with [libefex](https://github.com/YuzukiTsuru/libefex) for Allwinner USB communication
- Inspired by the need for a modern, reliable firmware flashing tool for Allwinner devices
