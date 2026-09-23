// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <crc32.h>
#include <lib/abr/data.h>
#include <lib/storage/gpt_utils.h>
#include <lib/storage/storage.h>
#include <lib/zircon_boot/zbi_utils.h>
#include <lib/zircon_boot/zircon_boot.h>
#include <zircon/hw/gpt.h>

// This file gives an example of using the firmware SDK to implement firmware for booting Zircon on
// a disk with GPT. The GPT disk should contain the following partitions:
//
//   zircon_a -- Zircon A slot
//   zircon_b -- Zircon B slot
//   zircon_r -- Zircon R slot
//   vbmeta_a -- verified boot metadata A slot
//   vbmeta_b -- verified boot metadata B slot
//   vbmeta_r -- verified boot metadata R slot
//   durable_boot -- A/B/R metadata
//
// The code in this example is generic and designed to be used as reference. Unless otherwise
// stated, the implementation is recommended to be reused as it is, or as a basis for additioanl
// changes.
//
// A end-to-end runnable demo of this example is available in
// src/firmware/lib/zircon_boot/gpt_boot_demo.cc

// Dependency for libabr Crc32 calculation. We use the crc32 implementation from the firmware SDK
// Device can also choose to use its own implementation.
uint32_t AbrCrc32(const void* buf, size_t buf_size) {
  return Crc32((const uint8_t*)buf, (uint32_t)buf_size);
}

// The first step is to construct a `FuchsiaFirmwareStorage` object. It is used as an abstraction of
// the main storage by the firmware SDK to access GPT disk. See
// src/firmware/lib/zircon_boot/include/lib/zircon_boot/storage_utils.h details of its definition.

// For the purpose of demonstration, the following implements a RAM buffer based
// `FuchsiaFirmwareStorage` for concept illustration. Device should replace them with its own
// implementation.

#define GPT_BOOT_STORAGE_BLOCK_SIZE 512
uint8_t ram_disk[GPT_BOOT_STORAGE_BLOCK_SIZE * 128];  // 64k

static bool Read(void* ctx, size_t block_offset, size_t blocks_count, void* dst) {
  size_t offset = block_offset * GPT_BOOT_STORAGE_BLOCK_SIZE,
         size = blocks_count * GPT_BOOT_STORAGE_BLOCK_SIZE;
  memcpy(dst, ram_disk + offset, size);
  return true;
}

static bool Write(void* ctx, size_t block_offset, size_t blocks_count, const void* src) {
  size_t offset = block_offset * GPT_BOOT_STORAGE_BLOCK_SIZE,
         size = blocks_count * GPT_BOOT_STORAGE_BLOCK_SIZE;
  memcpy(ram_disk + offset, src, size);
  return true;
}

// `FuchsiaFirmwareStorage` requires a scratch buffer to handle unaligned read/write. The buffer
// needs to have at least a capacity of the block size and be aligned according to
// FUCHSIA_FIRMWARE_STORAGE_BUFFER_ALIGNMENT. Below is an example definition of such a buffer.
static uint8_t scratch_buffer[GPT_BOOT_STORAGE_BLOCK_SIZE]
    __attribute__((__aligned__(FUCHSIA_FIRMWARE_STORAGE_BUFFER_ALIGNMENT)));

// Defines a global `FuchsiaFirmwareStorage` as boot storage
FuchsiaFirmwareStorage gpt_boot_storage = {
    .block_size = GPT_BOOT_STORAGE_BLOCK_SIZE,
    .total_blocks = sizeof(ram_disk) / GPT_BOOT_STORAGE_BLOCK_SIZE,
    .ctx = NULL,
    .scratch_buffer = scratch_buffer,
    .read = Read,
    .write = Write,
};

// Next we construct a `ZirconBootOps` object. It is the main argument needed by the LoadAndBoot()
// API which we'll use to boot zircon. It contains a set of callbacks for device operations. See
// src/firmware/lib/zircon_boot/include/lib/zircon_boot/zircon_boot.h for details. Below is a
// reference implementation for each callback.

