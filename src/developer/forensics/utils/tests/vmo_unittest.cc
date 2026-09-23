// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/utils/vmo.h"

#include <lib/zx/vmo.h>

#include <string>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

namespace forensics {
namespace {

TEST(VmoTest, StringFromVmoSuccess) {
  const std::string input = "Hello, world!";
  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(input.size(), /*options=*/0, &vmo), ZX_OK);
  ASSERT_EQ(vmo.write(input.data(), /*offset=*/0, input.size()), ZX_OK);

  const zx::result<std::string> result = StringFromVmo(vmo);
  ASSERT_TRUE(result.is_ok());
  EXPECT_EQ(*result, input);
}

TEST(VmoTest, StringFromVmoEmpty) {
  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(/*size=*/0, /*options=*/0, &vmo), ZX_OK);

  const zx::result<std::string> result = StringFromVmo(vmo);
  ASSERT_TRUE(result.is_ok());
  EXPECT_TRUE(result->empty());
}

TEST(VmoTest, StringFromVmoUnownedVmo) {
  const std::string input = "Hello, world!";
  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(input.size(), /*options=*/0, &vmo), ZX_OK);
  ASSERT_EQ(vmo.write(input.data(), /*offset=*/0, input.size()), ZX_OK);

  const zx::result<std::string> result = StringFromVmo(vmo.borrow());
  ASSERT_TRUE(result.is_ok());
  EXPECT_EQ(*result, input);
}

TEST(VmoTest, StringFromVmoInvalidVmo) {
  const zx::vmo invalid_vmo;
  const zx::result<std::string> result = StringFromVmo(invalid_vmo);
  ASSERT_TRUE(result.is_error());
  EXPECT_EQ(result.error_value(), ZX_ERR_BAD_HANDLE);
}

TEST(VmoTest, StringFromVmoNoReadRight) {
  const std::string input = "Hello, world!";
  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(input.size(), /*options=*/0, &vmo), ZX_OK);
  ASSERT_EQ(vmo.write(input.data(), /*offset=*/0, input.size()), ZX_OK);

  zx::vmo no_read_vmo;
  ASSERT_EQ(vmo.duplicate(ZX_RIGHT_WRITE, &no_read_vmo), ZX_OK);

  const zx::result<std::string> result = StringFromVmo(no_read_vmo);
  ASSERT_TRUE(result.is_error());
  EXPECT_EQ(result.error_value(), ZX_ERR_ACCESS_DENIED);
}

TEST(VmoTest, VmoFromStringSuccess) {
  const std::string input = "Hello, world!";
  const zx::result<zx::vmo> vmo = VmoFromString(input);
  ASSERT_TRUE(vmo.is_ok());
  ASSERT_TRUE(vmo->is_valid());

  uint64_t size;
  ASSERT_EQ(vmo->get_stream_size(&size), ZX_OK);
  EXPECT_EQ(size, input.size());

  std::string read_back(size, '\0');
  ASSERT_EQ(vmo->read(read_back.data(), /*offset=*/0, size), ZX_OK);
  EXPECT_EQ(read_back, input);
}

TEST(VmoTest, VmoFromStringEmpty) {
  const zx::result<zx::vmo> vmo = VmoFromString("");
  ASSERT_TRUE(vmo.is_ok());
  ASSERT_TRUE(vmo->is_valid());

  uint64_t size;
  ASSERT_EQ(vmo->get_stream_size(&size), ZX_OK);
  EXPECT_EQ(size, 0u);
}

TEST(VmoTest, RoundTrip) {
  const std::string input = "Hello, world!";
  const zx::result<zx::vmo> vmo = VmoFromString(input);
  ASSERT_TRUE(vmo.is_ok());

  const zx::result<std::string> output = StringFromVmo(*vmo);
  ASSERT_TRUE(output.is_ok());
  EXPECT_EQ(*output, input);
}

}  // namespace
}  // namespace forensics
