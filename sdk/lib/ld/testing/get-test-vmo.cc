// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "lib/ld/testing/get-test-vmo.h"

#include <lib/elfldltl/testing/get-test-data.h>

#include <filesystem>

namespace ld::testing {

zx::vmo GetExecutableVmo(std::string_view executable) {
  const std::string executable_path =
      std::filesystem::path("test") / executable / "bin" / executable;
  return elfldltl::testing::GetTestLibVmo(executable_path);
}

std::filesystem::path GetExecutableLibPath(  //
    std::string_view executable, std::optional<std::string_view> libprefix) {
  std::filesystem::path prefix{"test"};
  prefix /= executable;
  prefix /= "lib";
  if (libprefix) {
    prefix /= *libprefix;
  }
  return prefix;
}

}  // namespace ld::testing
