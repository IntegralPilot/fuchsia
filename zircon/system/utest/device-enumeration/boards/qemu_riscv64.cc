// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "zircon/system/utest/device-enumeration/common.h"

namespace {

TEST_F(DeviceEnumerationTest, QemuRiscv64Test) {
  // clang-format off
  static const char* kNodeMonikers[] = {
      "goldfish-rtc",        // goldfish-rtc
      "PCI0.bus.00_00_0", // host bridge
      "qemu-riscv64",     // board driver
  };
  // clang-format on

  VerifyNodes(kNodeMonikers);
}

}  // namespace
