// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ZBITL_INCLUDE_LIB_ZBITL_ITEM_H_
#define SRC_LIB_ZBITL_INCLUDE_LIB_ZBITL_ITEM_H_

#include <lib/zbi-format/internal/deprecated-cpu.h>
#include <lib/zbi-format/zbi.h>

#include <array>
#include <cstdint>
#include <string_view>
#include <type_traits>

namespace zbitl {

// All known item types. Must be kept in sync with definitions in
// <lib/zbi-format/zbi.h>.
//
// TODO(https://fxbug.dev/42078423): Consider some way of generating this.
inline constexpr std::array kItemTypes = {
    ZBI_TYPE_CONTAINER,
    ZBI_TYPE_KERNEL_X64,
    ZBI_TYPE_KERNEL_ARM64,
    ZBI_TYPE_KERNEL_RISCV64,
    ZBI_TYPE_DISCARD,
    ZBI_TYPE_STORAGE_RAMDISK,
    ZBI_TYPE_STORAGE_BOOTFS,
    ZBI_TYPE_STORAGE_BOOTFS_FACTORY,
    ZBI_TYPE_STORAGE_KERNEL,
    ZBI_TYPE_CMDLINE,
    ZBI_TYPE_CRASHLOG,
    ZBI_TYPE_NVRAM,
    ZBI_TYPE_PLATFORM_ID,
    ZBI_TYPE_CPU_TOPOLOGY,
    ZBI_TYPE_DEPRECATED_CPU_TOPOLOGY_V1,
    ZBI_TYPE_DEPRECATED_CPU_TOPOLOGY_V2,
    ZBI_TYPE_MEM_CONFIG,
    ZBI_TYPE_KERNEL_DRIVER,
    ZBI_TYPE_ACPI_RSDP,
    ZBI_TYPE_SMBIOS,
    ZBI_TYPE_EFI_SYSTEM_TABLE,
    ZBI_TYPE_FRAMEBUFFER,
    ZBI_TYPE_DRV_MAC_ADDRESS,
    ZBI_TYPE_DRV_PARTITION_MAP,
    ZBI_TYPE_DRV_BOARD_PRIVATE,
    ZBI_TYPE_DRV_BOARD_INFO,
    ZBI_TYPE_IMAGE_ARGS,
    ZBI_TYPE_BOOT_VERSION,
    ZBI_TYPE_HW_REBOOT_REASON,
    ZBI_TYPE_SERIAL_NUMBER,
    ZBI_TYPE_BOOTLOADER_FILE,
    ZBI_TYPE_DEVICETREE,
    ZBI_TYPE_SECURE_ENTROPY,
    ZBI_TYPE_EFI_MEMORY_ATTRIBUTES_TABLE,
};

/// This returns the canonical name string for this zbi_header_t.type value.
/// It returns the default-constructed (empty()) string_view for unknown types.
std::string_view TypeName(uint32_t);
inline std::string_view TypeName(const zbi_header_t& header) { return TypeName(header.type); }

/// This returns the canonical file name extension string for this
/// zbi_header_t.type value.  It returns the default-constructed (i.e. empty())
/// string_view for unknown types.
std::string_view TypeExtension(uint32_t);
inline std::string_view TypeExtension(const zbi_header_t& header) {
  return TypeExtension(header.type);
}

/// Returns true for any kernel item type.
constexpr bool TypeIsKernel(uint32_t type) {
  return (type & ZBI_TYPE_KERNEL_MASK) == ZBI_TYPE_KERNEL_PREFIX;
}

constexpr bool TypeIsKernel(const zbi_header_t& header) { return TypeIsKernel(header.type); }

/// Returns true for any ZBI_TYPE_STORAGE_* type.
/// These share a protocol for other header fields, compression, etc.
bool TypeIsStorage(uint32_t);
inline bool TypeIsStorage(const zbi_header_t& header) { return TypeIsStorage(header.type); }

/// This returns the length of the item payload after decompression.
/// If this is not a ZBI_TYPE_STORAGE_* item, this is just `header.length`.
inline uint32_t UncompressedLength(const zbi_header_t& header) {
  return TypeIsStorage(header) ? header.extra : header.length;
}

constexpr uint32_t AlignedPayloadLength(uint32_t content_length) {
  return ((content_length + ZBI_ALIGNMENT - 1) & -ZBI_ALIGNMENT);
}

constexpr uint32_t AlignedPayloadLength(const zbi_header_t& header) {
  return AlignedPayloadLength(header.length);
}

constexpr uint32_t AlignedItemLength(uint32_t content_length) {
  return sizeof(zbi_header_t) + AlignedPayloadLength(content_length);
}

constexpr uint32_t AlignedItemLength(const zbi_header_t& header) {
  return AlignedItemLength(header.length);
}

constexpr zbi_header_t ContainerHeader(uint32_t length) {
  return {
      .type = ZBI_TYPE_CONTAINER,
      .length = length,
      .extra = ZBI_CONTAINER_MAGIC,
      .flags = ZBI_FLAGS_VERSION,
      .magic = ZBI_ITEM_MAGIC,
      .crc32 = ZBI_ITEM_NO_CRC32,
  };
}

}  // namespace zbitl

#endif  // SRC_LIB_ZBITL_INCLUDE_LIB_ZBITL_ITEM_H_
