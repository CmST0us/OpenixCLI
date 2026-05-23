# raw.img 刷写功能 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 给 OpenixCLI 增加整盘 `raw.img` 刷写（`flash-raw`）与按设备 GPT 刷单分区（`flash-part`）两条新数据路径，不影响现有 IMAGEWTY 刷写。

**Architecture:** 新增两个子命令各对应一个独立 handler，并存于现有 `Flasher` 旁。抽取共享原语：`raw_writer`（分块扇区写+进度+校验，数据源为 `&[u8]`）、`FelBootstrap`（DRAM init→uboot→重连）、`DeviceSession`（USB 连接+模式检测）、`gpt_parser`（GPT 解析）。功能 A 在 FEL 模式时从 raw.img 固定偏移提取 boot0/uboot 引导；功能 B 要求设备已在 FES 模式，从设备读 GPT 后定位分区写入，支持 raw 与 sparse。

**Tech Stack:** Rust 2021, tokio, clap, memmap2, libefex（git 依赖，提供 `fes_down`/`fes_up`/`fes_verify_value`/`fel_*`）。

参考设计文档：`docs/superpowers/specs/2026-05-23-raw-img-flash-design.md`

**关键事实（实现时直接用）：**
- 设备「在 FES」== `libefex::DeviceMode::Srv`；「在 FEL」== `libefex::DeviceMode::Fel`。
- 按扇区写：`ctx.fes_down(buf, sector_u32, FesDataType::Flash)`；按扇区读：`ctx.fes_up(&mut buf, sector_u32, FesDataType::Flash)`。
- 写后校验：`ctx.fes_verify_value(start_sector_u32, len_bytes_u64)`，返回 `FesVerifyResp { flag, media_crc, .. }`；`flag == EFEX_CRC32_VALID_FLAG`（`crate::config::mbr_parser::EFEX_CRC32_VALID_FLAG`）时 `media_crc as u32` 应等于本地 `IncrementalChecksum`。
- 写前/后开关闪存访问：`ctx.fes_flash_set_onoff(0, true/false)`。
- boot0 在 raw.img 字节偏移 `16*512=8192`，长度取 `Boot0Header.length`（`crate::config::boot_header::Boot0Header`，magic `"eGON.BT0"`）。
- u-boot 在 raw.img 字节偏移 `UBOOT_START_SECTOR*512`，可用 `UBootBaseHeader`（magic `"uboot"`）校验。
- 现有 `IncrementalChecksum` 在 `crate::flash::fes_handler::types`（注意：这是简单 32 位累加和，**不是** CRC32；它只用于和设备 `media_crc` 比对，GPT 自身的 CRC 另用真正 CRC-32）。

---

## File Structure

**新建：**
- `src/config/gpt_parser.rs` — GPT 解析 + CRC-32。
- `src/flash/raw_writer.rs` — 通用分块扇区写（数据源 `&[u8]`）。
- `src/flash/device_session.rs` — USB 连接/初始化/模式检测共享样板。
- `src/flash/fel_handler/bootstrap.rs` — 共享 `FelBootstrap`。
- `src/flash/raw_image/mod.rs` — 功能 A：整盘 `flash-raw`。
- `src/flash/raw_image/layout.rs` — 从 raw.img 固定偏移提取 boot0/uboot。
- `src/flash/partition_flash/mod.rs` — 功能 B：`flash-part`。
- `src/flash/partition_flash/gpt_reader.rs` — 经 `fes_up` 从设备读 GPT 原始字节。
- `src/commands/flash_raw.rs` — `flash-raw` 命令入口。
- `src/commands/flash_part.rs` — `flash-part` 命令入口。

**修改：**
- `src/utils/error.rs` — 新增错误变体。
- `src/config/mod.rs` — `pub mod gpt_parser;`
- `src/flash/mod.rs` — 导出新模块。
- `src/flash/fes_handler/mod.rs` — `mod bootstrap; pub use bootstrap::FelBootstrap;`
- `src/flash/fes_handler/partition/sparse_parser.rs` — 把 `download_sparse_from_reader` 提升为可复用（`pub`）。
- `src/flash/fes_handler/partition/mod.rs` — 导出 `SparseDownloader`（若尚未导出）。
- `src/flash/fel_handler/uboot_download.rs` — `sysconfig` 改为可选。
- `src/cli.rs` — 新增 `FlashRaw` / `FlashPart` 子命令。
- `src/commands/mod.rs` — `pub mod flash_raw; pub mod flash_part;`
- `src/main.rs` — 路由新子命令。
- `README.md` — 文档新命令。

---

## Task 1: 新增 FlashError 错误变体

**Files:**
- Modify: `src/utils/error.rs`

- [ ] **Step 1: 在 `FlashError` 枚举内（`Unknown` 变体之前）加入新变体**

```rust
    #[error("Raw image ({image} bytes) is larger than device capacity ({capacity} bytes)")]
    RawImageTooLarge { image: u64, capacity: u64 },

    #[error("Invalid GPT: {0}")]
    GptInvalid(String),

    #[error("Partition not found: {0}")]
    PartitionNotFound(String),

    #[error("Device is not in FES mode (current: {0}); run flash-raw first or boot the device into FES")]
    DeviceNotInFes(String),
```

- [ ] **Step 2: 验证编译**

Run: `cargo build`
Expected: 编译通过（新变体带 `#[allow(dead_code)]` 已在文件顶部，未使用不报错）。

- [ ] **Step 3: Commit**

```bash
git add src/utils/error.rs
git commit -m "feat(error): add error variants for raw.img / GPT flashing"
```

---

## Task 2: GPT 解析模块（CRC-32）

**Files:**
- Create: `src/config/gpt_parser.rs`
- Modify: `src/config/mod.rs`
- Test: 内置 `#[cfg(test)]`

- [ ] **Step 1: 先写失败测试 —— 在 `src/config/gpt_parser.rs` 顶部写文件骨架与测试**

