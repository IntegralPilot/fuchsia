// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zxtest/zxtest.h>

#include "../test/safe-zero-construction.h"
#include "log.h"

namespace {

[[maybe_unused]] constinit LIBC_NAMESPACE::Log gLogIsConstinit;

TEST(LibcLogTests, SafeZeroConstruction) {
  LIBC_NAMESPACE::ExpectSafeZeroConstruction<LIBC_NAMESPACE::Log>();
}

}  // namespace
