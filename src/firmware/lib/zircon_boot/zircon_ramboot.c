// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "zircon_ramboot.h"

#ifdef ZIRCON_BOOT_CUSTOM_SYSDEPS_HEADER
#include <zircon_boot_sysdeps.h>
#else
#include <string.h>
#endif

#include <lib/zircon_boot/android_boot_image.h>

#include "utils.h"

// Locates the ZBI and vbmeta images in any of the supported formats.
static ZirconBootResult UnpackRamImage(const void* image, size_t size, UnpackedRamImage* unpacked) {
  // Try to unwrap an Android boot image first, which will be the most common
  // case using `fastboot boot`.
  const void* boot_image = NULL;
  size_t boot_image_size = 0;
  if (!zircon_boot_get_android_image_kernel(image, size, &boot_image, &boot_image_size)) {
    // If it's not an Android boot image, |image| points directly to the ZBI.
    boot_image = image;
    boot_image_size = size;
  }

  // Make sure it looks like a ZBI and mark the size.
  unpacked->zbi = boot_image;
  unpacked->zbi_size = ZbiCheckSize(boot_image, boot_image_size);
  if (unpacked->zbi_size == 0) {
    zircon_boot_dlog("RAM image does not have a valid ZBI\n");
    return kBootResultErrorInvalidZbi;
  }

  // Any leftover data following the ZBI is the vbmeta image.
  unpacked->vbmeta_size = boot_image_size - unpacked->zbi_size;
  if (unpacked->vbmeta_size == 0) {
    unpacked->vbmeta = NULL;
  } else if (unpacked->vbmeta_size < sizeof(AvbVBMetaImageHeader)) {
    zircon_boot_dlog("RAM image contains invalid vbmeta data\n");
    return kBootResultErrorSlotVerification;
  } else {
    unpacked->vbmeta = (const uint8_t*)unpacked->zbi + unpacked->zbi_size;
  }

  return kBootResultOK;
}

// Determines the RAM "paritition" location and size from the name.
// Returns true if the partition is valid for a RAM-boot, though data/size
// may still be zero if we're looking for the vbmeta partition and it wasn't
// provided in RAM.
static bool GetRamPartition(const char* name, const UnpackedRamImage* ram_image,
                            const uint8_t** data, size_t* size) {
  // RAM-boot should only ever need to access the zircon and vbmeta data using this API.
  if (strcmp(name, GPT_ZIRCON_SLOTLESS_NAME) == 0) {
    *data = (const uint8_t*)ram_image->zbi;
    *size = ram_image->zbi_size;
    return true;
  } else if (strcmp(name, GPT_VBMETA_SLOTLESS_NAME) == 0) {
    *data = (const uint8_t*)ram_image->vbmeta;
    *size = ram_image->vbmeta_size;
    return true;
  }

  zircon_boot_dlog("Unknown RAM-boot partition: %s\n", name);
  return false;
}

// Passes a ZirconBootOps call from the Ramboot wrapper through to the user
// implementation. We can't just call the original ops function directly because
// the ZirconBootOps context is now our custom RAM-boot context, so we need to
// call via the original ops which contains the user context.
#define RAMBOOT_PASSTHROUGH(ops, func, ...)                                      \
  do {                                                                           \
    ZirconBootOps* original_ops = ((RambootContext*)ops->context)->original_ops; \
    return original_ops->func(original_ops, __VA_ARGS__);                        \
  } while (0)

static bool RambootReadFromPartition(ZirconBootOps* ops, const char* part, size_t offset,
                                     size_t size, void* dst, size_t* read_size) {
  RambootContext* context = (RambootContext*)ops->context;

  const uint8_t* source = NULL;
  size_t source_size = 0;
  if (!GetRamPartition(part, &context->ram_image, &source, &source_size)) {
    return false;
  }

  // We sometimes need to read past the given image data, e.g. libavb loads
  // a fixed-size chunk of the largest supported vbmeta image which will be
  // more than exists in our RAM buffer, or the vbmeta image may not exist
  // at all if it wasn't provided for RAM-booting. In this case just pad the
  // ending with zeros to give the amount requested.
  if (offset + size <= source_size) {
    // The read can be entirely taken from source.
    source_size = size;
  } else if (offset < source_size) {
    // The read starts in the source but overflows.
    source_size -= offset;
  } else {
    // Entirely outside of source.
    source_size = 0;
  }
  size_t zeros_size = size - source_size;

  if (source_size) {
    memcpy(dst, source + offset, source_size);
  }
  if (zeros_size) {
    memset((uint8_t*)dst + source_size, 0, zeros_size);
  }

  *read_size = size;
  return true;
}

