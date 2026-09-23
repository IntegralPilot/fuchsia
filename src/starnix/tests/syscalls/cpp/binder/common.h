// Copyright 2025 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STARNIX_TESTS_SYSCALLS_CPP_BINDER_COMMON_H_
#define SRC_STARNIX_TESTS_SYSCALLS_CPP_BINDER_COMMON_H_

#include <lib/fit/result.h>

#include <cstdint>

#include "fbl/unique_fd.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

namespace starnix_binder {

struct FdAndMap {
  const fbl::unique_fd fd_;
  const fit::result<int, test_helper::ScopedMMap> mapping_;
};

FdAndMap OpenBinderAndMap(std::string_view dir);

// The same (otherwise-arbitrary) values found in SELinux Test Suite, because
// one of our tests performs the same operations (byte-for-byte) as a test in
// SELinux Test Suite.
constexpr uint32_t kAddService = 240616;
constexpr uint32_t kGetService = 290317;
constexpr uint32_t kServiceSendFd = 310120;

}  // namespace starnix_binder

#endif  // SRC_STARNIX_TESTS_SYSCALLS_CPP_BINDER_COMMON_H_
