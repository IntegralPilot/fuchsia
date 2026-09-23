// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "lib/ld/testing/interp.h"

#include <lib/ld/abi.h>

#include <gtest/gtest.h>

namespace ld::testing {

std::optional<std::string> ConfigFromInterp(  //
    const std::filesystem::path& interp, std::optional<std::string_view> expected_config) {
  EXPECT_EQ(interp.filename(), abi::kInterp) << interp;

  if (!interp.has_parent_path()) {
    EXPECT_EQ(std::nullopt, expected_config) << interp;
    return std::nullopt;
  }

  std::filesystem::path prefix = interp.parent_path();
  std::optional<std::string> config = prefix;
  if (expected_config) {
    EXPECT_EQ(config, expected_config) << interp;
  }

  return config;
}

}  // namespace ld::testing