```rust
//! GPT (GUID Partition Table) parser
//!
//! Parses a GPT primary header (LBA1) plus its partition entry array.
//! Used by flash-part to locate a partition on the device by name.

#![allow(dead_code)]

use crate::utils::FlashError;

pub const GPT_SIGNATURE: &[u8; 8] = b"EFI PART";
pub const GPT_HEADER_SIZE: usize = 92;
pub const SECTOR_SIZE: u64 = 512;

/// CRC-32 (IEEE 802.3, reflected, poly 0xEDB88320).
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_known_vector() {
        // Standard CRC-32 of "123456789" is 0xCBF43926.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }
}
```

- [ ] **Step 2: 注册模块并运行测试确认它能编译运行**

在 `src/config/mod.rs` 增加一行 `pub mod gpt_parser;`（与其它 `pub mod` 并列）。

Run: `cargo test --lib gpt_parser::tests::crc32_known_vector`
Expected: PASS（验证 CRC-32 实现正确）。

- [ ] **Step 3: 追加解析结构与失败测试**

在 `crc32` 之后、`#[cfg(test)]` 之前插入：

```rust
/// A single parsed GPT partition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GptPartition {
    pub name: String,
    pub first_lba: u64,
    pub last_lba: u64,
}

impl GptPartition {
    /// Inclusive sector count [first_lba, last_lba].
    pub fn sector_count(&self) -> u64 {
        self.last_lba.saturating_sub(self.first_lba) + 1
    }

    /// Capacity in bytes.
    pub fn size_bytes(&self) -> u64 {
        self.sector_count() * SECTOR_SIZE
    }
}

/// Parsed GPT: header fields needed to read entries + the partitions.
#[derive(Debug, Clone)]
pub struct Gpt {
    pub partition_entries_lba: u64,
    pub num_entries: u32,
    pub entry_size: u32,
    pub partitions: Vec<GptPartition>,
}

impl Gpt {
    /// Find a partition by exact name (case-sensitive).
    pub fn find(&self, name: &str) -> Option<&GptPartition> {
        self.partitions.iter().find(|p| p.name == name)
    }

    /// All partition names, for error messages.
    pub fn names(&self) -> Vec<String> {
        self.partitions.iter().map(|p| p.name.clone()).collect()
    }
}

fn read_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

fn read_u64(b: &[u8], off: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[off..off + 8]);
    u64::from_le_bytes(a)
}

/// Parse the GPT header (LBA1, 512 bytes) without entries.
/// Validates signature and header CRC-32.
pub fn parse_header(lba1: &[u8]) -> Result<Gpt, FlashError> {
    if lba1.len() < GPT_HEADER_SIZE {
        return Err(FlashError::GptInvalid("header buffer too small".into()));
    }
    if &lba1[0..8] != GPT_SIGNATURE {
        return Err(FlashError::GptInvalid("bad signature (expected 'EFI PART')".into()));
    }
    let header_size = read_u32(lba1, 12) as usize;
    if header_size < GPT_HEADER_SIZE || header_size > lba1.len() {
        return Err(FlashError::GptInvalid(format!("bad header_size {}", header_size)));
    }
    let stored_crc = read_u32(lba1, 16);
    let mut hdr = lba1[..header_size].to_vec();
    hdr[16..20].fill(0); // CRC field zeroed for computation
    let calc_crc = crc32(&hdr);
    if calc_crc != stored_crc {
        return Err(FlashError::GptInvalid(format!(
            "header CRC mismatch: stored=0x{:08x} calc=0x{:08x}",
            stored_crc, calc_crc
        )));
    }
    Ok(Gpt {
        partition_entries_lba: read_u64(lba1, 72),
        num_entries: read_u32(lba1, 80),
        entry_size: read_u32(lba1, 84),
        partitions: Vec::new(),
    })
}

/// Parse the partition entry array into `gpt.partitions`.
/// `entries` must contain at least `num_entries * entry_size` bytes.
/// Validates the entries CRC-32 against the value stored in `lba1`.
pub fn parse_entries(gpt: &mut Gpt, lba1: &[u8], entries: &[u8]) -> Result<(), FlashError> {
    let entry_size = gpt.entry_size as usize;
    let num = gpt.num_entries as usize;
    if entry_size < 128 {
        return Err(FlashError::GptInvalid(format!("bad entry_size {}", entry_size)));
    }
    let needed = entry_size * num;
    if entries.len() < needed {
        return Err(FlashError::GptInvalid("entry buffer too small".into()));
    }
    let stored_entries_crc = read_u32(lba1, 88);
    let calc = crc32(&entries[..needed]);
    if calc != stored_entries_crc {
        return Err(FlashError::GptInvalid(format!(
            "entries CRC mismatch: stored=0x{:08x} calc=0x{:08x}",
            stored_entries_crc, calc
        )));
    }
    for i in 0..num {
        let base = i * entry_size;
        let e = &entries[base..base + entry_size];
        // Entry is unused when type GUID is all-zero.
        if e[0..16].iter().all(|&x| x == 0) {
            continue;
        }
        let first_lba = read_u64(e, 32);
        let last_lba = read_u64(e, 40);
        // Name: 72 bytes UTF-16LE (36 code units), trailing zeros trimmed.
        let units: Vec<u16> = (0..36).map(|j| u16::from_le_bytes([e[56 + j * 2], e[57 + j * 2]])).collect();
        let name: String = char::decode_utf16(units.into_iter().take_while(|&u| u != 0))
            .map(|r| r.unwrap_or('\u{FFFD}'))
            .collect();
        gpt.partitions.push(GptPartition { name, first_lba, last_lba });
    }
    Ok(())
}
```

并在 `#[cfg(test)] mod tests` 内追加：

