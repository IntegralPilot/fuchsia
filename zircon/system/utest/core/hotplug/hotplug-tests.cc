// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/standalone-test/standalone.h>
#include <lib/zx/resource.h>
#include <lib/zx/result.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/resource.h>

#include <string_view>

#include <zxtest/zxtest.h>

namespace {

// We invoke the in-kernel hotplug unit tests here to ensure that they do
// indeed still work at the time that userspace is up and running (which has
// been a point of regression in the past).
TEST(HotplugTests, KHotplugTest) {
  zx::resource debug_rsrc;
  {
    zx::unowned_resource system_rsrc = standalone::GetSystemResource();
    zx::result<zx::resource> result =
        standalone::GetSystemResourceWithBase(system_rsrc, ZX_RSRC_SYSTEM_DEBUG_BASE);
    ASSERT_TRUE(result.is_ok());
    debug_rsrc = std::move(result.value());
  }

  // "k - Cmd" - get it??
  constexpr std::string_view kCmd = "ut hotplug";
  ASSERT_EQ(ZX_OK, zx_debug_send_command(debug_rsrc.release(), kCmd.data(), kCmd.size()));
}
}  // namespace