// Read data from the GPT partition.
//
// Note: The `offset`, `size` and `dst` arguments passed to this callback may not necessarily be
// aligned to block or DMA, thus we recommend using the reference implementation whenever possible,
// which internally handles unaligned read.
static bool ReadFromPartition(ZirconBootOps* ops, const char* part, size_t offset, size_t size,
                              void* dst, size_t* read_size) {
  *read_size = size;
  GptData* gpt_data = (GptData*)ops->context;
  return FuchsiaFirmwareStorageGptRead(&gpt_boot_storage, gpt_data, part, offset, size, dst);
}

// Write data to the a GPT partition.
//
// Note: See note of `ReadFromPartition`
static bool WriteToPartition(ZirconBootOps* ops, const char* part, size_t offset, size_t size,
                             const void* src, size_t* write_size) {
  *write_size = size;
  GptData* gpt_data = (GptData*)ops->context;
  return FuchsiaFirmwareStorageGptWriteConst(&gpt_boot_storage, gpt_data, part, offset, size, src);
}

// An example buffer for loading a kernel image. Device should replace it with its owned buffer.
static uint8_t kernel_load_buffer[8 * 1024];

// Callback to get a buffer for loading the kernel
static uint8_t* GetKernelLoadBuffer(ZirconBootOps* ops, size_t* size) {
  if (*size > sizeof(kernel_load_buffer)) {
    printf("kernel is too large\n");
    return NULL;
  }
  *size = sizeof(kernel_load_buffer);
  return kernel_load_buffer;
}

// An example buffer for booting a kernel image. Device should replace it with its owned buffer.
// The buffer needs to be aligned according to `ZIRCON_BOOT_KERNEL_ALIGN`
static uint8_t kernel_boot_buffer[8 * 1024] __attribute__((__aligned__(ZIRCON_BOOT_KERNEL_ALIGN)));

// Callback to boot the zircon kernel in `image`.
static void Boot(ZirconBootOps* ops, zbi_header_t* image, size_t capacity) {
  // We need to relocate the kernel to a different buffer for booting because zircon requires
  // additional scratch memory at the tail when booting. The following API can be used to extract
  // the kernel from `image` and relocate it to a different address for booting. It also checks
  // whether the buffer can provide sufficient space for scratch memory. Here we relocate the kernel
  // to `kernel_boot_buffer`.
  size_t boot_buffer_size = sizeof(kernel_boot_buffer);
  void* kernel_entry_address = RelocateKernel(image, kernel_boot_buffer, &boot_buffer_size);
  if (!kernel_entry_address) {
    // In case the buffer is too small, `boot_buffer_size` holds the minimum required size.
    printf("Failed to relocate kernel. Required capacity is %zu\n", boot_buffer_size);
    ops->reboot(ops, false);
    return;
  }

  printf("\nJumping to kernel @ %p with zbi @ %p...\n\n", kernel_entry_address, image);
  // From here the device should jump to `kernel_entry_address` and pass the `image` pointer as the
  // first argument.
}

// If device uses firmware A/B/R, that is, if firmware also participates in A/B/R slot switching,
// the following callback should be implemented to make sure that a firmware slot can only boot to
// the corresponding zircon slot, i.e. bootloader_a can only boot zircon_a. If device only has
// Zircon A/B/R, this callback can be ignored.
//
// Zircon A/B/R boot flow:
//   first stage firmware --> main firmware (this example) --> zircon_a
//                                                         --> zircon_b
//                                                         --> zircon_r
//
// Firmware A/B/R boot flow:
//   first stage firmware --> bootloader_a (this example) --> zircon_a
//                        --> bootloader_b (this example) --> zircon_b
//                        --> bootloader_r (this example) --> zircon_r
//
// Note: For firmware A/B/R, we strongly recommend that implementation of `first stage firmware`
// boots bootloader_a/b/r using the same libabr logic and according to the same A/B/R metadata on
// the `durable_boot` partition. A/B/R metadata should be in read-only mode. We suggest using the
// LoadAbrFirmware() API in the firmware SDK for loading firmware.

