// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zx/event.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/object.h>

#include <fbl/vector.h>
#include <zxtest/zxtest.h>

namespace {

constexpr uint32_t kEventOption = 0u;

TEST(HandleDup, ReplaceSuccessOrigInvalid) {
  zx::event orig_event;
  ASSERT_OK(zx::event::create(kEventOption, &orig_event));

  zx::event replaced_event;
  ASSERT_OK(orig_event.replace(ZX_RIGHTS_BASIC, &replaced_event));
  EXPECT_FALSE(orig_event.is_valid());
  EXPECT_TRUE(replaced_event.is_valid());
}

TEST(HandleDup, ReplaceFailureBothInvalid) {
  zx::event orig_event;
  ASSERT_OK(zx::event::create(kEventOption, &orig_event));

  zx::event failed_event;
  EXPECT_EQ(orig_event.replace(ZX_RIGHT_EXECUTE, &failed_event), ZX_ERR_INVALID_ARGS);
  // Even on failure, a replaced object is now invalid.
  EXPECT_FALSE(orig_event.is_valid());
  EXPECT_FALSE(failed_event.is_valid());
}
}  // namespace
