// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/assert.h>

#include <memory>

#include <fbl/alloc_checker.h>
#include <zxtest/zxtest.h>

// These test verify behavior of AllocChecker that cannot easily be verified by its main in-kernel
// test suite, //zircon/kernel/tests/alloc_checker_tests.cc.  Prefer to add tests there instead.

namespace {

#if ZX_DEBUG_ASSERT_IMPLEMENTED

TEST(AllocCheckerTests, PanicIfDestroyedWhenArmed) {
  ASSERT_DEATH(
      []() {
        fbl::AllocChecker ac;
        ac.arm(1, false);
      },
      "AllocChecker should have panicked because it was destroyed while armed");
}

TEST(AllocCheckerTests, PanicIfReusedWhenArmed) {
  fbl::AllocChecker ac;
  ac.arm(1, false);
  ASSERT_DEATH([&ac]() { ac.arm(1, false); },
               "AllocChecker should have panicked because it was used while armed");
  ASSERT_FALSE(ac.check());
}

#else

TEST(AllocCheckerTests, DontPanicIfDestroyedWhenArmed) {
  fbl::AllocChecker ac;
  ac.arm(1, false);
}

TEST(AllocCheckerTests, DontPanicIfReusedWhenArmed) {
  fbl::AllocChecker ac;
  ac.arm(1, false);
  ac.arm(1, false);
  ASSERT_FALSE(ac.check());
}

#endif

TEST(AllocCheckerTests, ArmWithSmartPointer) {
  std::unique_ptr<int> ptr;
  fbl::AllocChecker ac;
  ac.arm(sizeof(*ptr), ptr);
  ASSERT_FALSE(ac.check());
}

TEST(AllocCheckerTests, MakeUniqueChecked) {
  fbl::AllocChecker ac;
  std::unique_ptr<int> ptr = fbl::make_unique_checked<int>(ac, 23);
  ASSERT_TRUE(ac.check());
  EXPECT_EQ(*ptr, 23);
}

TEST(AllocCheckerTests, MakeUniqueCheckedArray) {
  fbl::AllocChecker ac;
  std::unique_ptr<int[]> ptr = fbl::make_unique_checked<int[]>(ac, 2);
  ASSERT_TRUE(ac.check());
  EXPECT_EQ(ptr[0], 0);
  EXPECT_EQ(ptr[1], 0);
}

TEST(AllocCheckerTests, MakeUniqueForOverwriteChecked) {
  fbl::AllocChecker ac;
  std::unique_ptr<int> ptr = fbl::make_unique_for_overwrite_checked<int>(ac);
  ASSERT_TRUE(ac.check());
  *ptr = 23;
  EXPECT_EQ(*ptr, 23);
}

TEST(AllocCheckerTests, MakeUniqueForOverwriteCheckedArray) {
  fbl::AllocChecker ac;
  std::unique_ptr<int[]> ptr = fbl::make_unique_for_overwrite_checked<int[]>(ac, 2);
  ASSERT_TRUE(ac.check());
  ptr[0] = 17;
  ptr[1] = 23;
  EXPECT_EQ(ptr[0], 17);
  EXPECT_EQ(ptr[1], 23);
}

}  // namespace