// A global variable for storing the current firmware slot. See GptBootMain() for how it should be
// set.
static AbrSlotIndex g_firmware_slot_index;
static bool FirmwareCanBootKernelSlot(ZirconBootOps* ops, AbrSlotIndex kernel_slot, bool* out) {
  *out = g_firmware_slot_index == kernel_slot;
  return true;
}

// A callback to reboot the device. If the device uses firmware A/B/R and `force_recovery` is true,
// the device should reboot to the bootloader_r slot. The recommended way to reboot when
// `force_recovery`=true is using the libabr AbrSetOneShotRecovery() API. i.e.
static void Reboot(ZirconBootOps* ops, bool force_recovery) {
  if (force_recovery) {
    AbrOps abr_ops = GetAbrOpsFromZirconBootOps(ops);
    if (AbrSetOneShotRecovery(&abr_ops, true) != kAbrResultOk) {
      printf("Failed to set one shot recovery\n");
      while (1) {
      }
    }
  }

  printf("\nDevice reboots...\n\n");

  // From here, trigger device reboot using device specific API.
}

// Some features in Fuchsia/Zircon requires certain zbi items to be appended by the firmware. The
// following callback is introduced for this purpose. It will be called by LoadAndBoot() before
// handing off to the kernel.
static bool AddZbiItems(ZirconBootOps* ops, zbi_header_t* image, size_t capacity,
                        const AbrSlotIndex* slot) {
  // Appends the current slot index. This should almost always be appended as it is required to
  // enable A/B/R support in Zircon, but in some cases such as RAM-boot there is no slot.
  if (slot && AppendCurrentSlotZbiItem(image, capacity, *slot) != ZBI_RESULT_OK) {
    return false;
  }

  // See zircon/kernel/target/arm64/board/pinecrest/boot-shim-config.h for examples of other zbi
  // items that are commonly needed.
  return true;
}

// The following are verified boot related callbacks. If all of them are implemented, verified boot
// is enabled. If any of them is missing, verified boot is not performed.

// Get the size of a partition.
static bool VerifiedBootGetPartitionSize(ZirconBootOps* ops, const char* part, size_t* out) {
  GptData* gpt_data = (GptData*)ops->context;
  return FuchsiaFirmwareStorageGetPartitionSize(&gpt_boot_storage, gpt_data, part, out);
}

// Read the anti-rollback index at location `rollback_index_location`. On production devices, this
// should be implemented using a secure storage such as RPMB. For demo purpose, we set it to be a
// no-op and always return 0.
static bool VerifiedBootReadRollbackIndex(ZirconBootOps* ops, size_t rollback_index_location,
                                          uint64_t* out_rollback_index) {
  *out_rollback_index = 0;
  return true;
}

// Similar to VerifiedBootReadRollbackIndex, set to no-op for demo purpose.
static bool VerifiedBootWriteRollbackIndex(ZirconBootOps* ops, size_t rollback_index_location,
                                           uint64_t rollback_index) {
  return true;
}

// Returns whether the device is locked. If the device is unlocked, verified boot is attempted but
// failure is not considered fatal and it will still continue to boot. For this demo, we always
// return true. On production device the lock state should be stored in secure storage such as RPMB.
static bool VerifiedBootReadIsDeviceLocked(ZirconBootOps* ops, bool* out_is_locked) {
  *out_is_locked = true;
  return true;
}

