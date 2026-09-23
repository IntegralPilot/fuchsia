// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/screenshot/util.h"

#include <gtest/gtest.h>

namespace screenshot {
namespace test {

TEST(UtilTest, GenerateReadableVmoTetst) {
  size_t vmo_test_size = 4096u;
  zx::vmo unreadable_vmo;

  EXPECT_EQ(zx::vmo::create(vmo_test_size, 0, &unreadable_vmo), ZX_OK);
  EXPECT_EQ(unreadable_vmo.set_cache_policy(ZX_CACHE_POLICY_UNCACHED_DEVICE), ZX_OK);

  std::vector<uint8_t> buf(1);

  // Confirm that unreadable_vmo is NOT readable.
  EXPECT_NE(unreadable_vmo.read(buf.data(), 0, 1), ZX_OK);

  size_t unreadable_vmo_size;
  EXPECT_EQ(unreadable_vmo.get_size(&unreadable_vmo_size), ZX_OK);
  EXPECT_EQ(unreadable_vmo_size, vmo_test_size);

  zx::vmo readable_vmo;
  EXPECT_EQ(GenerateReadableVmo(std::move(unreadable_vmo), vmo_test_size, &readable_vmo), ZX_OK);

  size_t readable_vmo_size;
  EXPECT_EQ(readable_vmo.get_size(&readable_vmo_size), ZX_OK);
  EXPECT_EQ(readable_vmo_size, unreadable_vmo_size);

  std::vector<uint8_t> read_buf(1);
  // Confirm readable_vmo is readable.
  EXPECT_EQ(readable_vmo.read(read_buf.data(), 0, 1), ZX_OK);
}
}  // namespace test
}  // namespace screenshot
