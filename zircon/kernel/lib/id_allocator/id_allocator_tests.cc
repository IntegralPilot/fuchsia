// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/fit/defer.h>
#include <lib/unittest/unittest.h>

#include "lib/id_allocator.h"

static bool id_allocator_alloc_and_free() {
  BEGIN_TEST;

  constexpr uint8_t kMaxId = sizeof(size_t);
  constexpr uint8_t kMinId = 1;
  id_allocator::IdAllocator<uint8_t, UINT8_MAX - 1, kMinId> allocator;

  // Reset to invalid value, before using a valid value.
  auto result = allocator.Reset(kMinId);
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, result.status_value());
  result = allocator.Reset(UINT8_MAX);
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, result.status_value());
  result = allocator.Reset(kMaxId);
  EXPECT_EQ(ZX_OK, result.status_value());

  // Allocate all IDs.
  for (uint8_t i = kMinId; i < kMaxId; i++) {
    auto id = allocator.TryAlloc();
    ASSERT_EQ(ZX_OK, id.status_value());
    EXPECT_EQ(i, *id);
  }

  // Allocate when no IDs are free.
  auto id = allocator.TryAlloc();
  EXPECT_EQ(ZX_ERR_NO_RESOURCES, id.status_value());

  // Free an ID that was just allocated.
  constexpr uint8_t kFreeId = kMaxId / 2;
  result = allocator.Free(kFreeId);
  EXPECT_EQ(ZX_OK, result.status_value());

  // Free an ID that was already freed.
  result = allocator.Free(kFreeId);
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, result.status_value());

  // Free an invalid ID.
  result = allocator.Free(kMaxId + 1);
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, result.status_value());

  END_TEST;
}

UNITTEST_START_TESTCASE(id_allocator_tests)
UNITTEST("id_allocator_alloc_and_free", id_allocator_alloc_and_free)
UNITTEST_END_TESTCASE(id_allocator_tests, "id_allocator", "id_allocator tests")