// The callback returns the permanent attributes, which contains the avb root trust for performing
// verified boot. A common approach is to hardcode the permanent attributes into the firmware
// binary. For the purpose of demonstration, here we hardcode and return the libavb test permanent
// attributes third_party/android/platform/external/avb/test/data/atx_permanent_attributes.bin. It
// can be used to verify vbmeta images signed using libavb test key
// third_party/android/platform/external/avb/test/data/testkey_atx_psk.pem. For production devices
// using a different vbmeta signing key, this needs to be replaced with the corresponding permanent
// attributes associated with it.
static bool VerifiedBootReadPermanentAttributes(ZirconBootOps* ops,
                                                AvbAtxPermanentAttributes* attribute) {
  static const unsigned char kPermanentAttributes[] = {
      0x1,  0x0,  0x0,  0x0,  0x0,  0x0,  0x10, 0x0,  0x9f, 0x35, 0xef, 0x65, 0xc3, 0x29, 0x4c,
      0x23, 0x16, 0x10, 0xac, 0x32, 0xc1, 0x3c, 0xd5, 0xc5, 0xab, 0xa1, 0xd9, 0xe7, 0x13, 0x3f,
      0x7e, 0xd1, 0xe6, 0x61, 0x5d, 0xa3, 0xa1, 0x60, 0xda, 0x57, 0x4b, 0xb2, 0xe6, 0xf,  0xe1,
      0x50, 0xbf, 0x47, 0xff, 0x9,  0xaf, 0xcd, 0x49, 0x2d, 0x82, 0x33, 0x76, 0xa1, 0xfe, 0x28,
      0x5f, 0x89, 0x62, 0xb3, 0xc0, 0xf1, 0x11, 0xaf, 0x15, 0x9,  0x27, 0xdb, 0xeb, 0x6,  0x1,
      0xa2, 0xf8, 0xb7, 0xd7, 0x9c, 0xe4, 0x88, 0x3a, 0x86, 0x5,  0x2,  0x20, 0x69, 0xb2, 0x36,
      0x4c, 0x3e, 0x25, 0x3,  0xed, 0xfc, 0xc,  0x6b, 0x1b, 0xa,  0x4,  0x9c, 0xce, 0x7f, 0x83,
      0x82, 0x60, 0xd9, 0x52, 0x7e, 0xc4, 0x35, 0x7b, 0x1c, 0xe6, 0x64, 0x9c, 0x17, 0xec, 0x81,
      0xe7, 0x9c, 0xc,  0x8b, 0x4b, 0x7e, 0x48, 0xbe, 0x0,  0x98, 0xa8, 0x20, 0x10, 0x4c, 0x9b,
      0xd1, 0x16, 0x5b, 0x25, 0xe9, 0x4e, 0x61, 0xda, 0x7c, 0x63, 0x80, 0x8f, 0xa4, 0xac, 0x74,
      0xee, 0xa8, 0x6,  0xac, 0x26, 0xd5, 0x71, 0x6f, 0xaa, 0x73, 0x20, 0x9c, 0x7f, 0xcd, 0x73,
      0xd4, 0xa9, 0xa0, 0x7e, 0x5a, 0xb5, 0x61, 0xb0, 0x88, 0xb0, 0xdd, 0xdb, 0x6b, 0x79, 0xd1,
      0x5a, 0x9e, 0x54, 0x49, 0x55, 0xc6, 0x89, 0x76, 0x7a, 0xc6, 0x78, 0x99, 0xdc, 0xc9, 0x0,
      0x5d, 0x20, 0xf5, 0xfc, 0x8f, 0x39, 0x46, 0xf3, 0x2,  0x96, 0xd,  0x9b, 0xfb, 0xbc, 0xd5,
      0xcf, 0x5a, 0x4f, 0xc4, 0xb8, 0xb,  0xd0, 0xf3, 0x19, 0x3c, 0x74, 0x4,  0xd5, 0x94, 0x2c,
      0x19, 0x15, 0x64, 0xbf, 0x53, 0x67, 0x97, 0x7b, 0x9e, 0xc6, 0xe0, 0xfb, 0x29, 0x5b, 0x90,
      0xad, 0x4,  0x8a, 0xd8, 0x5b, 0xdf, 0x69, 0x9,  0xe4, 0xa5, 0xe9, 0xd9, 0xf,  0xc4, 0xff,
      0xae, 0xb7, 0x44, 0x12, 0xae, 0xad, 0x3,  0x97, 0xb8, 0xda, 0xd7, 0x60, 0x37, 0x15, 0xf2,
      0xb9, 0xdb, 0x10, 0xf6, 0xe2, 0x26, 0x48, 0x7e, 0x3e, 0x3e, 0xc3, 0x67, 0xd3, 0xa6, 0x2,
      0xf7, 0xbc, 0x60, 0xed, 0x45, 0xdf, 0x37, 0xef, 0xf9, 0xea, 0x97, 0x5f, 0x37, 0xb4, 0xeb,
      0xb4, 0x91, 0x6c, 0x39, 0x4d, 0xed, 0x52, 0x15, 0x39, 0x47, 0x59, 0x62, 0xde, 0x32, 0x55,
      0xe1, 0xd4, 0x15, 0x58, 0x7d, 0x52, 0x41, 0x12, 0x78, 0xee, 0x9f, 0xd,  0xc8, 0x5e, 0x34,
      0x91, 0xf9, 0xe7, 0x4c, 0x1e, 0xe7, 0x2f, 0x90, 0x7f, 0xbb, 0xf8, 0x99, 0x3e, 0xc9, 0x79,
      0xab, 0x1,  0xdb, 0x24, 0x39, 0xe3, 0xb4, 0xc9, 0x52, 0x73, 0xdb, 0x65, 0x42, 0xa5, 0x2e,
      0x43, 0x56, 0xa0, 0x33, 0x8c, 0x1a, 0xb7, 0xa1, 0xed, 0x5c, 0xd0, 0x14, 0x93, 0x8d, 0x23,
      0x78, 0x93, 0xcb, 0x3a, 0x3,  0x1f, 0xbb, 0xc6, 0x7b, 0xcd, 0x51, 0x4e, 0xaa, 0x14, 0x1,
      0xe9, 0x3,  0x27, 0x13, 0xe2, 0xb2, 0xf8, 0x36, 0xc6, 0xe3, 0xc3, 0x7f, 0xb5, 0x74, 0x20,
      0x5e, 0x17, 0xaa, 0x25, 0x7,  0x9b, 0x60, 0xda, 0x83, 0x98, 0xb5, 0x55, 0xae, 0x1b, 0x7a,
      0xc1, 0x1f, 0x49, 0x72, 0xe2, 0xcb, 0x6a, 0x11, 0x77, 0xdf, 0x3f, 0xc0, 0x9f, 0x8f, 0x33,
      0xc7, 0x10, 0x17, 0x8c, 0xfc, 0xd5, 0xb7, 0x5f, 0x5e, 0xb2, 0xe3, 0x7b, 0x2e, 0xdc, 0xc7,
      0x34, 0xdb, 0x31, 0xb0, 0xdc, 0x5d, 0x14, 0x98, 0xb6, 0x1a, 0x2a, 0xd4, 0xb4, 0x4,  0x2c,
      0xf0, 0x68, 0x1c, 0x91, 0x60, 0x28, 0xa5, 0x3b, 0x1,  0x98, 0xb6, 0x1e, 0x6e, 0xaa, 0x35,
      0x89, 0xc7, 0x94, 0xaa, 0x9e, 0xf0, 0x11, 0x52, 0xf,  0x28, 0xa1, 0x3d, 0xd3, 0x17, 0xb5,
      0x8,  0xd8, 0x7a, 0x41, 0xf9, 0x7,  0xe2, 0x87, 0x36, 0xcd, 0x86, 0x3e, 0x79, 0x99, 0x73,
      0x50, 0x21, 0x30, 0x0,  0xd2, 0xf3, 0x88, 0x60, 0x32, 0x59, 0x58, 0x2f, 0x55, 0x93, 0x86,
      0x56, 0x9a, 0x96, 0xb9, 0xf8, 0xbf, 0x24, 0xc4, 0xba, 0xea, 0xa4, 0x73, 0xb0, 0xc,  0xa6,
      0xdb, 0x9,  0x2d, 0xa,  0x36, 0x3f, 0x80, 0xe6, 0x85, 0x7a, 0xf3, 0x1,  0x90, 0x3a, 0xc6,
      0xee, 0x2d, 0xa8, 0xce, 0xb4, 0x3f, 0x3a, 0xa6, 0xa3, 0xaf, 0xb9, 0x21, 0xef, 0x40, 0x6f,
      0xf4, 0x7f, 0x78, 0x25, 0x55, 0x39, 0x53, 0x67, 0x53, 0x56, 0x8d, 0x81, 0xaf, 0x63, 0x97,
      0x68, 0x86, 0x75, 0x66, 0x14, 0x1e, 0xa6, 0x63, 0x1e, 0x2,  0xd0, 0x41, 0xd8, 0x78, 0x75,
      0xd,  0x76, 0x77, 0xfa, 0x9c, 0xc5, 0xcc, 0x54, 0x6,  0x25, 0x53, 0x95, 0xeb, 0x4b, 0x7c,
      0xb4, 0xc8, 0xbb, 0x5d, 0x6b, 0x6e, 0xf0, 0xd7, 0x8d, 0x3f, 0xdf, 0x93, 0x4c, 0x30, 0x5b,
      0x2,  0xf5, 0xe,  0x49, 0x87, 0x60, 0x5f, 0x19, 0x6,  0x24, 0x3d, 0x5d, 0x97, 0x37, 0x61,
      0xef, 0x3e, 0xb,  0x9e, 0x85, 0x1c, 0x1a, 0xa6, 0x53, 0x91, 0xd2, 0x2c, 0x18, 0x7c, 0x8f,
      0x5b, 0x4a, 0xd5, 0xdd, 0xd9, 0x8a, 0xc3, 0x92, 0x19, 0x54, 0x39, 0xde, 0x33, 0xa1, 0xe1,
      0x37, 0x60, 0x3c, 0x3b, 0x3b, 0xc5, 0xed, 0x1b, 0xef, 0x28, 0xf5, 0xdf, 0x44, 0x91, 0xa3,
      0x1e, 0x69, 0x6a, 0x35, 0x85, 0x6e, 0x26, 0x46, 0x22, 0x4d, 0x87, 0x92, 0x44, 0x6b, 0x96,
      0xdb, 0x75, 0xfe, 0x76, 0x3,  0x60, 0xf7, 0xfd, 0x90, 0x55, 0x7d, 0x6e, 0xd7, 0xaa, 0x44,
      0x5,  0xc7, 0x23, 0x37, 0x12, 0xa8, 0xd4, 0xb2, 0x2b, 0xed, 0x41, 0x5f, 0x23, 0x38, 0x7c,
      0x16, 0xe6, 0x16, 0xd3, 0x10, 0x19, 0x12, 0xcc, 0x8b, 0x6e, 0xcd, 0xd6, 0xa6, 0x39, 0x8a,
      0x1b, 0x24, 0x3f, 0x4d, 0x6f, 0xa6, 0x0,  0x7c, 0xa0, 0xa1, 0x4a, 0xfd, 0xcd, 0x68, 0x50,
      0x76, 0xc8, 0x68, 0x9d, 0xeb, 0xdf, 0x24, 0x39, 0xaf, 0x77, 0xb2, 0xb6, 0xaf, 0xb6, 0x34,
      0x61, 0x37, 0x6a, 0xfd, 0xc7, 0x6d, 0x2,  0x9f, 0x29, 0xd5, 0x45, 0xf4, 0x89, 0xd8, 0x8c,
      0x5c, 0xd3, 0x31, 0xa0, 0x58, 0x19, 0x54, 0x33, 0x46, 0x92, 0xbc, 0x1e, 0x4b, 0x14, 0xac,
      0x73, 0xa5, 0x9,  0x9f, 0xb6, 0x2b, 0x2b, 0x73, 0x6b, 0x83, 0x86, 0x13, 0x6e, 0x3,  0xf7,
      0xe0, 0x7d, 0x81, 0x47, 0x18, 0x8,  0xea, 0x9,  0x10, 0x24, 0x61, 0x6d, 0x9,  0x1d, 0xb8,
      0x8e, 0xba, 0x4,  0x4d, 0xcc, 0xe6, 0xff, 0x28, 0x27, 0x86, 0x38, 0x1,  0x86, 0xbe, 0xf0,
      0x5b, 0xf8, 0x1a, 0xd6, 0xde, 0xbe, 0xf9, 0x3b, 0x76, 0x3f, 0x85, 0x82, 0x22, 0x92, 0x4b,
      0xe0, 0x76, 0x15, 0xb2, 0x57, 0x5a, 0xb0, 0x64, 0xde, 0xce, 0x93, 0xb8, 0x9f, 0x25, 0x53,
      0x8c, 0x5e, 0xdf, 0x29, 0x4e, 0x50, 0x69, 0xfb, 0x7e, 0x33, 0xcb, 0xe,  0x28, 0x1,  0x6c,
      0xab, 0xfa, 0xd8, 0x88, 0x2,  0xbc, 0xf2, 0xb1, 0xe,  0x2f, 0x6d, 0x1c, 0x8d, 0xe4, 0x11,
      0x23, 0xcc, 0x67, 0x94, 0x7b, 0xf7, 0x8a, 0xf3, 0x68, 0x52, 0xe4, 0x82, 0x25, 0x86, 0xc6,
      0x72, 0x19, 0x77, 0x80, 0x28, 0xe3, 0x86, 0xc8, 0x8a, 0xea, 0x3d, 0x54, 0x2f, 0xb,  0x64,
      0xa,  0xc5, 0x12, 0x8c, 0xb2, 0x7,  0x72, 0x1b, 0x9,  0x9f, 0x32, 0xbd, 0xa3, 0xb0, 0xc,
      0x95, 0xc8, 0x4d, 0xe5, 0xd7, 0x20, 0xdb, 0xf8, 0x34, 0x2a, 0x9d, 0x91, 0x58, 0x38, 0x7a,
      0x9c, 0xe0, 0xa3, 0xf,  0x40, 0x9d, 0xff, 0xeb, 0x4b, 0xe2, 0x16, 0x94, 0x32, 0xce, 0xe8,
      0x52, 0x75, 0x49, 0xf4, 0x71, 0x13, 0xbc, 0x59, 0x7d, 0x9a, 0xe8, 0x60, 0x29, 0x58, 0x1a,
      0x14, 0x94, 0xe6, 0x37, 0x23, 0xad, 0xfe, 0xb,  0xf0, 0x63, 0x60, 0x4f, 0x5d, 0x10, 0x91,
      0xf2, 0x50, 0x8e, 0xb,  0x4a, 0x47, 0xc9, 0xc,  0x1f, 0xdc, 0x94, 0x75, 0x25, 0x52, 0x99,
      0xfc, 0x0,  0x0,  0x0,  0x0,  0x0,  0x0,  0x0,  0x0,  0x0,  0x0,  0x0,  0x0,  0x0,  0x0,
      0x0,  0x0,
  };
  memcpy(attribute, kPermanentAttributes, sizeof(AvbAtxPermanentAttributes));
  return true;
}

