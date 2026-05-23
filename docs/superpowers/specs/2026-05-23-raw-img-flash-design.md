# raw.img 刷写功能设计

- 日期：2026-05-23
- 状态：已批准设计，待编写实现计划
- 范围：为 OpenixCLI 增加「整盘 raw.img 刷写」与「按设备 GPT 刷单个分区」两个能力

## 背景

当前 OpenixCLI 只能刷写 Allwinner 私有的 `IMAGEWTY`（LiveSuit/.img）容器格式。其刷写流程（`src/flash/mod.rs` 的 `Flasher`）为：

1. 从固件取出 FES + U-Boot，通过 FEL 推到内存引导设备进入 FES 模式
2. 查询设备（secure / storage_type / flash_size）→ 擦除
3. 写 Allwinner 私有 MBR（`softw411` 格式）→ 逐分区写入 → 写 boot0/boot1

本设计新增的是一条**完全不同的数据路径**：直接写一个全盘 `raw.img`（从扇区 0 起 byte-for-byte，类似 `dd if=raw.img of=/dev/mmcblk0`），镜像内部已含 GPT 分区表、boot0、toc1。

底层 `libefex` 已具备所需原语：

- `fes_down(buf, sector, FesDataType::Flash)`：按扇区号写
- `fes_up(buf, sector, FesDataType::Flash)`：按扇区号读
- `fes_verify_value(sector, len_bytes)` + `IncrementalChecksum`：写后校验

## 目标

- **功能 A — `flash-raw`**：把整盘 `raw.img` 从扇区 0 起逐块原样写入设备。
- **功能 B — `flash-part`**：从设备读取已有 GPT，定位某个分区，把一个分区 `.img`（raw 或 sparse）写到该分区。

## 非目标（YAGNI）

- 不为整盘 `flash-raw` 支持 sparse 全盘镜像（误传时明确报错）。
- 不实现 GPT 的写入/重建（功能 B 只读设备已有 GPT）。
- 不改动现有 IMAGEWTY 刷写路径的行为。
- 不支持 NAND/SPI-NAND 的 UBI 布局（raw.img 模型面向 SD/eMMC 块设备）。

## 总体方案

采用「两个独立 handler + 两个新子命令，复用底层原语」的方案：新路径与现有 `Flasher` 并存，互不影响；通过抽取共享原语把重复降到最低。

被否决的备选：

- 把 `Flasher` 泛化成 `FlashSource` trait —— 两条流程本质不同（结构化分区表 + sunxi MBR + boot0 特殊 tag 对比 整盘 verbatim dd），强行统一会造成抽象泄漏并危及现有可用路径。
- 仅做最小 verbatim 写 —— 不满足已确认的 FEL 引导 + flash-part 需求。

## CLI 与命令

在 `src/cli.rs` 的 `Commands` 枚举新增两个子命令，并在 `src/commands/` 各加一个执行模块。

```
openixcli flash-raw  <raw.img>        [--bus B --port P] [--verify] [--post-action reboot|poweroff]
openixcli flash-part <分区名> <part.img> [--bus B --port P] [--verify] [--post-action reboot|poweroff|none]
```

- `flash-raw`
  - 参数：`image`（raw.img 路径，必填）；`bus`/`port`（可选）；`verify`（默认 `true`，沿用现有语义）；`post_action`（默认 `reboot`）。
  - 不提供 `mode`/`partitions`：整盘 verbatim 写，无「分区子集」概念。
- `flash-part`
  - 参数：`partition`（分区名，必填，对应设备 GPT 中的分区名）；`image`（分区 img 路径，必填）；`bus`/`port`/`verify` 同上；`post_action` **默认 `none`**（刷单分区通常希望继续刷别的或自行控制重启）。
- 两个命令都复用全局 `-v/--verbose` 与 `Logger`。

## 模块结构