static bool RambootAddZbiItems(ZirconBootOps* ops, zbi_header_t* image, size_t capacity,
                               const AbrSlotIndex* slot) {
  RAMBOOT_PASSTHROUGH(ops, add_zbi_items, image, capacity, slot);
}

static bool RambootVerifiedBootGetPartitionSize(ZirconBootOps* ops, const char* part, size_t* out) {
  RambootContext* context = (RambootContext*)ops->context;

  const uint8_t* source = NULL;
  size_t source_size = 0;
  if (!GetRamPartition(part, &context->ram_image, &source, &source_size)) {
    return false;
  }

  *out = source_size;
  return true;
}

static bool RambootVerifiedBootReadRollbackIndex(ZirconBootOps* ops, size_t rollback_index_location,
                                                 uint64_t* out_rollback_index) {
  // Read the actual rollback index so we provide anti-rollback enforcement.
  RAMBOOT_PASSTHROUGH(ops, verified_boot_read_rollback_index, rollback_index_location,
                      out_rollback_index);
}

static bool RambootVerifiedBootWriteRollbackIndex(ZirconBootOps* ops,
                                                  size_t rollback_index_location,
                                                  uint64_t rollback_index) {
  // No-op. This is critical, rollbacks must be enforced but never updated when
  // RAM-booting or else we could easily brick the device. Only loads from disk
  // should update the rollbacks.
  return true;
}

static bool RambootVerifiedBootReadIsDeviceLocked(ZirconBootOps* ops, bool* out_is_locked) {
  RAMBOOT_PASSTHROUGH(ops, verified_boot_read_is_device_locked, out_is_locked);
}

static bool RambootVerifiedBootReadPermanentAttributes(ZirconBootOps* ops,
                                                       AvbAtxPermanentAttributes* attribute) {
  RAMBOOT_PASSTHROUGH(ops, verified_boot_read_permanent_attributes, attribute);
}

static bool RambootVerifiedBootReadPermanentAttributesHash(ZirconBootOps* ops, uint8_t* hash) {
  RAMBOOT_PASSTHROUGH(ops, verified_boot_read_permanent_attributes_hash, hash);
}

static uint8_t* RambootGetKernelLoadBuffer(ZirconBootOps* ops, size_t* size) {
  RAMBOOT_PASSTHROUGH(ops, get_kernel_load_buffer, size);
}

#undef RAMBOOT_PASSTHROUGH

ZirconBootResult SetupRambootOps(ZirconBootOps* ops, const void* image, size_t size,
                                 RambootContext* ramboot_context, ZirconBootOps* ramboot_ops) {
  ramboot_context->original_ops = ops;
  ZirconBootResult res = UnpackRamImage(image, size, &ramboot_context->ram_image);
  if (res != kBootResultOK) {
    return res;
  }

  memset(ramboot_ops, 0, sizeof(*ramboot_ops));
  ramboot_ops->context = ramboot_context;

  // Wrap the user-defined ops in our RAM-boot implementation, but leave the
  // NULL ops empty so the boot logic knows which functions are available.
#define SET_OP(name, wrapper) ramboot_ops->name = (ops->name ? wrapper : NULL)
  SET_OP(read_from_partition, RambootReadFromPartition);
  // write_to_partition() should never be called during RAM-boot; leave zeroed.
  // boot() should never be called during RAM-boot; leave zeroed.
  // firmware_can_boot_kernel_slot() should never be called during RAM-boot; leave zeroed.
  // reboot() should never be called during RAM-boot; leave zeroed.
  SET_OP(add_zbi_items, RambootAddZbiItems);
  SET_OP(verified_boot_get_partition_size, RambootVerifiedBootGetPartitionSize);
  SET_OP(verified_boot_read_rollback_index, RambootVerifiedBootReadRollbackIndex);
  SET_OP(verified_boot_write_rollback_index, RambootVerifiedBootWriteRollbackIndex);
  SET_OP(verified_boot_read_is_device_locked, RambootVerifiedBootReadIsDeviceLocked);
  SET_OP(verified_boot_read_permanent_attributes, RambootVerifiedBootReadPermanentAttributes);
  SET_OP(verified_boot_read_permanent_attributes_hash,
         RambootVerifiedBootReadPermanentAttributesHash);
  SET_OP(get_kernel_load_buffer, RambootGetKernelLoadBuffer);
#undef SET_OP

  return kBootResultOK;
}
