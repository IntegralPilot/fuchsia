// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <string.h>
#include <sys/personality.h>
#include <sys/utsname.h>

#include <gtest/gtest.h>

TEST(PersonalityTest, UnameMachineName) {
  struct utsname name;

  int orig_pers = personality(0xffffffff);
  ASSERT_NE(orig_pers, -1);

  // Set personality to PER_LINUX
  int prev = personality(PER_LINUX);
  ASSERT_NE(prev, -1);

  ASSERT_EQ(uname(&name), 0);
#if defined(__aarch64__) || defined(__arm__)
  EXPECT_STREQ(name.machine, "aarch64");
#elif defined(__x86_64__) || defined(__i386__)
  EXPECT_STREQ(name.machine, "x86_64");
#elif defined(__riscv)
  EXPECT_STREQ(name.machine, "riscv64");
#endif

  // Set personality to PER_LINUX32
  prev = personality(PER_LINUX32);
  ASSERT_NE(prev, -1);

  ASSERT_EQ(uname(&name), 0);
#if defined(__aarch64__) || defined(__arm__)
  EXPECT_STREQ(name.machine, "armv8l");
#elif defined(__x86_64__) || defined(__i386__)
  EXPECT_STREQ(name.machine, "i686");
#elif defined(__riscv)
  EXPECT_STREQ(name.machine, "riscv32");
#endif

  // Restore
  personality(orig_pers);
}
