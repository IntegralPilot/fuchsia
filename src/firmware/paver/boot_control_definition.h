// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_FIRMWARE_PAVER_BOOT_CONTROL_DEFINITION_H_
#define SRC_FIRMWARE_PAVER_BOOT_CONTROL_DEFINITION_H_

#include <stdint.h>

namespace paver {
namespace android {

/**
 * The A/B-specific bootloader message structure (4-KiB).
 *
 * See Android reference structure for details:
 * hardware/interfaces/boot/1.1/default/boot_control/include/private/boot_control_definition.h
 * Also can be found in Fuchsia tree:
 * //third_party/android/platform/hardware/interfaces/boot/1.1/default/boot_control/include/private/boot_control_definition.h
 * But it depends on `bootloader_message.h`, which has to be brought in if want
 * to use it.
 */
struct BootloaderMessageAB {
  // reserved 2048-byte block for BootloaderMessage
  uint8_t reserved0[2048];

  uint8_t slot_suffix[32];
  // reserved 128-byte block for update_channel
  uint8_t reserved1[128];
  // Round up the entire struct to 4096-byte.
  uint8_t reserved2[1888];
};

static_assert(sizeof(struct BootloaderMessageAB) == 4096,
              "struct BootloaderMessageAB size changes");

#define BOOT_CTRL_MAGIC 0x42414342 /* Bootloader Control AB */
#define BOOT_CTRL_VERSION 1

// Android slot meta data structure
//
// See Android reference structure for details:
// hardware/interfaces/boot/1.1/default/boot_control/include/private/boot_control_definition.h
struct SlotMetadata {
  // Slot priority with 15 meaning highest priority, 1 lowest
  // priority and 0 the slot is unbootable.
  uint8_t priority : 4;
  // Number of times left attempting to boot this slot.
  uint8_t tries_remaining : 3;
  // 1 if this slot has booted successfully, 0 otherwise.
  uint8_t successful_boot : 1;
  // 1 if this slot is corrupted from a dm-verity corruption, 0
  // otherwise.
  uint8_t verity_corrupted : 1;
  // Reserved for further use.
  uint8_t reserved : 7;
} __attribute__((packed));

// Android specific Bootloader Control AB structure
//
// See Android reference structure for details:
// hardware/interfaces/boot/1.1/default/boot_control/include/private/boot_control_definition.h
struct BootloaderControl {
  // NUL terminated active slot suffix.
  char slot_suffix[4];
  // Bootloader Control AB magic number (see BOOT_CTRL_MAGIC).
  uint32_t magic;
  // Version of struct being used (see BOOT_CTRL_VERSION).
  uint8_t version;
  // Number of slots being managed.
  uint8_t nb_slot : 3;
  // Number of times left attempting to boot recovery.
  uint8_t recovery_tries_remaining : 3;
  // Status of any pending snapshot merge of dynamic partitions.
  uint8_t merge_status : 3;
  // Ensure 4-bytes alignment for slot_info field.
  uint8_t reserved0[1];
  // Per-slot information.  Up to 4 slots.
  struct SlotMetadata slot_info[4];
  // Reserved for further use.
  uint8_t reserved1[8];
  // CRC32 of all 28 bytes preceding this field (little endian
  // format).
  uint32_t crc32_le;
} __attribute__((packed));

static_assert(sizeof(struct BootloaderControl) ==
                  sizeof(((struct BootloaderMessageAB *)0)->slot_suffix),
              "struct BootloaderControl has wrong size");

}  // namespace android
}  // namespace paver

#endif  // SRC_FIRMWARE_PAVER_BOOT_CONTROL_DEFINITION_H_