// The callback returns the SHA256 hash of the permanent attributes
static bool VerifiedBootReadPermanentAttributesHash(ZirconBootOps* ops, uint8_t* hash) {
  // Pre-computed SHA256 hash for `kPermanentAttributes`
  static const uint8_t kPermanentAttributesHash[] = {
      0x55, 0x41, 0x9e, 0x1a, 0xff, 0xff, 0x15, 0x3b, 0x58, 0xf6, 0x5c,
      0xe8, 0xa5, 0x31, 0x3a, 0x71, 0xd2, 0xa8, 0x3a, 0x00, 0xd0, 0xab,
      0xae, 0x10, 0xa2, 0x5b, 0x9a, 0x8e, 0x49, 0x3d, 0x04, 0xf7,
  };
  memcpy(hash, kPermanentAttributesHash, sizeof(kPermanentAttributesHash));
  return true;
}

// See GptBootMain() for details.
ZirconBootFlags WaitForUserForceRecoveryInput(uint32_t timeout_seconds);

// Now integrate everything and start Zircon. The function should be called from the firmware main
// function
ZirconBootResult GptBootMain(void) {
  // Opaque GptData that contains GPT partitions info.
  GptData gpt_data;
  // Checks, repairs GPT on storage, and initializes `gpt_data`.
  if (!FuchsiaFirmwareStorageSyncGpt(&gpt_boot_storage, &gpt_data)) {
    printf("Failed to process GPT\n");
    // If GPT is corrupted, there's nothing we can do. Device can choose to enter a firmware
    // recovery mode such as fastboot etc from here.
    FuchsiaFirmwareStorageFreeGptData(&gpt_boot_storage, &gpt_data);
    return kBootResultErrorGeneric;
  }

  // Construct a ZirconBootOps from the implemented callbacks.
  ZirconBootOps zircon_boot_ops = {
      .context = &gpt_data,
      .read_from_partition = ReadFromPartition,
      .write_to_partition = WriteToPartition,
      .boot = Boot,
      .firmware_can_boot_kernel_slot = FirmwareCanBootKernelSlot,
      .reboot = Reboot,
      .add_zbi_items = AddZbiItems,
      .verified_boot_get_partition_size = VerifiedBootGetPartitionSize,
      .verified_boot_read_rollback_index = VerifiedBootReadRollbackIndex,
      .verified_boot_write_rollback_index = VerifiedBootWriteRollbackIndex,
      .verified_boot_read_is_device_locked = VerifiedBootReadIsDeviceLocked,
      .verified_boot_read_permanent_attributes = VerifiedBootReadPermanentAttributes,
      .verified_boot_read_permanent_attributes_hash = VerifiedBootReadPermanentAttributesHash,
      .get_kernel_load_buffer = GetKernelLoadBuffer,
  };

  // In this reference code, we assume firmware A/B/R is used and thus need to store current
  // firmware slot in `g_firmware_slot_index` so that FirmwareCanBootKernelSlot() can work
  // correctly. We assume and strongly recommend that firmware A/B/R uses the LoadAbrFirmware()
  // API for ABR firmware loading. Given this, we can call the GetActiveBootSlot() API to know the
  // firmware slot. Note that this needs to be done before any changes is made to the A/B/R
  // metadata.
  g_firmware_slot_index = GetActiveBootSlot(&zircon_boot_ops);

  // Process force recovery request. This should typically be done by listening a key press from
  // the serial, or ideally, via a physical mechanism such as button press etc. Device should
  // implement similar logic using device-specific APIs.
  uint32_t timeout_seconds = 2;
  printf("Autoboot in %u seconds. Listening for force recovery input...\n", timeout_seconds);
  ZirconBootFlags boot_flags = WaitForUserForceRecoveryInput(timeout_seconds);

  // Finally after all the preparation, we can call LoadAndBoot to boot zircon.
  ZirconBootResult res = LoadAndBoot(&zircon_boot_ops, boot_flags);

  // Should not reach here if booted successfully. In this example, boot is simply a return and we
  // need to release the gpt_data to avoid sanitizer build errors.
  FuchsiaFirmwareStorageFreeGptData(&gpt_boot_storage, &gpt_data);
  return res;
}
