// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELFLDLTL_TESTING_INCLUDE_LIB_ELFLDLTL_TESTING_GET_TEST_DATA_H_
#define SRC_LIB_ELFLDLTL_TESTING_INCLUDE_LIB_ELFLDLTL_TESTING_GET_TEST_DATA_H_

#include <filesystem>
#include <string_view>

#include <fbl/unique_fd.h>

#ifdef __Fuchsia__
#include <lib/zx/vmo.h>
#endif

namespace elfldltl::testing {

// Get the full path to an arbitrary test data file.
std::filesystem::path GetTestDataPath(std::string_view filename);

// Get an open fd specifically on a test DSO file, if it exists.
fbl::unique_fd TryGetTestLib(std::string_view libname);

// Get an open fd specifically on a test DSO file.
fbl::unique_fd GetTestLib(std::string_view libname);

#ifdef __Fuchsia__

// Get the vmo backing an arbitrary test data file, if it exists.
zx::vmo TryGetTestLibVmo(std::string_view libname);

// Get the vmo backing an arbitrary test data file.
zx::vmo GetTestLibVmo(std::string_view libname);
#endif

}  // namespace elfldltl::testing

#endif  // SRC_LIB_ELFLDLTL_TESTING_INCLUDE_LIB_ELFLDLTL_TESTING_GET_TEST_DATA_H_
