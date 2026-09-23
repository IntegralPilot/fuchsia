// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/errors.h>

#include <gtest/gtest.h>

#include "src/firmware/lib/fastboot/rust/ffi_c/bindings.h"

namespace {

TEST(RustTest, InstallFromUsbNonexistentDisk) {
  // Given non-existent paths for installation, function should fail.
  ASSERT_NE(install_from_usb("foo/bar", "/baz"), ZX_OK);
}

// TODO: is it feasible to register fake block devices to test against?

}  // namespace
