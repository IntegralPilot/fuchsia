// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/unittest/unittest.h>

#include <fbl/alloc_checker.h>
#include <ktl/unique_ptr.h>

#include "asid_allocator.h"

namespace {

bool asid_allocator_test_inner(enum arm64_asid_width asid_width) {
  BEGIN_TEST;

  fbl::AllocChecker ac;
  ktl::unique_ptr<AsidAllocator> aa(new (&ac) AsidAllocator(asid_width));
  ASSERT_TRUE(ac.check());

  // test that it computed the correct asid width
  uint32_t max_asid = (asid_width == arm64_asid_width::ASID_8) ? MMU_ARM64_MAX_USER_ASID_8
                                                               : MMU_ARM64_MAX_USER_ASID_16;
  ASSERT_EQ(aa->max_user_asid(), max_asid);

  // run the test twice to make sure it clears back to a default state
  for (auto j = 0; j < 2; j++) {
    // use up all the asids
    for (uint32_t i = MMU_ARM64_FIRST_USER_ASID; i <= max_asid; i++) {
      auto status = aa->Alloc();
      ASSERT_TRUE(status.is_ok());

      ASSERT_GE(status.value(), MMU_ARM64_FIRST_USER_ASID);
      ASSERT_LE(status.value(), max_asid);
    }

    // expect the next one to fail
    {
      auto status = aa->Alloc();
      EXPECT_TRUE(status.is_error());
      EXPECT_EQ(status.status_value(), ZX_ERR_NO_MEMORY);
    }

    // free them all
    for (uint32_t i = MMU_ARM64_FIRST_USER_ASID; i <= max_asid; i++) {
      auto status = aa->Free(static_cast<uint16_t>(i));
      ASSERT_TRUE(status.is_ok());
    }
  }

  END_TEST;
}

bool asid_allocator_test_8bit() { return asid_allocator_test_inner(arm64_asid_width::ASID_8); }

bool asid_allocator_test_16bit() { return asid_allocator_test_inner(arm64_asid_width::ASID_16); }

UNITTEST_START_TESTCASE(asid_allocator)
UNITTEST("8 bit asid allocator test", asid_allocator_test_8bit)
UNITTEST("16 bit asid allocator test", asid_allocator_test_16bit)
UNITTEST_END_TESTCASE(asid_allocator, "asid", "Tests for asid allocator")

}  // anonymous namespace