```rust
    // Build a minimal one-entry GPT (header in lba1, 1 entry of 128 bytes) with valid CRCs.
    fn build_gpt(name: &str, first: u64, last: u64) -> (Vec<u8>, Vec<u8>) {
        let entry_size = 128usize;
        let num = 1u32;
        let mut entries = vec![0u8; entry_size];
        entries[0] = 1; // non-zero type GUID
        entries[32..40].copy_from_slice(&first.to_le_bytes());
        entries[40..48].copy_from_slice(&last.to_le_bytes());
        for (j, u) in name.encode_utf16().enumerate() {
            entries[56 + j * 2..58 + j * 2].copy_from_slice(&u.to_le_bytes());
        }
        let entries_crc = crc32(&entries);

        let mut lba1 = vec![0u8; 512];
        lba1[0..8].copy_from_slice(GPT_SIGNATURE);
        lba1[12..16].copy_from_slice(&(GPT_HEADER_SIZE as u32).to_le_bytes());
        lba1[72..80].copy_from_slice(&2u64.to_le_bytes()); // entries at LBA2
        lba1[80..84].copy_from_slice(&num.to_le_bytes());
        lba1[84..88].copy_from_slice(&(entry_size as u32).to_le_bytes());
        lba1[88..92].copy_from_slice(&entries_crc.to_le_bytes());
        let hdr_crc = crc32(&lba1[..GPT_HEADER_SIZE]);
        lba1[16..20].copy_from_slice(&hdr_crc.to_le_bytes());
        (lba1, entries)
    }

    #[test]
    fn parses_partition_by_name() {
        let (lba1, entries) = build_gpt("boot", 40, 1063);
        let mut gpt = parse_header(&lba1).unwrap();
        parse_entries(&mut gpt, &lba1, &entries).unwrap();
        let p = gpt.find("boot").expect("boot present");
        assert_eq!(p.first_lba, 40);
        assert_eq!(p.last_lba, 1063);
        assert_eq!(p.sector_count(), 1024);
        assert_eq!(p.size_bytes(), 1024 * 512);
        assert!(gpt.find("missing").is_none());
        assert_eq!(gpt.names(), vec!["boot".to_string()]);
    }

    #[test]
    fn rejects_bad_signature() {
        let mut lba1 = vec![0u8; 512];
        lba1[0..8].copy_from_slice(b"NOTEFIPT");
        assert!(parse_header(&lba1).is_err());
    }

    #[test]
    fn rejects_bad_header_crc() {
        let (mut lba1, _e) = build_gpt("boot", 40, 1063);
        lba1[16] ^= 0xFF; // corrupt stored CRC
        assert!(parse_header(&lba1).is_err());
    }
```

- [ ] **Step 4: 运行确认全部失败再实现 → 通过**

Run: `cargo test --lib gpt_parser`
Expected: 全部 PASS。

- [ ] **Step 5: Commit**

```bash
git add src/config/gpt_parser.rs src/config/mod.rs
git commit -m "feat(config): add GPT parser with CRC-32 validation"
```

---

## Task 3: 通用分块扇区写原语 raw_writer

**Files:**
- Create: `src/flash/raw_writer.rs`
- Modify: `src/flash/mod.rs`
- Test: 内置 `#[cfg(test)]`

- [ ] **Step 1: 先写纯函数与失败测试 —— 创建 `src/flash/raw_writer.rs`**

```rust
//! Generic raw sector writer
//!
//! Writes an in-memory byte slice to device storage starting at a given
//! sector, in fixed-size chunks, with progress reporting and optional
//! checksum verification. Shared by flash-raw (full image) and flash-part
//! (raw partition images).

#![allow(dead_code)]

use crate::config::mbr_parser::EFEX_CRC32_VALID_FLAG;
use crate::flash::fes_handler::types::IncrementalChecksum;
use crate::utils::{FlashError, FlashResult, Logger};
use libefex::FesDataType;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Bytes per write chunk fed to libefex (libefex further splits into 64 KB USB transfers).
pub const WRITE_CHUNK: u64 = 16 * 1024 * 1024;
const SPEED_UPDATE_INTERVAL: u64 = 64 * 1024;
const SECTOR_SIZE: u64 = 512;

/// Split `total` bytes into `(offset, len)` chunks of at most `chunk` bytes.
pub fn chunk_ranges(total: u64, chunk: u64) -> Vec<(u64, u64)> {
    let mut out = Vec::new();
    let mut off = 0u64;
    while off < total {
        let len = std::cmp::min(chunk, total - off);
        out.push((off, len));
        off += len;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_exact_multiple() {
        assert_eq!(chunk_ranges(20, 10), vec![(0, 10), (10, 10)]);
    }

    #[test]
    fn chunks_with_remainder() {
        assert_eq!(chunk_ranges(25, 10), vec![(0, 10), (10, 10), (20, 5)]);
    }

    #[test]
    fn chunks_empty_and_small() {
        assert_eq!(chunk_ranges(0, 10), Vec::<(u64, u64)>::new());
        assert_eq!(chunk_ranges(7, 10), vec![(0, 7)]);
    }
}
```

- [ ] **Step 2: 注册模块并运行纯函数测试**

在 `src/flash/mod.rs` 的 `pub mod fel_handler;` / `pub mod fes_handler;` 附近加入 `pub mod raw_writer;`。

Run: `cargo test --lib raw_writer::tests`
Expected: 三个测试 PASS。

- [ ] **Step 3: 在 `chunk_ranges` 之后、`#[cfg(test)]` 之前实现 `write_raw`**

