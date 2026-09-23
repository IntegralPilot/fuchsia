// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zbitl/item.h>

#include <string_view>

using namespace std::literals;

namespace zbitl {

// TODO(https://fxbug.dev/42078423): Consider some way of generating this.
std::string_view TypeName(uint32_t type) {
  using namespace std::string_view_literals;

  switch (type) {
    case ZBI_TYPE_CONTAINER:
      return "CONTAINER"sv;
    case ZBI_TYPE_KERNEL_X64:
      return "KERNEL_X64"sv;
    case ZBI_TYPE_KERNEL_ARM64:
      return "KERNEL_ARM64"sv;
    case ZBI_TYPE_KERNEL_RISCV64:
      return "KERNEL_RISCV64"sv;
    case ZBI_TYPE_DISCARD:
      return "DISCARD"sv;
    case ZBI_TYPE_STORAGE_RAMDISK:
      return "RAMDISK"sv;
    case ZBI_TYPE_STORAGE_BOOTFS:
      return "BOOTFS"sv;
    case ZBI_TYPE_STORAGE_BOOTFS_FACTORY:
      return "BOOTFS_FACTORY"sv;
    case ZBI_TYPE_STORAGE_KERNEL:
      return "KERNEL"sv;
    case ZBI_TYPE_CMDLINE:
      return "CMDLINE"sv;
    case ZBI_TYPE_CRASHLOG:
      return "CRASHLOG"sv;
    case ZBI_TYPE_NVRAM:
      return "NVRAM"sv;
    case ZBI_TYPE_PLATFORM_ID:
      return "PLATFORM_ID"sv;
    case ZBI_TYPE_CPU_TOPOLOGY:
      return "CPU_TOPOLOGY"sv;
    case ZBI_TYPE_DEPRECATED_CPU_TOPOLOGY_V1:
      return "DEPRECATED_CPU_TOPOLOGY_V1"sv;
    case ZBI_TYPE_DEPRECATED_CPU_TOPOLOGY_V2:
      return "DEPRECATED_CPU_TOPOLOGY_V2"sv;
    case ZBI_TYPE_MEM_CONFIG:
      return "MEM_CONFIG"sv;
    case ZBI_TYPE_KERNEL_DRIVER:
      return "KERNEL_DRIVER"sv;
    case ZBI_TYPE_ACPI_RSDP:
      return "ACPI_RSDP"sv;
    case ZBI_TYPE_SMBIOS:
      return "SMBIOS"sv;
    case ZBI_TYPE_EFI_SYSTEM_TABLE:
      return "EFI_SYSTEM_TABLE"sv;
    case ZBI_TYPE_FRAMEBUFFER:
      return "FRAMEBUFFER"sv;
    case ZBI_TYPE_DRV_MAC_ADDRESS:
      return "DRV_MAC_ADDRESS"sv;
    case ZBI_TYPE_DRV_PARTITION_MAP:
      return "DRV_PARTITION_MAP"sv;
    case ZBI_TYPE_DRV_BOARD_PRIVATE:
      return "DRV_BOARD_PRIVATE"sv;
    case ZBI_TYPE_DRV_BOARD_INFO:
      return "DRV_BOARD_INFO"sv;
    case ZBI_TYPE_IMAGE_ARGS:
      return "IMAGE_ARGS"sv;
    case ZBI_TYPE_BOOT_VERSION:
      return "BOOT_VERSION"sv;
    case ZBI_TYPE_HW_REBOOT_REASON:
      return "HW_REBOOT_REASON"sv;
    case ZBI_TYPE_SERIAL_NUMBER:
      return "SERIAL_NUMBER"sv;
    case ZBI_TYPE_BOOTLOADER_FILE:
      return "BOOTLOADER_FILE"sv;
    case ZBI_TYPE_DEVICETREE:
      return "DEVICETREE"sv;
    case ZBI_TYPE_SECURE_ENTROPY:
      return "ENTROPY"sv;
    case ZBI_TYPE_EFI_MEMORY_ATTRIBUTES_TABLE:
      return "EFI_MEMORY_ATTRIBUTES_TABLE"sv;
  }
  return {};
}

std::string_view TypeExtension(uint32_t type) {
  using namespace std::string_view_literals;

  switch (type) {
    case ZBI_TYPE_CMDLINE:
    case ZBI_TYPE_IMAGE_ARGS:
    case ZBI_TYPE_SERIAL_NUMBER:
      return ".txt"sv;

    case ZBI_TYPE_DEVICETREE:
      return ".dtb";
  }
  return ".bin"sv;
}

bool TypeIsStorage(uint32_t type) {
  // TODO(mcgrathr): Ideally we'd encode this as a mask of type bits or
  // something else simple.  Short of that, someplace more authoritative than
  // here should contain this list.  But N.B. that no ZBI_TYPE_STORAGE_* type
  // is a long-term stable protocol with boot loaders, so meh.
  //
  // TODO(mcgrathr): ZBI_TYPE_STORAGE_BOOTFS_FACTORY is misnamed and is not
  // actually a "storage" type.
  switch (type) {
    case ZBI_TYPE_STORAGE_RAMDISK:
    case ZBI_TYPE_STORAGE_BOOTFS:
    case ZBI_TYPE_STORAGE_KERNEL:
      return true;
  }
  return false;
}

}  // namespace zbitl
