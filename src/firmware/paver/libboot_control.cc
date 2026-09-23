// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/firmware/paver/libboot_control.h"

#include <endian.h>
#include <fcntl.h>
#include <lib/cksum.h>

#include "src/firmware/paver/boot_control_definition.h"
#include "src/firmware/paver/pave-logging.h"

namespace paver {
namespace android {

constexpr off_t kBootloaderControlOffset = offsetof(BootloaderMessageAB, slot_suffix);

void UpdateCrcLe(BootloaderControl* boot_ctrl) {
  boot_ctrl->crc32_le = htole32(
      crc32(0, reinterpret_cast<const uint8_t*>(boot_ctrl), offsetof(BootloaderControl, crc32_le)));
}

bool BootControl::Load(BootloaderControl* buffer) {
  int res = ops_.read(buffer, kBootloaderControlOffset, sizeof(BootloaderControl));
  if (res != 0) {
    ERROR("failed to read (%i)\n", res);
    return false;
  }
  return true;
}

bool BootControl::UpdateAndSave(BootloaderControl* buffer) {
  UpdateCrcLe(buffer);
  int res = ops_.write(buffer, kBootloaderControlOffset, sizeof(BootloaderControl));
  if (res != 0) {
    ERROR("failed to write (%i)\n", res);
    return false;
  }
  return true;
}

}  // namespace android
}  // namespace paver