```rust
/// Write `data` to the device starting at `start_sector`.
///
/// Chunks are addressed by sector (data is byte-for-byte). When `verify`
/// is true an IncrementalChecksum is accumulated and compared against the
/// device CRC over the whole written range.
pub async fn write_raw(
    ctx: &libefex::Context,
    logger: &Logger,
    data: &[u8],
    start_sector: u32,
    verify: bool,
) -> FlashResult<()> {
    let total = data.len() as u64;
    let written_bytes = Arc::new(AtomicU64::new(0));
    let last_speed = Arc::new(AtomicU64::new(0));
    let mut checksum = if verify { Some(IncrementalChecksum::new()) } else { None };

    for (offset, len) in chunk_ranges(total, WRITE_CHUNK) {
        let chunk = &data[offset as usize..(offset + len) as usize];
        if let Some(ref mut cs) = checksum {
            cs.update(chunk);
        }
        let chunk_start_sector = start_sector.wrapping_add((offset / SECTOR_SIZE) as u32);
        let base = written_bytes.load(Ordering::SeqCst);
        let wb = Arc::clone(&written_bytes);
        let ls = Arc::clone(&last_speed);

        ctx.fes_down_with_progress(chunk, chunk_start_sector, FesDataType::Flash, {
            move |transferred, _total| {
                let current = base + transferred;
                wb.store(current, Ordering::SeqCst);
                let last = ls.load(Ordering::SeqCst);
                if current.saturating_sub(last) >= SPEED_UPDATE_INTERVAL {
                    ls.store(current, Ordering::SeqCst);
                    logger.update_progress_with_speed(current);
                }
            }
        })
        .map_err(|e| FlashError::UsbTransferError(e.to_string()))?;
    }

    if let Some(mut cs) = checksum {
        let local = cs.finalize();
        let resp = ctx
            .fes_verify_value(start_sector, total)
            .map_err(|e| FlashError::UsbTransferError(e.to_string()))?;
        if resp.flag == EFEX_CRC32_VALID_FLAG {
            let device_crc = resp.media_crc as u32;
            if local != device_crc {
                logger.warn(&format!(
                    "Verify mismatch at sector {}: local=0x{:08x} device=0x{:08x}",
                    start_sector, local, device_crc
                ));
            } else {
                logger.info(&format!("Verified {} bytes at sector {}", total, start_sector));
            }
        } else {
            logger.warn(&format!("Verify failed at sector {}: flag=0x{:08x}", start_sector, resp.flag));
        }
    }
    Ok(())
}
```

- [ ] **Step 4: 验证编译**

Run: `cargo build && cargo test --lib raw_writer::tests`
Expected: 编译通过，纯函数测试 PASS。（`write_raw` 需真机，留待集成测试。）

- [ ] **Step 5: Commit**

```bash
git add src/flash/raw_writer.rs src/flash/mod.rs
git commit -m "feat(flash): add generic raw_writer (chunked sector write + verify)"
```

---

## Task 4: 从 raw.img 提取 boot0/uboot（layout）

**Files:**
- Create: `src/flash/raw_image/layout.rs`
- Test: 内置 `#[cfg(test)]`

(本任务只建文件与单元测试，模块在 Task 6 一并注册。)

- [ ] **Step 1: 创建 `src/flash/raw_image/layout.rs` 含失败测试**

```rust
//! Locate boot0 / u-boot inside a full-disk raw.img at standard sunxi offsets.

#![allow(dead_code)]

use crate::config::boot_header::{Boot0Header, UBootBaseHeader, BOOT0_MAGIC, UBOOT_MAGIC};
use crate::utils::FlashError;

/// boot0 sits at sector 16 (8 KiB) in the SD/eMMC sunxi layout.
pub const BOOT0_OFFSET: usize = 16 * 512;
/// u-boot / toc1 standard sunxi start sector. NOTE: verify against target SoC;
/// overridable via the hidden --uboot-offset flag (see flash_raw command).
pub const UBOOT_START_SECTOR: usize = 32800;

/// Extracted bootstrap blobs borrowed from the raw image.
pub struct RawBootstrap<'a> {
    pub boot0: &'a [u8],
    pub uboot: &'a [u8],
}

/// Slice boot0 and u-boot out of `img`. `uboot_sector` lets callers override
/// the default UBOOT_START_SECTOR.
pub fn extract_bootstrap(img: &[u8], uboot_sector: usize) -> Result<RawBootstrap<'_>, FlashError> {
    // boot0
    if img.len() < BOOT0_OFFSET + std::mem::size_of::<Boot0Header>() {
        return Err(FlashError::InvalidFirmwareFormat("image too small for boot0".into()));
    }
    let boot0_hdr = Boot0Header::parse(&img[BOOT0_OFFSET..])
        .map_err(|e| FlashError::InvalidFirmwareFormat(e.to_string()))?;
    if !boot0_hdr.magic_str().starts_with(BOOT0_MAGIC) {
        return Err(FlashError::InvalidFirmwareFormat(format!(
            "no '{}' magic at boot0 offset 0x{:x}", BOOT0_MAGIC, BOOT0_OFFSET
        )));
    }
    let boot0_len = { let l = boot0_hdr.length; l as usize };
    let boot0_end = BOOT0_OFFSET + boot0_len;
    if boot0_len == 0 || boot0_end > img.len() {
        return Err(FlashError::InvalidFirmwareFormat(format!("bad boot0 length {}", boot0_len)));
    }
    let boot0 = &img[BOOT0_OFFSET..boot0_end];

    // u-boot
    let uboot_off = uboot_sector * 512;
    if img.len() < uboot_off + std::mem::size_of::<UBootBaseHeader>() {
        return Err(FlashError::InvalidFirmwareFormat("image too small for u-boot".into()));
    }
    let uboot_hdr = UBootBaseHeader::parse(&img[uboot_off..])
        .map_err(|e| FlashError::InvalidFirmwareFormat(e.to_string()))?;
    if !uboot_hdr.magic_str().starts_with(UBOOT_MAGIC) {
        return Err(FlashError::InvalidFirmwareFormat(format!(
            "no '{}' magic at u-boot sector {} (offset 0x{:x}); try --uboot-offset",
            UBOOT_MAGIC, uboot_sector, uboot_off
        )));
    }
    let uboot_len = { let l = uboot_hdr.length; l as usize };
    let uboot_end = uboot_off + uboot_len;
    if uboot_len == 0 || uboot_end > img.len() {
        return Err(FlashError::InvalidFirmwareFormat(format!("bad u-boot length {}", uboot_len)));
    }
    let uboot = &img[uboot_off..uboot_end];

    Ok(RawBootstrap { boot0, uboot })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(buf: &mut [u8], off: usize, bytes: &[u8]) {
        buf[off..off + bytes.len()].copy_from_slice(bytes);
    }

    #[test]
    fn extracts_boot0_and_uboot() {
        let uboot_sector = 64usize; // small offset for the test image
        let mut img = vec![0u8; uboot_sector * 512 + 4096];
        // boot0 header: jump(4) + magic(8) + check_sum(4) + length(4)@offset16
        put(&mut img, BOOT0_OFFSET + 4, BOOT0_MAGIC.as_bytes());
        put(&mut img, BOOT0_OFFSET + 16, &1024u32.to_le_bytes());
        // uboot base header: jump(4) + magic(8) + check_sum(4) + align(4) + length(4)@offset20
        let uoff = uboot_sector * 512;
        put(&mut img, uoff + 4, UBOOT_MAGIC.as_bytes());
        put(&mut img, uoff + 20, &2048u32.to_le_bytes());

        let bs = extract_bootstrap(&img, uboot_sector).unwrap();
        assert_eq!(bs.boot0.len(), 1024);
        assert_eq!(bs.uboot.len(), 2048);
    }

    #[test]
    fn errors_when_uboot_magic_missing() {
        let uboot_sector = 64usize;
        let mut img = vec![0u8; uboot_sector * 512 + 4096];
        put(&mut img, BOOT0_OFFSET + 4, BOOT0_MAGIC.as_bytes());
        put(&mut img, BOOT0_OFFSET + 16, &1024u32.to_le_bytes());
        // no uboot magic written
        assert!(extract_bootstrap(&img, uboot_sector).is_err());
    }
}
```

