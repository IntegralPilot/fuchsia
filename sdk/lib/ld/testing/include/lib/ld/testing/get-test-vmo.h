// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_TESTING_GET_TEST_VMO_H_
#define LIB_LD_TESTING_GET_TEST_VMO_H_

#include <lib/zx/vmo.h>

#include <filesystem>
#include <optional>
#include <string_view>

namespace ld::testing {

zx::vmo GetExecutableVmo(std::string_view executable);

// This yields the path prefix for dependencies of the executable packages as
// GetExecutableVmo() will find it.  See <lib/ld/testing/interp.h>; the
// libprefix should match its ld::testing::ConfigFromInterp() result, if any.
// The returned path can be passed to elfldltl::testing::GetLibVmo.
std::filesystem::path GetExecutableLibPath(
    std::string_view executable, std::optional<std::string_view> libprefix = std::nullopt);

}  // namespace ld::testing

#endif  // LIB_LD_TESTING_GET_TEST_VMO_H_
