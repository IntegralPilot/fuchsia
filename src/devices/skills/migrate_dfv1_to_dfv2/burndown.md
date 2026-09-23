# DFv1 to DFv2 Migration Burndown Plan

This document outlines the plan for migrating the remaining DFv1 drivers to DFv2, organized by logical groups and ranked by estimated difficulty (easiest first).

## 1. RTC Drivers [Deferred: These drivers are being converted to Rust]
**Drivers**: `intel-rtc`
**Difficulty**: Easy
**Reasoning**: Real-Time Clock drivers are typically simple, involving reading and writing time values to hardware registers. They usually do not involve complex DMA or high-frequency interrupts. This is a good starting point for learning the migration process.

## 2. VirtIO Drivers
**Drivers**: `virtio-socket`
**Difficulty**: Medium
**Reasoning**: While `virtio-rng` has been migrated, `virtio-socket` remains. It is more complex than RNG due to ring management and FIDL communication, but it follows the VirtIO pattern which is now familiar.

## 3. Block & Storage Drivers
**Drivers**: `block-verity`, `core` (block), `ftl`, `gpt`, `mbr`, `pci-sdhci`, `ramdisk`, `ums-function`, `zxcrypt`
**Difficulty**: Medium to Hard
**Reasoning**: This group includes partition managers (`gpt`, `mbr`) which are purely logical and might be easier, as well as crypto filters (`zxcrypt`) and hardware drivers (`pci-sdhci`). The complexity varies, but they all interact with the block protocol.

## 4. NAND Drivers
**Drivers**: `aml-rawnand`, `aml-spinand`, `intel-spi-flash`, `skip-block`
**Difficulty**: Hard
**Reasoning**: NAND flash drivers deal with complex operations like bad block management, wear leveling, and hardware-specific controller logic. They are significantly more complex than simple register-based drivers.

## 5. USB Core Drivers
**Drivers**: `usb-bus`, `usb-composite`, `usb-hub`
**Difficulty**: Very Hard
**Reasoning**: The USB stack is notoriously complex. Migrating the core bus and hub drivers requires a deep understanding of the USB protocol and the driver framework's interaction with it.

## 6. Board Drivers
**Drivers**: `astro`, `nelson`, `sherlock`, `x86`, `machina`, `qemu-arm64`, `qemu-riscv64`
**Difficulty**: Very Hard
**Reasoning**: Board drivers are the root of the device tree for specific platforms. They handle platform initialization and resource allocation. Migrating them is high risk and requires understanding the platform's specific boot sequence and hardware layout.