注：`UBootBaseHeader` 字段顺序为 `jump_instruction(4), magic[8], check_sum(4), align_size(4), length(4), ...`，故 `length` 在偏移 20；`Boot0Header` 的 `length` 在偏移 16。测试按此布局填充。

- [ ] **Step 2: 临时注册模块以便测试（Task 6 会建 `raw_image/mod.rs` 正式注册）**

在 `src/flash/mod.rs` 暂加 `pub mod raw_image { pub mod layout; }`，或直接在本任务先创建 `src/flash/raw_image/mod.rs` 内容为 `pub mod layout;` 并在 `src/flash/mod.rs` 加 `pub mod raw_image;`（推荐后者，Task 6 续写同文件）。

Run: `cargo test --lib raw_image::layout`
Expected: 两个测试 PASS。

- [ ] **Step 3: Commit**

```bash
git add src/flash/raw_image/layout.rs src/flash/raw_image/mod.rs src/flash/mod.rs
git commit -m "feat(flash): extract boot0/uboot from raw.img at fixed sunxi offsets"
```

---

## Task 5: u-boot 下载支持可选 sys_config

**Files:**
- Modify: `src/flash/fel_handler/uboot_download.rs`

- [ ] **Step 1: 把 `execute` 的 `sysconfig_data: &[u8]` 改为 `Option<&[u8]>`**

将签名改为：

```rust
    pub async fn execute(
        &self,
        ctx: &libefex::Context,
        uboot_data: &[u8],
        dtb_data: Option<&[u8]>,
        sysconfig_data: Option<&[u8]>,
        board_config_data: Option<&[u8]>,
    ) -> FlashResult<()> {
```

并把对 `write_sysconfig` 的调用改为：

```rust
        self.write_dtb(ctx, run_addr, dtb_data)?;
        self.write_sysconfig(ctx, run_addr, sysconfig_data)?;
        self.write_board_config(ctx, run_addr, board_config_data)?;
```

`write_sysconfig` 改为接受 `Option`：

```rust
    fn write_sysconfig(
        &self,
        ctx: &libefex::Context,
        run_addr: u32,
        sysconfig_data: Option<&[u8]>,
    ) -> FlashResult<()> {
        let Some(sysconfig_data) = sysconfig_data else {
            return Ok(());
        };
        let dtb_sysconfig_base = run_addr + UBOOT_MAX_LEN as u32;
        let sys_config_bin_base = dtb_sysconfig_base + DTB_MAX_LEN as u32;
        ctx.fel_write(sys_config_bin_base, sysconfig_data)
            .map_err(|e| FlashError::UsbTransferError(e.to_string()))?;
        self.logger.debug(&format!(
            "SysConfig written to 0x{:x} ({} bytes)",
            sys_config_bin_base,
            sysconfig_data.len()
        ));
        Ok(())
    }
```

- [ ] **Step 2: 更新现有唯一调用点 `src/flash/mod.rs` 的 FEL 分支**

把 `Flasher::execute` 里调用 `download_uboot`/`UbootDownload::execute` 处的 `&sysconfig_data` 改为 `Some(&sysconfig_data)`（保持现有 IMAGEWTY 行为不变）。定位：`src/flash/mod.rs:158-166` 的 `fel_handler.download_uboot(...)`，以及 `src/flash/fel_handler/mod.rs` 中转发 `sysconfig_data` 的方法签名同步改为传 `Option`。

> 说明：`FelHandler::download_uboot` 当前以 `&sysconfig` 传入；将其形参类型保持 `&[u8]` 并在内部以 `Some(sysconfig)` 调用 `UbootDownload::execute`，可把改动面缩到最小，无需改 `Flasher` 调用点。二选一即可，关键是编译通过且现有行为等价。

- [ ] **Step 3: 验证编译**

Run: `cargo build`
Expected: 通过。

- [ ] **Step 4: Commit**

```bash
git add src/flash/fel_handler/uboot_download.rs src/flash/fel_handler/mod.rs src/flash/mod.rs
git commit -m "refactor(fel): make sys_config optional in u-boot download"
```

---

## Task 6: 共享 FelBootstrap

**Files:**
- Create: `src/flash/fel_handler/bootstrap.rs`
- Modify: `src/flash/fel_handler/mod.rs`

- [ ] **Step 1: 创建 `src/flash/fel_handler/bootstrap.rs`**

封装「DRAM init（boot0/fes 当 blob）→ u-boot 下载并执行」，供 IMAGEWTY 路径与 raw 路径共用。重连逻辑保留在 `Flasher`（已存在），本模块只负责把设备从 FEL 推进到「u-boot 已执行」。

```rust
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
```

- [ ] **Step 2: 注册模块**

确认 `src/flash/fel_handler/mod.rs` 已 `pub use` 了 `DramInit` 与 `UbootDownload`（若未公开则补 `pub use dram_init::DramInit; pub use uboot_download::UbootDownload;`），并新增：

```rust
mod bootstrap;
pub use bootstrap::FelBootstrap;
```

- [ ] **Step 3: 验证编译**

Run: `cargo build`
Expected: 通过。

- [ ] **Step 4: Commit**

