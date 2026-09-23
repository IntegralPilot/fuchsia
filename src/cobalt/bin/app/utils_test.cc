// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/cobalt/bin/app/utils.h"

#include <fuchsia/buildinfo/cpp/fidl.h>

#include <gtest/gtest.h>

namespace cobalt {
namespace {

TEST(GetSystemVersionTest, RequiresProductAndPlatformVersion) {
  fuchsia::buildinfo::BuildInfo build_info;
  EXPECT_EQ(GetSystemVersion(build_info), "<version not specified>");

  build_info.set_platform_version("");
  EXPECT_EQ(GetSystemVersion(build_info), "<version not specified>");

  build_info.set_platform_version("test-platform-version");
  EXPECT_EQ(GetSystemVersion(build_info), "<version not specified>");

  build_info.clear_platform_version();
  build_info.set_product_version("");
  EXPECT_EQ(GetSystemVersion(build_info), "<version not specified>");

  build_info.set_product_version("test-product-version");
  EXPECT_EQ(GetSystemVersion(build_info), "<version not specified>");
}

TEST(GetSystemVersionTest, DoesNotConcatenateIdenticalVersions) {
  fuchsia::buildinfo::BuildInfo build_info;

  build_info.set_platform_version("test-version");
  build_info.set_product_version("test-version");
  EXPECT_EQ(GetSystemVersion(build_info), "test-version");
}

TEST(GetSystemVersionTest, ProductAndPlatformVersionsConcatenated) {
  fuchsia::buildinfo::BuildInfo build_info;

  build_info.set_platform_version("platform-version");
  build_info.set_product_version("product-version");
  EXPECT_EQ(GetSystemVersion(build_info), "product-version--platform-version");
}

}  // namespace
}  // namespace cobalt
