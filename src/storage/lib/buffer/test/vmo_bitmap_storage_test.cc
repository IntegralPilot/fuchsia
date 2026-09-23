// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/buffer/vmo_bitmap_storage.h"

#include <lib/zx/vmo.h>
#include <zircon/errors.h>
#include <zircon/syscalls.h>
#include <zircon/types.h>

#include <cstddef>
#include <cstring>
#include <utility>

#include <bitmap/raw-bitmap.h>
#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"

namespace storage {
namespace {

TEST(VmoBitmapStorageTest, RawBitmapGenericAllocateAndAccess) {
  bitmap::RawBitmapGeneric<VmoBitmapStorage> bitmap;
  const size_t kBits = 2048;
  ASSERT_OK(bitmap.Reset(kBits));
  EXPECT_EQ(kBits, bitmap.size());
  EXPECT_TRUE(bitmap.StorageUnsafe()->vmo().is_valid());

  EXPECT_OK(bitmap.Set(0, 10));
  EXPECT_OK(bitmap.Set(100, 150));

  size_t next = 0;
  EXPECT_TRUE(bitmap.Get(0, 10, &next));
  EXPECT_EQ(10u, next);
  EXPECT_FALSE(bitmap.Get(10, 90, &next));
  EXPECT_EQ(10u, next);
  EXPECT_TRUE(bitmap.Get(100, 150, &next));
  EXPECT_EQ(150u, next);

  EXPECT_OK(bitmap.SetOne(500));
  EXPECT_TRUE(bitmap.GetOne(500));
  EXPECT_OK(bitmap.ClearOne(500));
  EXPECT_FALSE(bitmap.GetOne(500));
}

TEST(VmoBitmapStorageTest, RawBitmapGenericGrow) {
  bitmap::RawBitmapGeneric<VmoBitmapStorage> bitmap;
  const size_t kInitialBits = 4096;
  ASSERT_OK(bitmap.Reset(kInitialBits));
  EXPECT_OK(bitmap.Set(0, 100));
  EXPECT_OK(bitmap.Set(4000, 4096));

  const size_t kGrownBits = 100000;
  ASSERT_OK(bitmap.Grow(kGrownBits));
  EXPECT_EQ(kGrownBits, bitmap.size());

  size_t next = 0;
  EXPECT_TRUE(bitmap.Get(0, 100, &next));
  EXPECT_TRUE(bitmap.Get(4000, 4096, &next));

  EXPECT_FALSE(bitmap.Get(4096, kGrownBits, &next));
  EXPECT_EQ(4096u, next);

  EXPECT_OK(bitmap.Set(50000, 51000));
  EXPECT_TRUE(bitmap.Get(50000, 51000, &next));
}

TEST(VmoBitmapStorageTest, RawBitmapGenericMove) {
  bitmap::RawBitmapGeneric<VmoBitmapStorage> bitmap1;
  const size_t kBits = 1024;
  ASSERT_OK(bitmap1.Reset(kBits));
  EXPECT_OK(bitmap1.Set(10, 20));

  bitmap::RawBitmapGeneric<VmoBitmapStorage> bitmap2 = std::move(bitmap1);
  EXPECT_EQ(kBits, bitmap2.size());
  EXPECT_TRUE(bitmap2.StorageUnsafe()->vmo().is_valid());

  size_t next = 0;
  EXPECT_TRUE(bitmap2.Get(10, 20, &next));

  bitmap::RawBitmapGeneric<VmoBitmapStorage> bitmap3;
  bitmap3 = std::move(bitmap2);
  EXPECT_EQ(kBits, bitmap3.size());
  EXPECT_TRUE(bitmap3.Get(10, 20, &next));
}

}  // namespace
}  // namespace storage
