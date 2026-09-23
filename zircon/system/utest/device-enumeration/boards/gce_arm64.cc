// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "zircon/system/utest/device-enumeration/common.h"

namespace {

TEST_F(DeviceEnumerationTest, GceArm64Test) {
#ifdef __aarch64__
  static const char* kNodeMonikers[] = {
      // TODO(https://fxbug.dev/42052364): Once we use userspace PCI, add PCI devices we expect to
      // see.
      "acpi",
      "acpi._SB_",
  };

  VerifyNodes(kNodeMonikers);
  return;
#endif
  ASSERT_TRUE(false, "GCE board not ARM64.");
}

}  // namespace
