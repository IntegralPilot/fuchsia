// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_SYSTEM_UTEST_CORE_CORE_TEST_UTILS_INCLUDE_LIB_CORE_TEST_UTILS_H_
#define ZIRCON_SYSTEM_UTEST_CORE_CORE_TEST_UTILS_INCLUDE_LIB_CORE_TEST_UTILS_H_

#include <stdint.h>

#include <optional>
#include <string_view>

namespace core_test_utils {

// Returns std::nullopt if skipping should not occur, or else will return the
// message that should be passed to ZXTEST_SKIP().
std::optional<std::string_view> SkipBug363254896();

// Retrieves the lowest non-canonical address on x86-64.
// This function asserts that the VMAR info query succeeds.
void X86LowestNonCanonicalAddr(uintptr_t& out);

}  // namespace core_test_utils

#endif  // ZIRCON_SYSTEM_UTEST_CORE_CORE_TEST_UTILS_INCLUDE_LIB_CORE_TEST_UTILS_H_
