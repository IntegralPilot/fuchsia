// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fidl/internal.h>
#include <lib/zx/event.h>

#include <array>

#include <zxtest/zxtest.h>

namespace {

TEST(CloseMany, FidlHandleInfoCloseMany) {
  zx_handle_info_t handle_info[1] = {};
  EXPECT_OK(zx_event_create(0, &handle_info[0].handle));
  EXPECT_OK(FidlHandleInfoCloseMany(handle_info, std::size(handle_info)));
  EXPECT_EQ(ZX_ERR_BAD_HANDLE, zx_handle_close(handle_info[0].handle));
}

}  // namespace
