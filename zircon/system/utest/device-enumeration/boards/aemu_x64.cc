// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "zircon/system/utest/device-enumeration/common.h"

namespace {

TEST_F(DeviceEnumerationTest, AemuX64Test) {
  static const char* kNodeMonikers[] = {
      "acpi",
      "PCI0.bus.00_1f_2.00_1f_2.ahci",
      "acpi._SB_.PCI0.ISA_.KBD_.pt.KBD_-composite-spec.i8042.i8042-keyboard",
      "acpi._SB_.PCI0.ISA_.KBD_.pt.KBD_-composite-spec.i8042.i8042-mouse",
  };
  VerifyNodes(kNodeMonikers);

  static const char* kAemuNodeMonikers[] = {
      "PCI0.bus.00_0b_0.00_0b_0.goldfish-address-space",

      "acpi._SB_.GFPP.pt.GFPP-composite-spec.goldfish-pipe",
      "acpi._SB_.GFPP.pt.GFPP-composite-spec.goldfish-pipe-control",
      "acpi._SB_.GFSK.pt.GFSK-composite-spec.goldfish-sync",

      "acpi._SB_.GFPP.pt.GFPP-composite-spec.goldfish-pipe-control.goldfish-control-2.goldfish-control",
      "acpi._SB_.GFPP.pt.GFPP-composite-spec.goldfish-pipe-control.goldfish-control-2.goldfish-control.goldfish-display",
      "acpi._SB_.GFPP.pt.GFPP-composite-spec.goldfish-pipe-control.goldfish-control-2",
  };

  VerifyNodes(kAemuNodeMonikers);
}

}  // namespace
