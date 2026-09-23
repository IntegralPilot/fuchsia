// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "zircon/system/utest/device-enumeration/common.h"

namespace {

TEST_F(DeviceEnumerationTest, QemuX64Test) {
  const char* kNodeMonikers[] = {
      "acpi",
      "PCI0.bus.00_1f_2.00_1f_2.ahci",
      // TODO(https://fxbug.dev/42075162): Re-enable with new names after QEMU roll
      //"acpi._SB_.PCI0.ISA_.KBD_.pt.KBD_-composite-spec.i8042.i8042-keyboard",
      //"acpi._SB_.PCI0.ISA_.KBD_.pt.KBD_-composite-spec.i8042.i8042-mouse",
  };

  VerifyNodes(kNodeMonikers);
}

}  // namespace