```
src/commands/
  flash_raw.rs       ← 新：flash-raw 命令入口（仿 flash.rs）
  flash_part.rs      ← 新：flash-part 命令入口
src/config/
  gpt_parser.rs      ← 新：GPT 解析（"EFI PART" 头 + 分区项数组 → 分区名/起始LBA/大小）
src/flash/
  device_session.rs  ← 新：抽取「扫描/打开 USB + usb_init + efex_init + 模式检测」共享样板
  raw_writer.rs      ← 新：通用「分块扇区写 + 进度 + 校验」，数据源是文件/字节流
  raw_image/         ← 新：功能 A 整盘刷写 handler（含从固定偏移提取 boot0/uboot）
  partition_flash/   ← 新：功能 B 单分区刷写 handler（读设备 GPT → 定位 → 写）
  fel_handler/
    bootstrap.rs     ← 新：抽出 FelBootstrap(dram_init_blob, uboot_blob, 可选 sys_config)
```

设计约束与边界：

- `raw_writer.rs` 把现有 `RawDownloader`（`partition/raw_download.rs`）里的「分块写 + 进度 + `IncrementalChecksum` + `fes_verify_value`」泛化出来，数据源从 `OpenixPacker` 改为通用字节源（文件 reader / 字节切片）。**现有 `RawDownloader` 保持不动**，只有新代码使用新原语，避免触碰现有可用路径。
- `FelBootstrap` 收拢现在散在 `Flasher::execute` 与 `FelHandler` 里的「DRAM init → uboot → 重连」，IMAGEWTY 路径与 raw 路径都调用它，仅数据来源不同。
- sparse 复用：现有 `download_sparse_from_reader` 已基于 `Read + Seek` reader，只有「从 packer 取数据」前缀绑死 packer。flash-part 把分区 img 喂成 reader，直接复用该核心。

## 数据流 · 功能 A（flash-raw）

```
1. 加载 raw.img（memmap2 映射，避免整盘读入内存）
2. DeviceSession：扫描/打开设备 → usb_init → efex_init → 检测模式
3. 若设备在 FEL 模式：
     a. 从 raw.img 固定偏移提取 boot0（sector 16 / 8 KiB 起，按 eGON.BT0 头 length 读取）
     b. 从固定偏移提取 toc1/uboot
     c. FelBootstrap：boot0 作 DRAM-init blob → uboot 设 USB_PRODUCT 启动 → 重连为 FES
   若设备已在 FES 模式：跳过引导，直接进入下一步
4. FES 查询：secure / storage_type / flash_size（复用 fes_query_*）
5. 容量校验：raw.img 大小 ≤ flash_size，否则报错中止
6. RawSectorWriter：从 sector 0 起，按 BUFFER 分块把整个 raw.img verbatim 写入
     （FesDataType::Flash，sector 寻址，不跳零块，带进度与速度显示）
7. verify=true：边写边累计 IncrementalChecksum，写完用 fes_verify_value 校验
8. post_action：reboot/poweroff（复用现有 set_device_mode）
```

要点：

- **不跳零块**：整盘完整写一遍。
- boot0/toc1 不做特殊 `FesDataType::Boot0` 处理——它们已在镜像正确偏移上，随整盘 verbatim 写自然落位（raw/dd 模型）。
- 采用 `BUFFER_SIZE`(256 KB) 级流式读写 + 进度，而非一次性 256 MB chunk。

## 数据流 · 功能 B（flash-part）

```
1. 加载分区 img（memmap2）
2. DeviceSession：连接 + 检测模式
3. 前提校验：设备必须在 FES 模式。若在 FEL → 明确报错
     （提示：请先 flash-raw 刷整盘，或让设备进入 FES）
4. 从设备读 GPT：
     fes_up 读 LBA1（GPT 头, "EFI PART"）+ 分区项数组（默认 LBA2~33, 128 项 × 128 B）
5. gpt_parser 解析：分区名(UTF-16LE) → first_lba / last_lba / 大小
6. 按命令行 <分区名> 查找：
     - 找不到 → 报错并列出设备上所有可用分区名
     - 分区 img 大小 > 分区容量 → 报错中止
7. 头 28 字节探测格式：
     - sparse magic 0xed26ff3a → 走 sparse 核心（复用 download_sparse_from_reader，数据源换成分区 img）
     - 否则 raw → RawSectorWriter 从分区 first_lba 起写入
8. verify=true：fes_verify_value(first_lba, img_len) 校验
9. post_action：默认 none（不重启）
```