```bash
git add src/flash/fel_handler/bootstrap.rs src/flash/fel_handler/mod.rs
git commit -m "feat(fel): add shared FelBootstrap orchestrator"
```

---

## Task 7: DeviceSession 共享连接/模式检测

**Files:**
- Create: `src/flash/device_session.rs`
- Modify: `src/flash/mod.rs`

- [ ] **Step 1: 创建 `src/flash/device_session.rs`**

抽出现有 `Flasher::execute` 开头的设备扫描/打开/初始化逻辑（`src/flash/mod.rs:90-117`）成可复用函数，供 `flash-raw`/`flash-part` 使用。

```rust
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
```

- [ ] **Step 2: 注册模块**

在 `src/flash/mod.rs` 加 `pub mod device_session;`。

- [ ] **Step 3: 验证编译**

Run: `cargo build`
Expected: 通过。

- [ ] **Step 4: Commit**

```bash
git add src/flash/device_session.rs src/flash/mod.rs
git commit -m "feat(flash): add DeviceSession connect helper"
```

---

## Task 8: 暴露 sparse reader 核心供复用

**Files:**
- Modify: `src/flash/fes_handler/partition/sparse_parser.rs`
- Modify: `src/flash/fes_handler/partition/mod.rs`

- [ ] **Step 1: 把 `download_sparse_from_reader` 与其参数结构体公开**

将 `src/flash/fes_handler/partition/sparse_parser.rs` 中：
- `async fn download_sparse_from_reader<R: Read + Seek>` 改为 `pub async fn download_sparse_from_reader<R: Read + Seek>`。
- 其参数类型 `SparseDownloadParams`（同文件内定义）及其字段改为 `pub`（结构体与字段都加 `pub`）。
- 确认 `SparseDownloader::new` 已是 `pub`（现状即是）。

- [ ] **Step 2: 在 partition 模块导出 SparseDownloader 与参数**

在 `src/flash/fes_handler/partition/mod.rs` 增加：

```rust
pub use sparse_parser::{SparseDownloadParams, SparseDownloader};
```

并确认 `src/flash/fes_handler/mod.rs` 通过 `pub use partition::...` 能让外部访问（如需，补 `pub use partition::{SparseDownloader, SparseDownloadParams};`）。

- [ ] **Step 3: 验证编译**

Run: `cargo build`
Expected: 通过（无行为变化，仅可见性）。

- [ ] **Step 4: Commit**

```bash
git add src/flash/fes_handler/partition/sparse_parser.rs src/flash/fes_handler/partition/mod.rs src/flash/fes_handler/mod.rs
git commit -m "refactor(sparse): expose reader-based sparse downloader for reuse"
```

---

## Task 9: 从设备读取 GPT

**Files:**
- Create: `src/flash/partition_flash/gpt_reader.rs`

(模块在 Task 11 随 `partition_flash/mod.rs` 注册；本任务先建文件，编译通过即可。)

- [ ] **Step 1: 创建 `src/flash/partition_flash/gpt_reader.rs`**

```rust
//! Read the primary GPT (header + entries) from a device in FES mode.

use crate::config::gpt_parser::{self, Gpt};
use crate::utils::{FlashError, FlashResult, Logger};
use libefex::FesDataType;

const SECTOR_SIZE: usize = 512;

/// Read sectors [start_sector, start_sector + count) from the device.
fn read_sectors(ctx: &libefex::Context, start_sector: u32, count: usize) -> FlashResult<Vec<u8>> {
    let mut buf = vec![0u8; count * SECTOR_SIZE];
    ctx.fes_up(&mut buf, start_sector, FesDataType::Flash)
        .map_err(|e| FlashError::UsbTransferError(e.to_string()))?;
    Ok(buf)
}

/// Read and parse the device's primary GPT.
pub fn read_gpt(ctx: &libefex::Context, logger: &Logger) -> FlashResult<Gpt> {
    // LBA1: GPT header.
    let lba1 = read_sectors(ctx, 1, 1)?;
    let mut gpt = gpt_parser::parse_header(&lba1)?;

    let entries_bytes = (gpt.num_entries as usize) * (gpt.entry_size as usize);
    let entry_sectors = entries_bytes.div_ceil(SECTOR_SIZE);
    let entries = read_sectors(ctx, gpt.partition_entries_lba as u32, entry_sectors)?;
    gpt_parser::parse_entries(&mut gpt, &lba1, &entries)?;

    logger.info(&format!("Device GPT: {} partitions", gpt.partitions.len()));
    Ok(gpt)
}
```

- [ ] **Step 2: 临时注册以编译**

在 `src/flash/mod.rs` 加 `pub mod partition_flash;`，并创建 `src/flash/partition_flash/mod.rs` 内容暂为 `pub mod gpt_reader;`（Task 11 续写）。

Run: `cargo build`
Expected: 通过。

- [ ] **Step 3: Commit**

```bash
git add src/flash/partition_flash/gpt_reader.rs src/flash/partition_flash/mod.rs src/flash/mod.rs
git commit -m "feat(flash): read device GPT via fes_up"
```

---

## Task 10: RawImageFlasher（功能 A 编排）

**Files:**
- Modify: `src/flash/raw_image/mod.rs`

- [ ] **Step 1: 在 `src/flash/raw_image/mod.rs` 写入编排逻辑**

在已有的 `pub mod layout;` 之后追加（`reconnect_fes` / `set_post_action` 在 Step 2 于同文件定义）：

```rust
//! flash-raw: write a whole raw.img to the device verbatim from sector 0.

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
        // boot0/uboot are borrowed from img; copy to owned buffers to satisfy
        // any mutation (work_mode) done inside the bootstrap.
        let boot0 = bs.boot0.to_vec();
        let uboot = bs.uboot.to_vec();
        FelBootstrap::new(logger)
            .run(&mut ctx, &boot0, &uboot, None, None, None)
            .await?;
        ctx = reconnect_fes(logger, opts.bus, opts.port).await?;
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
```

