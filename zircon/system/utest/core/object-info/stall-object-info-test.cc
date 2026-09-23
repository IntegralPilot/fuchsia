// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/maybe-standalone-test/maybe-standalone.h>
#include <zircon/syscalls/object.h>

#include <zxtest/zxtest.h>

#include "helper.h"

namespace object_info_test {
namespace {

class StallGetInfoTest : public zxtest::Test {
 public:
  static void SetUpTestSuite() {
    zx::unowned_resource system_resource = maybe_standalone::GetSystemResource();
    if (!system_resource->is_valid()) {
      ZXTEST_SKIP("System resource not available, skipping\n");
    }

    ASSERT_OK(zx::resource::create(*system_resource, ZX_RSRC_KIND_SYSTEM, ZX_RSRC_SYSTEM_STALL_BASE,
                                   1, nullptr, 0, &stall_resource_));
  }

  static const zx::resource& handle_provider() { return stall_resource_; }

 private:
  static zx::resource stall_resource_;
};

zx::resource StallGetInfoTest::stall_resource_;

TEST_F(StallGetInfoTest, MemoryStallSmokeTest) {
  CheckSelfInfoSucceeds<zx_info_memory_stall_t>(ZX_INFO_MEMORY_STALL, handle_provider());
}

TEST_F(StallGetInfoTest, MemoryStallInvalidHandleFails) {
  ASSERT_NO_FATAL_FAILURE(
      (CheckInvalidHandleFails<zx_info_memory_stall_t>(ZX_INFO_MEMORY_STALL, handle_provider)));
}

TEST_F(StallGetInfoTest, MemoryStallNullAvailSucceeds) {
  ASSERT_NO_FATAL_FAILURE(
      CheckNullAvailSucceeds<zx_info_memory_stall_t>(ZX_INFO_MEMORY_STALL, handle_provider));
}

TEST_F(StallGetInfoTest, MemoryStallNullActualSucceeds) {
  ASSERT_NO_FATAL_FAILURE(
      CheckNullActualSucceeds<zx_info_memory_stall_t>(ZX_INFO_MEMORY_STALL, handle_provider));
}

TEST_F(StallGetInfoTest, MemoryStallNullActualAndAvailSucceeds) {
  ASSERT_NO_FATAL_FAILURE(CheckNullActualAndAvailSucceeds<zx_info_memory_stall_t>(
      ZX_INFO_MEMORY_STALL, handle_provider));
}

TEST_F(StallGetInfoTest, MemoryStallInvalidBufferPointerFails) {
  ASSERT_NO_FATAL_FAILURE((CheckInvalidBufferPointerFails<zx_info_memory_stall_t>(
      ZX_INFO_MEMORY_STALL, handle_provider)));
}

TEST_F(StallGetInfoTest, MemoryStallBadActualIsInvalidArg) {
  ASSERT_NO_FATAL_FAILURE(
      BadActualIsInvalidArgs<zx_info_memory_stall_t>(ZX_INFO_MEMORY_STALL, handle_provider));
}

TEST_F(StallGetInfoTest, MemoryStallBadAvailIsInvalidArg) {
  ASSERT_NO_FATAL_FAILURE(
      BadAvailIsInvalidArgs<zx_info_memory_stall_t>(ZX_INFO_MEMORY_STALL, handle_provider));
}

TEST_F(StallGetInfoTest, MemoryStallZeroSizedBufferIsTooSmall) {
  ASSERT_NO_FATAL_FAILURE(
      (CheckZeroSizeBufferFails<zx_info_memory_stall_t>(ZX_INFO_MEMORY_STALL, handle_provider)));
}

}  // namespace
}  // namespace object_info_test
