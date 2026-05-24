# CLAUDE.md

Guidance for working in this repo. OpenixCLI is a Rust CLI/TUI firmware flasher
for Allwinner (sunxi) SoCs, over USB FEL/FES.

## Build & test

```bash
cargo build              # debug -> ./target/debug/openixcli
cargo build --release    # -> ./target/release/openixcli
cargo test               # unit tests (no hardware needed)
cargo clippy             # keep this clean; the codebase has zero warnings
```

Hardware flashing needs a real device in FEL or FES mode on USB. Tests never
touch hardware; pure logic (offset math, GPT/MBR parsing, chunking, sparse) is
factored into testable functions — keep it that way when adding flash logic.

## Architecture

- `src/cli.rs` — clap subcommands; `src/main.rs` routes them.
- `src/commands/` — thin entry points (mmap the image, build options, call into `flash::`).
- `src/flash/` — the core:
  - `device_session` connect/scan; `fel_handler` (DRAM init + u-boot download);
    `fel_bootstrap` (FEL→FES bring-up from a LiveSuit firmware).
  - `fes_handler` — partition/MBR/boot download for the LiveSuit `flash` command.
  - `raw_writer` — generic sector writer + read-back verify (shared by flash-raw/part).
  - `raw_image/` — `flash-raw` (whole-disk image); `partition_flash/` — `flash-part` (one partition into the device GPT).
  - `logic_offset` — FES logical-sector compensation (read this; see below).
- `src/firmware/` — LiveSuit/IMAGEWTY parsing (`OpenixPacker`), sparse, `bootstrap_pack` (mkbootstrap).
- `src/config/` — GPT/MBR/boot-header/sys_config parsers.
- `libefex` is a git dependency (the C FFI to the sunxi FEL/FES protocol).

## Critical domain knowledge

These were hard-won by debugging on a Radxa Cubie A7A (A733 / sun60iw2, eMMC).
Don't relearn them the hard way.

### FEL vs FES
- **FEL** = BROM USB boot (`DeviceMode::Fel`). Can only poke memory / run code.
- **FES** = `DeviceMode::Srv`, the sprite/U-Boot running after bootstrap. This is
  where flash reads/writes happen (`fes_down`/`fes_up`/`fes_flash_set_onoff`).
- To flash from FEL you must first bootstrap into FES. On newer SoCs (A733) boot0
  is a real SPL and u-boot is in a sunxi-package, so you **cannot** slice them out
  of a raw.img — bootstrap from a LiveSuit firmware: `--bootstrap misc/a733_bootstrap.img`
  (a slimmed firmware built by `mkbootstrap`).

### FES logical-sector compensation (the big one) — `src/flash/logic_offset.rs`
FES `Flash`-tagged transfers address a **logical** sector space offset from the
physical media:

```
physical_sector = fes_logical_sector + logic_offset   (mod 2^32, u32 wrapping)
```

`logic_offset = 40960` (20 MiB) for SD/eMMC; `0` for NAND. So to land data at
physical sector `P`, transfer at logical `P - logic_offset` (`wrapping_sub`).

- A full-disk raw.img (physical 0) is written **starting at logical sector
  `0 - 40960 = 0xffff6000`** as a single `fes_down`. libefex chunks into 64 KiB
  pieces and increments the address as `uint32_t` (`libefex/src/efex-fes.c`), so
  it wraps and every image sector N lands at physical N.
- flash-part must compensate the **GPT reads too** (GPT header is physical LBA1 →
  read logical `1 - 40960`), not just the partition write.
- The sign matters and bit us repeatedly: `+40960` and `0` both flash without
  error but **don't boot** (data lands past the LBAs U-Boot reads). Only the
  negative offset boots. This was confirmed by reverse-engineering OpenixSuit
  (its log prints `Partition address: 0xffff6000`).

### Read-back verify caveats — `raw_writer.rs`
- `fes_up` read-back lives in the **same logical space** as the write, so it is
  self-consistent regardless of physical placement — a passing read-back does
  **not** prove the image will boot. Don't trust it to validate addressing.
- FES logical sector 0 (physical == logic_offset) holds the sunxi sprite's
  logical MBR/metadata; the device rewrites it, so our bytes there never
  round-trip. Verify ignores a 16 KiB metadata window there (`in_metadata_window`).
- Device CRC (`fes_verify_value`) can read the write cache → false pass. Flush
  with `fes_flash_set_onoff(off)` before trusting any read-back.

### Misc gotchas
- Query the storage type with `fes_query_storage` (eMMC == 2) and pass it to
  `fes_flash_set_onoff`. Don't hardcode 0 (NAND) — writes fail with USB errors.
- Writes are cached and only hit media on `fes_flash_set_onoff(off)`; "device
  boots" is the real success signal.

## Reverse-engineering OpenixSuit (the reference flasher)
OpenixSuit (Tauri app, same `libefex`) is the known-good GUI flasher; when our
behavior diverges, diff against it. The macOS binary is a single Mach-O with Rust
symbols. Useful: `nm` + `objdump -d --macho` dumps the whole `__text` (start/stop
flags are ignored — dump once, slice by address). `__cstring` vaddr = file offset
+ 0x100000000; objdump annotates string-literal references inline. Key functions:
`flash::task::download::download_partition_from_file`, `flash::task::run::run_flash_task`.

## Conventions
- Develop directly on `master` (owner's preference); commit/push only when asked.
- `docs/`, `target/`, `cargo.lock` are gitignored. Spec/plan docs under
  `docs/superpowers/` are force-added (`git add -f`) when committed.
