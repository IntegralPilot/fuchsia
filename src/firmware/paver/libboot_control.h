// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_FIRMWARE_PAVER_LIBBOOT_CONTROL_H_
#define SRC_FIRMWARE_PAVER_LIBBOOT_CONTROL_H_

#include <cstddef>

#include "src/firmware/paver/boot_control_definition.h"

namespace paver {
namespace android {

class IoOps {
 public:
  virtual int read(void* data, uint64_t offset, size_t len) const = 0;
  virtual int write(const void* data, uint64_t offset, size_t len) const = 0;
};

// Helper library to read/write bootcontrol structure
// There are different implementations and metadata is not necessarily located in A/B/R meta
// partition. This class would help to extend support for this cases.
class BootControl {
 public:
  BootControl(IoOps& ops) : ops_(ops) { (void)this->ops_; }

  bool Load(BootloaderControl* buffer);
  bool UpdateAndSave(BootloaderControl* buffer);

 private:
  IoOps& ops_;
};

}  // namespace android
}  // namespace paver

#endif  // SRC_FIRMWARE_PAVER_LIBBOOT_CONTROL_H_