要点：

- GPT 解析校验头部 `signature == "EFI PART"`、header CRC32、entries CRC32，避免误刷坏 GPT。
- 找不到分区时列出设备上全部分区名，便于对照。

## FEL 引导（功能 A，风险点）

抽出 `FelBootstrap`，输入 `(dram_init_blob, uboot_blob, 可选 sys_config)`：

```
现有 IMAGEWTY 路径： blob 来自 packer.get_fes()/get_uboot()/get_sys_config_bin()
新 raw.img 路径：     blob 来自 raw.img 固定偏移切片
   - boot0  @ sector 16 (8 KiB)，按 eGON.BT0 头里的 length 字段确定长度
   - toc1/uboot @ 标准 sunxi 偏移（具名常量 UBOOT_START_SECTOR）
```

控制风险的实现策略：

- 偏移用具名常量集中放在一处，并提供隐藏调试参数（如 `--uboot-offset`）作安全阀，便于实测覆盖。
- boot0 作 DRAM-init blob，复用 `DramInit`（其本就解析 `Boot0Header`）。
- uboot blob 复用 `UbootDownload`：设 `WORK_MODE_USB_PRODUCT` 进 FES；现有逻辑里 sys_config 为必填，**raw 路径改为可选**（raw.img 通常无独立 sys_config.bin）。
- 引导后复用现有 `reconnect_device` 逻辑重连为 FES。

**已知风险（诚实声明）**：能否直接从 raw.img 提取的 boot0/uboot 走 FEL 引导成功，依赖镜像内 boot0/uboot 的格式与构建方式，因 SoC 而异，属尽力而为。「设备已在 FES 模式直接写」始终作为可靠退路。`UBOOT_START_SECTOR` 等常量的具体取值在实现阶段对照目标 SoC 的 boot ROM 约定及 boot0 eGON 头记录的 boot package 信息确认。

## 错误处理与校验

- 复用现有 `FlashError`，新增变体：`RawImageTooLarge`、`GptInvalid`、`PartitionNotFound(name)`、`DeviceNotInFes`、`SparseUnsupportedHere`。
- 容量校验：写前比较镜像/分区 img 大小与目标容量，超限即中止。
- 校验：`verify=true` 时边写边累计 `IncrementalChecksum`，写完用 `fes_verify_value(sector, len)` 比对（与现有分区校验同一套）。
- flash-part 找不到分区名时，列出设备 GPT 上全部分区名。

## 测试

- **单元测试（不依赖设备）**
  - `gpt_parser`：构造 GPT 字节（protective MBR + 头 + 分区项 + 正确/错误 CRC），测试解析、CRC 校验、UTF-16 分区名、按名查找。
  - sparse 数据源抽象：用现有 sparse 样本走新 reader 数据源，确认与原 packer 路径行为一致。
  - 容量/边界校验逻辑。
- **手动/集成测试（需真机，文档说明）**
  - FEL 引导路径、整盘 flash-raw、flash-part 单分区，对照 verify 结果。

## 涉及的现有代码改动

- `src/cli.rs`：新增 `FlashRaw`、`FlashPart` 子命令变体。
- `src/commands/mod.rs`、`src/main.rs`：路由新命令。
- `src/flash/mod.rs`、`src/flash/fel_handler/mod.rs`：导出新模块；`Flasher`/`FelHandler` 改用抽出的 `FelBootstrap` 与 `DeviceSession`（行为保持等价）。
- `src/utils/error.rs`：新增 `FlashError` 变体。
- 现有 `RawDownloader`/`SparseDownloader` 保持原状，新原语另起，避免破坏现有路径。
