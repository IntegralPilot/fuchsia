// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zbitl/error-stdio.h>
#include <lib/zircon_boot/zircon_boot.h>

#include <phys/boot-zbi.h>
#include <phys/stdio.h>

#include "backends.h"
#include "boot_zbi_items.h"
#include "gpt.h"
#include "utils.h"

namespace gigaboot {
namespace {
// Reasonable guess for the size of additional zbi items on this platform.
constexpr size_t kKernelBufferZbiSizeEstimate = 0x8000;

bool ReadFromPartition(ZirconBootOps* ops, const char* part, size_t offset, size_t size, void* dst,
                       size_t* read_size) {
  auto gpt_device = FindEfiGptDevice();
  if (gpt_device.is_error()) {
    return false;
  }

  auto load_res = gpt_device.value().Load();
  if (load_res.is_error()) {
    return false;
  }

  *read_size = size;
  part = MaybeMapPartitionName(gpt_device.value(), part).data();
  return gpt_device.value().ReadPartition(part, offset, size, dst).is_ok();
}

bool WriteToPartition(ZirconBootOps* ops, const char* part, size_t offset, size_t size,
                      const void* src, size_t* write_size) {
  auto gpt_device = FindEfiGptDevice();
  if (gpt_device.is_error()) {
    return false;
  }

  auto load_res = gpt_device.value().Load();
  if (load_res.is_error()) {
    return false;
  }

  *write_size = size;
  part = MaybeMapPartitionName(gpt_device.value(), part).data();
  return gpt_device.value().WritePartition(part, src, offset, size).is_ok();
}

void Boot(ZirconBootOps* ops, zbi_header_t* zbi, size_t capacity) {
  // TODO(https://fxbug.dev/42159148): Implement the same relocation logic in zircon_boot
  // library and use it here to validate.
  printf("Booting zircon\n");

  BootZbi::InputZbi input_zbi_view(
      zbitl::StorageFromRawHeader(static_cast<const zbi_header_t*>(zbi)));

  BootZbi boot;
  if (auto result = boot.Init(input_zbi_view); result.is_error()) {
    printf("boot: Not a bootable ZBI: ");
    zbitl::PrintViewCopyError(result.error_value());
    abort();
  }

  // Reserve extra capacity for memory map items to be added next.
  // Once we add memory map related items, we must not do anything that can change the memory map,
  // i.e. dynamic allocation. Because BootZbi::Load() do memory allocation, we can only add these
  // items after.
  uint32_t extra_data_capacity = 64 * 1024;
  if (auto result = boot.Load(extra_data_capacity); result.is_error()) {
    printf("boot: Failed to load ZBI: ");
    zbitl::PrintViewCopyError(result.error_value());
    abort();
  }

  void* data_zbi = reinterpret_cast<void*>(boot.DataLoadAddress());
  size_t data_zbi_capacity = boot.DataZbi().storage().size();
  auto memory_attr =
      AddMemoryItems(data_zbi, data_zbi_capacity, reinterpret_cast<ZbiContext*>(ops->context));
  if (memory_attr.is_error()) {
    printf("Failed to add additional memory ranges\n");
    abort();
  }
  size_t mkey = memory_attr.value();

  efi_status exit_res = gEfiSystemTable->BootServices->ExitBootServices(gEfiImageHandle, mkey);
  if (exit_res != EFI_SUCCESS) {
    printf("Failed to exit boot service %s\n", gigaboot::EfiStatusToString(exit_res));
    abort();
  }

  // Once we exit boot service, console output will be set to NULL. Thus we must not print from now
  // on otherwise it crashes.
  PhysConsole& console = PhysConsole::Get();
  console.set_serial(*console.null());
  console.set_graphics(*console.null());
  boot.Boot();
}

bool AddZbiItems(ZirconBootOps* ops, zbi_header_t* image, size_t capacity,
                 const AbrSlotIndex* slot) {
  // TODO(b/235489025): To implement. Append necessary ZBI items for booting the ZBI image. Refers
  // to the C Gigaboot implementation in function boot_zircon() in
  // `src/firmware/gigaboot/src/zircon.c` for what items are needed.
  return AddGigabootZbiItems(image, capacity, slot, reinterpret_cast<ZbiContext*>(ops->context));
}

bool ReadPermanentAttributes(ZirconBootOps* ops, AvbAtxPermanentAttributes* attribute) {
  ZX_ASSERT(attribute);
  const std::span<const uint8_t> perm_attr = GetPermanentAttributes();
  if (perm_attr.size() != sizeof(AvbAtxPermanentAttributes)) {
    return false;
  }

  memcpy(attribute, perm_attr.data(), perm_attr.size());
  return true;
}

bool ReadPermanentAttributesHash(ZirconBootOps* ops, uint8_t* hash) {
  ZX_ASSERT(hash);
  const std::span<const uint8_t> perm_attr_hash = GetPermanentAttributesHash();
  memcpy(hash, perm_attr_hash.data(), perm_attr_hash.size());
  return true;
}

uint8_t* GetLoadBuffer(ZirconBootOps* ops, size_t* size) {
  ZX_ASSERT(size);
  efi_physical_addr addr;
  *size += kKernelBufferZbiSizeEstimate;
  efi_status status = gEfiSystemTable->BootServices->AllocatePages(
      AllocateAnyPages, EfiLoaderData, DivideRoundUp(*size, kUefiPageSize), &addr);
  return status == EFI_SUCCESS ? reinterpret_cast<uint8_t*>(addr) : nullptr;
}

bool VerifiedBootGetPartitionSize(ZirconBootOps* ops, const char* part, size_t* out) {
  auto gpt_device = FindEfiGptDevice();
  if (gpt_device.is_error()) {
    return false;
  }

  auto load_res = gpt_device.value().Load();
  if (load_res.is_error()) {
    return false;
  }

  part = MaybeMapPartitionName(gpt_device.value(), part).data();
  const gpt_entry_t* gpt_entry = gpt_device.value().FindPartition(part);
  if (!gpt_entry) {
    return false;
  }

  *out = (gpt_entry->last - gpt_entry->first + 1) * gpt_device.value().BlockSize();
  return true;
}

// TODO(b/235489025): Implement these callbacks correctly.
bool VerifiedBootReadIsDeviceLocked(ZirconBootOps* ops, bool* out_is_locked) {
  *out_is_locked = true;
  return true;
}

bool VerifiedBootWriteRollbackIndex(ZirconBootOps* ops, size_t rollback_index_location,
                                    uint64_t rollback_index) {
  return true;
}

bool VerifiedBootReadRollbackIndex(ZirconBootOps* ops, size_t rollback_index_location,
                                   uint64_t* out_rollback_index) {
  *out_rollback_index = 0;
  return true;
}

}  // namespace

// TODO(b/269178761): write unit tests for "default" implementation of zircon boot ops code.
ZirconBootOps GetZirconBootOps() {
  static ZbiContext zbi_context = {};

  ZirconBootOps zircon_boot_ops;
  zircon_boot_ops.context = &zbi_context;
  zircon_boot_ops.read_from_partition = ReadFromPartition;
  zircon_boot_ops.write_to_partition = WriteToPartition;
  zircon_boot_ops.boot = Boot;
  zircon_boot_ops.add_zbi_items = AddZbiItems;
  zircon_boot_ops.get_kernel_load_buffer = GetLoadBuffer;
  zircon_boot_ops.firmware_can_boot_kernel_slot = nullptr;

  zircon_boot_ops.verified_boot_get_partition_size = VerifiedBootGetPartitionSize;
  zircon_boot_ops.verified_boot_read_rollback_index = VerifiedBootReadRollbackIndex;
  zircon_boot_ops.verified_boot_write_rollback_index = VerifiedBootWriteRollbackIndex;
  zircon_boot_ops.verified_boot_read_is_device_locked = VerifiedBootReadIsDeviceLocked;
  zircon_boot_ops.verified_boot_read_permanent_attributes = ReadPermanentAttributes;
  zircon_boot_ops.verified_boot_read_permanent_attributes_hash = ReadPermanentAttributesHash;
  return zircon_boot_ops;
}

}  // namespace gigaboot
