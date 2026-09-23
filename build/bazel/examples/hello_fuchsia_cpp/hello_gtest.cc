// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fdio/spawn.h>
#include <lib/zx/process.h>

#include <iostream>

#include <gtest/gtest.h>

// Demonstrate some basic assertions.
TEST(HelloTest, BasicAssertions) {
  std::cout << "Example stdout." << std::endl;

  // Expect two strings not to be equal.
  EXPECT_STRNE("hello", "world");
  // Expect equality.
  EXPECT_EQ(7 * 6, 42);
}

void subprocess(const char* bin_path, int64_t* retval) {
  zx::process process;
  const char* argv[] = {bin_path, nullptr};
  zx_status_t status = fdio_spawn(ZX_HANDLE_INVALID, FDIO_SPAWN_CLONE_ALL, bin_path, argv,
                                  process.reset_and_get_address());
  ASSERT_EQ(status, ZX_OK);
  status = process.wait_one(ZX_TASK_TERMINATED, zx::time::infinite(), nullptr);
  ASSERT_EQ(status, ZX_OK);
  zx_info_process_t proc_info{};
  status = process.get_info(ZX_INFO_PROCESS, &proc_info, sizeof(proc_info), nullptr, nullptr);
  ASSERT_EQ(status, ZX_OK);
  *retval = proc_info.return_code;
}

#if defined(__has_feature)
#if __has_feature(address_sanitizer) || __has_feature(undefined_behavior_sanitizer)
#include <zircon/sanitizer.h>
void SanLog(std::string_view s) { __sanitizer_log_write(s.data(), s.size()); }
#endif
#endif

#if defined(__has_feature)
#if __has_feature(address_sanitizer)
TEST(HelloTest, AddressSanitizerWorks) {
  SanLog("[===ASAN EXCEPT BLOCK START===]");
  ASSERT_DEATH(
      {
        // Volatile to suppress optimizations
        volatile int* p = new int;
        delete p;
        // Use after free!
        *p = 42;
      },
      "");
  SanLog("[===ASAN EXCEPT BLOCK END===]");
}
#endif
#endif

#if defined(__has_feature)
#if __has_feature(undefined_behavior_sanitizer)
#include <limits.h>
#include <stdio.h>

TEST(HelloTest, UndefinedBehaviorSanitizerWorks) {
  SanLog("[===UBSAN EXCEPT BLOCK START===]");
  ASSERT_DEATH(
      {
        int overflow = INT_MAX;
        // Integer overflow!
        overflow += 1;
        // Side effect to suppress optimizations.
        printf("overflow = %i\n", overflow);
      },
      "");
  SanLog("[===UBSAN EXCEPT BLOCK END===]");
}
#endif
#endif