> 实现注记：`reconnect_fes` 与 `set_post_action` 是本模块内的自包含函数（Step 2 给出完整实现），逻辑等价于现有 `Flasher::reconnect_device`（`src/flash/mod.rs:196-244`）与 `Flasher::set_device_mode`（`src/flash/mod.rs:247-259`）。这里选择在本模块内实现而非把 `Flasher` 私有方法公开，以保持新路径自包含、不触碰现有路径。

- [ ] **Step 2: 在同文件追加自包含的 `reconnect_fes` 与 `set_post_action`**

```rust
async fn reconnect_fes(
    logger: &Logger,
    _bus: Option<u8>,
    _port: Option<u8>,
) -> FlashResult<libefex::Context> {
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
```

- [ ] **Step 3: 验证编译**

Run: `cargo build && cargo clippy`
Expected: 通过。（真机行为留待集成测试。）

- [ ] **Step 4: Commit**

```bash
git add src/flash/raw_image/mod.rs
git commit -m "feat(flash): add RawImageFlasher (flash-raw orchestration)"
```

---

## Task 11: PartitionFlasher（功能 B 编排）

**Files:**
- Modify: `src/flash/partition_flash/mod.rs`

- [ ] **Step 1: 在 `src/flash/partition_flash/mod.rs` 写入编排逻辑**

在已有的 `pub mod gpt_reader;` 之后追加：

```rust
//! flash-part: flash one partition image into the device's existing GPT layout.

use crate::firmware::sparse::{is_sparse_format, SPARSE_HEADER_SIZE};
use crate::flash::device_session;
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
    pub post_action: String, // "none" | "reboot" | "poweroff"
}

/// Flash `img` (raw or sparse) into the named partition read from the device GPT.
pub async fn flash_partition(
    logger: &Logger,
    partition_name: &str,
    img: &[u8],
    opts: &PartitionFlashOptions,
) -> FlashResult<()> {
    let (ctx, mode) = device_session::connect(logger, opts.bus, opts.port)?;
    if mode != libefex::DeviceMode::Srv {
        return Err(FlashError::DeviceNotInFes(format!("{:?}", mode)));
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

    ctx.fes_flash_set_onoff(0, true)
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
            img.len(), partition_name, part.first_lba
        ));
        raw_writer::write_raw(&ctx, logger, img, part.first_lba as u32, opts.verify).await
    };

    let _ = ctx.fes_flash_set_onoff(0, false);
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
```

> 注：`SparseDownloadParams.partition_name` 字段类型为 `&str`（见 sparse_parser），故直接传 `partition_name`。若该结构体字段命名/生命周期与此不符，以 sparse_parser.rs 中的实际定义为准微调。

- [ ] **Step 2: 验证编译**

Run: `cargo build && cargo clippy`
Expected: 通过。

- [ ] **Step 3: Commit**

```bash
git add src/flash/partition_flash/mod.rs
git commit -m "feat(flash): add PartitionFlasher (flash-part: raw + sparse)"
```

---

## Task 12: CLI 子命令与命令入口

**Files:**
- Modify: `src/cli.rs`
- Create: `src/commands/flash_raw.rs`
- Create: `src/commands/flash_part.rs`
- Modify: `src/commands/mod.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: 在 `src/cli.rs` 的 `Commands` 枚举新增两个子命令**

在 `Tui` 之前插入：

```rust
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
```

- [ ] **Step 2: 创建 `src/commands/flash_raw.rs`**

```rust
//! flash-raw command entry.

use crate::flash::raw_image::{flash_raw_image, RawImageOptions};
use crate::flash::raw_image::layout::UBOOT_START_SECTOR;
use crate::utils::logger::Logger;
use memmap2::Mmap;
use std::fs::File;

#[allow(clippy::too_many_arguments)]
pub async fn execute(
    image: String,
    bus: Option<u8>,
    port: Option<u8>,
    verify: bool,
    post_action: String,
    uboot_offset: Option<usize>,
    verbose: bool,
) -> anyhow::Result<()> {
    let logger = Logger::with_verbose(verbose);
    let path = std::path::Path::new(&image);
    if !path.exists() {
        return Err(anyhow::anyhow!("Raw image not found: {}", image));
    }
    let file = File::open(path)?;
    let mmap = unsafe { Mmap::map(&file)? };
    logger.info(&format!("Loaded raw image: {} ({} bytes)", image, mmap.len()));

    let opts = RawImageOptions {
        bus,
        port,
        verify,
        post_action,
        uboot_sector: uboot_offset.unwrap_or(UBOOT_START_SECTOR),
    };
    if let Err(e) = flash_raw_image(&logger, &mmap, &opts).await {
        logger.error(&format!("flash-raw failed: {}", e));
        return Err(anyhow::anyhow!("{}", e));
    }
    Ok(())
}
```

- [ ] **Step 3: 创建 `src/commands/flash_part.rs`**

```rust
//! flash-part command entry.

use crate::flash::partition_flash::{flash_partition, PartitionFlashOptions};
use crate::utils::logger::Logger;
use memmap2::Mmap;
use std::fs::File;

pub async fn execute(
    partition: String,
    image: String,
    bus: Option<u8>,
    port: Option<u8>,
    verify: bool,
    post_action: String,
    verbose: bool,
) -> anyhow::Result<()> {
    let logger = Logger::with_verbose(verbose);
    let path = std::path::Path::new(&image);
    if !path.exists() {
        return Err(anyhow::anyhow!("Partition image not found: {}", image));
    }
    let file = File::open(path)?;
    let mmap = unsafe { Mmap::map(&file)? };
    logger.info(&format!("Loaded partition image: {} ({} bytes)", image, mmap.len()));

    let opts = PartitionFlashOptions { bus, port, verify, post_action };
    if let Err(e) = flash_partition(&logger, &partition, &mmap, &opts).await {
        logger.error(&format!("flash-part failed: {}", e));
        return Err(anyhow::anyhow!("{}", e));
    }
    Ok(())
}
```

- [ ] **Step 4: 注册命令模块**

在 `src/commands/mod.rs` 增加：

```rust
pub mod flash_part;
pub mod flash_raw;
```

并确认 `src/flash/raw_image` 与 `partition_flash` 的公开项可达：在 `src/flash/raw_image/mod.rs` 顶部 `pub mod layout;` 已在 Task 4 加入；`flash_raw_image`/`RawImageOptions` 为本模块 `pub`。

- [ ] **Step 5: 在 `src/main.rs` 路由新子命令**

在 `match cli.command { ... }` 内 `Some(Commands::Flash { .. }) => { .. }` 之后添加：

```rust
        Some(Commands::FlashRaw { image, bus, port, verify, post_action, uboot_offset }) => {
            setup_logging(cli.verbose);
            commands::flash_raw::execute(image, bus, port, verify, post_action, uboot_offset, cli.verbose).await?;
        }
        Some(Commands::FlashPart { partition, image, bus, port, verify, post_action }) => {
            setup_logging(cli.verbose);
            commands::flash_part::execute(partition, image, bus, port, verify, post_action, cli.verbose).await?;
        }
