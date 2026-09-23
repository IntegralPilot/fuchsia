// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/buffer/mapped_vmo.h"

#include <lib/zx/vmo.h>
#include <zircon/errors.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/object.h>
#include <zircon/types.h>

#include <cstddef>
#include <cstdint>
#include <cstring>
#include <limits>
#include <utility>

#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"

namespace storage {
namespace {

TEST(MappedVmoTest, Uninitialized) {
  MappedVmo vmo;
  EXPECT_EQ(0u, vmo.size());
  EXPECT_EQ(nullptr, vmo.start());
  EXPECT_FALSE(vmo.vmo().is_valid());

  EXPECT_STATUS(ZX_ERR_BAD_STATE, vmo.Grow(zx_system_get_page_size()));
  EXPECT_STATUS(ZX_ERR_BAD_STATE, vmo.Shrink(zx_system_get_page_size()));
}

TEST(MappedVmoTest, CreateAndMap) {
  MappedVmo vmo;
  EXPECT_STATUS(ZX_ERR_INVALID_ARGS, vmo.CreateAndMap(0, "test"));

  const size_t kSize = zx_system_get_page_size() * 2;
  ASSERT_OK(vmo.CreateAndMap(kSize, "test-vmo"));
  EXPECT_EQ(kSize, vmo.size());
  EXPECT_NE(nullptr, vmo.start());
  EXPECT_TRUE(vmo.vmo().is_valid());

  char name[ZX_MAX_NAME_LEN] = {};
  EXPECT_OK(vmo.vmo().get_property(ZX_PROP_NAME, name, sizeof(name)));
  EXPECT_STREQ("test-vmo", name);

  EXPECT_STATUS(ZX_ERR_BAD_STATE, vmo.CreateAndMap(kSize, "another"));
}

TEST(MappedVmoTest, Grow) {
  MappedVmo vmo;
  const size_t kPageSize = zx_system_get_page_size();
  const size_t kInitialSize = kPageSize * 2;
  ASSERT_OK(vmo.CreateAndMap(kInitialSize, "grow-test"));

  memset(vmo.start(), 0xaa, kInitialSize);

  EXPECT_STATUS(ZX_ERR_INVALID_ARGS, vmo.Grow(kPageSize));
  EXPECT_OK(vmo.Grow(kInitialSize));
  EXPECT_EQ(kInitialSize, vmo.size());

  const size_t kGrownSize = kPageSize * 4;
  ASSERT_OK(vmo.Grow(kGrownSize));
  EXPECT_EQ(kGrownSize, vmo.size());
  EXPECT_NE(nullptr, vmo.start());

  const uint8_t* ptr = reinterpret_cast<const uint8_t*>(vmo.start());
  for (size_t i = 0; i < kInitialSize; ++i) {
    EXPECT_EQ(0xaa, ptr[i]);
  }
}

TEST(MappedVmoTest, Shrink) {
  MappedVmo vmo;
  const size_t kPageSize = zx_system_get_page_size();
  const size_t kInitialSize = kPageSize * 4;
  ASSERT_OK(vmo.CreateAndMap(kInitialSize, "shrink-test"));

  memset(vmo.start(), 0xcc, kInitialSize);

  EXPECT_STATUS(ZX_ERR_INVALID_ARGS, vmo.Shrink(kInitialSize * 2));
  EXPECT_OK(vmo.Shrink(kInitialSize));
  EXPECT_EQ(kInitialSize, vmo.size());

  void* original_start = vmo.start();
  const size_t kShrunkSize = kPageSize;
  ASSERT_OK(vmo.Shrink(kShrunkSize));
  EXPECT_EQ(kShrunkSize, vmo.size());
  EXPECT_EQ(original_start, vmo.start());

  const uint8_t* ptr = reinterpret_cast<const uint8_t*>(vmo.start());
  for (size_t i = 0; i < kShrunkSize; ++i) {
    EXPECT_EQ(0xcc, ptr[i]);
  }

  zx_info_vmo_t info;
  ASSERT_OK(vmo.vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  EXPECT_EQ(kShrunkSize, info.committed_bytes);

  EXPECT_STATUS(ZX_ERR_INVALID_ARGS, vmo.Shrink(0));
}

TEST(MappedVmoTest, Move) {
  MappedVmo vmo1;
  const size_t kSize = zx_system_get_page_size() * 2;
  ASSERT_OK(vmo1.CreateAndMap(kSize, "move-test"));

  MappedVmo vmo2 = std::move(vmo1);
  EXPECT_EQ(0u, vmo1.size());
  EXPECT_EQ(nullptr, vmo1.start());
  EXPECT_FALSE(vmo1.vmo().is_valid());

  EXPECT_EQ(kSize, vmo2.size());
  EXPECT_NE(nullptr, vmo2.start());
  EXPECT_TRUE(vmo2.vmo().is_valid());

  MappedVmo vmo3;
  vmo3 = std::move(vmo2);
  EXPECT_EQ(0u, vmo2.size());
  EXPECT_FALSE(vmo2.vmo().is_valid());

  EXPECT_EQ(kSize, vmo3.size());
  EXPECT_TRUE(vmo3.vmo().is_valid());

  MappedVmo vmo_dest;
  ASSERT_OK(vmo_dest.CreateAndMap(zx_system_get_page_size(), "dest-vmo"));
  vmo_dest = std::move(vmo3);
  EXPECT_EQ(0u, vmo3.size());
  EXPECT_FALSE(vmo3.vmo().is_valid());
  EXPECT_EQ(kSize, vmo_dest.size());
  EXPECT_TRUE(vmo_dest.vmo().is_valid());
}

TEST(MappedVmoTest, PageAlignment) {
  MappedVmo vmo;
  const size_t kPageSize = zx_system_get_page_size();
  ASSERT_OK(vmo.CreateAndMap(1, "align-test"));
  EXPECT_EQ(kPageSize, vmo.size());

  ASSERT_OK(vmo.Grow(kPageSize + 1));
  EXPECT_EQ(kPageSize * 2, vmo.size());

  ASSERT_OK(vmo.Shrink(kPageSize + 1));
  EXPECT_EQ(kPageSize * 2, vmo.size());

  ASSERT_OK(vmo.Shrink(kPageSize));
  EXPECT_EQ(kPageSize, vmo.size());

  EXPECT_STATUS(ZX_ERR_INVALID_ARGS, vmo.Grow(std::numeric_limits<size_t>::max()));

  MappedVmo overflow_vmo;
  EXPECT_STATUS(ZX_ERR_INVALID_ARGS,
                overflow_vmo.CreateAndMap(std::numeric_limits<size_t>::max(), "overflow"));
}

}  // namespace
}  // namespace storage