```

- [ ] **Step 6: 验证编译与 CLI**

Run: `cargo build && ./target/debug/openixcli flash-raw --help && ./target/debug/openixcli flash-part --help`
Expected: 编译通过；两条 `--help` 正确显示参数（`flash-raw` 默认 `post-action=reboot`，`flash-part` 默认 `none`，`--uboot-offset` 因 `hide` 不显示）。

- [ ] **Step 7: Commit**

```bash
git add src/cli.rs src/commands/flash_raw.rs src/commands/flash_part.rs src/commands/mod.rs src/main.rs
git commit -m "feat(cli): wire flash-raw and flash-part subcommands"
```

---

## Task 13: 收尾——现有 Flasher 复用 DeviceSession（可选，低风险化）

**Files:**
- Modify: `src/flash/mod.rs`

> 此任务为可选清理。若担心影响现有可用路径，可跳过——`Flasher` 维持原状即可正常工作，新代码已通过 `device_session` 共享逻辑，不存在功能缺口，仅有少量重复。

- [ ] **Step 1: 用 `device_session::connect` 替换 `Flasher::execute` 开头的扫描/初始化块**

将 `src/flash/mod.rs:90-117` 的设备扫描+`usb_init`+`efex_init`+`get_device_mode` 替换为：

```rust
        let (mut ctx, mode) = crate::flash::device_session::connect(
            &self.logger, self.options.bus, self.options.port,
        )?;
        let has_fel = mode == libefex::DeviceMode::Fel;
```

并删除其后重复的 `let mode = ctx.get_device_mode();` 与 `let has_fel = ...;`（保持后续逻辑不变）。

- [ ] **Step 2: 验证编译**

Run: `cargo build && cargo clippy`
Expected: 通过；现有 IMAGEWTY 刷写逻辑行为等价。

- [ ] **Step 3: Commit**

```bash
git add src/flash/mod.rs
git commit -m "refactor(flash): reuse DeviceSession in existing Flasher"
```

---

## Task 14: 文档更新

**Files:**
- Modify: `README.md`

- [ ] **Step 1: 在 README 的 Usage 部分新增两节**

在「Flash Firmware」一节之后加入：

````markdown
### Flash a Raw Disk Image

Write a whole-disk `raw.img` (containing the GPT partition table, boot0 and
toc1/u-boot) to the device verbatim from sector 0:

```bash
openixcli flash-raw raw.img [--bus B --port P] [--verify] [-a reboot|poweroff]
```

If the device is in FES mode the image is written directly. If the device is in
FEL mode, OpenixCLI bootstraps it by extracting boot0 and u-boot from the image
at the standard sunxi offsets (boot0 at sector 16, u-boot at the configured
start sector). FEL bootstrap is best-effort and SoC-dependent; if it fails,
bring the device into FES mode first.

### Flash a Single Partition

Flash one partition image (raw or Android sparse) into the device's existing
GPT layout. The device must already be in FES mode with a valid GPT:

```bash
openixcli flash-part <partition_name> <partition.img> [--bus B --port P] [--verify]
```

The GPT is read back from the device to locate the partition; if the name is not
found, all available partition names are listed.
````

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "docs: document flash-raw and flash-part commands"
```

---

## 集成测试（需真机，文档化执行）

自动化测试无法覆盖 USB/FES 交互。在有目标硬件时按下列清单手动验证：

- [ ] **FES 直写**：设备进入 FES 后 `openixcli flash-raw raw.img`，确认进度、verify 通过、重启后系统正常。
- [ ] **FEL 引导**：设备处于 FEL，运行 `flash-raw`，确认能从镜像 boot0/uboot 引导进 FES 并完成整盘写入；失败时按提示用 `--uboot-offset <sector>` 重试。
- [ ] **flash-part raw**：对已有 GPT 的设备 `openixcli flash-part boot boot.img`，确认按 GPT 定位正确、verify 通过。
- [ ] **flash-part sparse**：用 Android sparse 镜像重复上一步。
- [ ] **错误路径**：分区名不存在时列出可用分区；镜像超容量时报 `RawImageTooLarge`；设备在 FEL 时 `flash-part` 报 `DeviceNotInFes`。

---

## Self-Review 记录

- **Spec 覆盖**：CLI/命令→Task 12；模块结构→各任务；功能 A 数据流→Task 4/6/7/10；功能 B 数据流→Task 8/9/11；FEL 引导→Task 5/6/10 + layout（Task 4）；错误处理与校验→Task 1 + 各 verify；测试→Task 2/3/4 单测 + 集成清单。无遗漏。
- **占位符**：无。Task 10 的 `reconnect_fes`/`set_post_action` 在 Step 2 给出完整实现；所有 `use` 列表自首次出现即正确。
- **类型一致性**：`write_raw(ctx, logger, &[u8], start_sector: u32, verify: bool)` 在 Task 3 定义，Task 10/11 调用一致；`extract_bootstrap(img, uboot_sector)`（Task 4）与 Task 10 调用一致；`gpt_parser::{parse_header, parse_entries, Gpt::find/names}`（Task 2）与 Task 9 一致；`FelBootstrap::run` 的 `Option` 参数与 Task 5 改造后的 `UbootDownload::execute` 一致；`SparseDownloadParams`/`SparseDownloader`（Task 8 公开）与 Task 11 一致。
