// Copyright 2025 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/starnix/tests/syscalls/cpp/binder/common.h"

#include <fcntl.h>

#include <utility>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/starnix/tests/syscalls/cpp/binder_helper.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"

namespace starnix_binder {

FdAndMap OpenBinderAndMap(std::string_view dir) {
  fbl::unique_fd binder_fd =
      fbl::unique_fd(open((std::string(dir) + "/binder").c_str(), O_RDWR | O_CLOEXEC));
  EXPECT_TRUE(binder_fd) << strerror(errno);

  auto mapping = test_helper::ScopedMMap::MMap(nullptr, kBinderMMapSize, PROT_READ, MAP_PRIVATE,
                                               binder_fd.get(), 0);
  EXPECT_THAT(mapping, SyscallResultIsOk());

  return {.fd_ = std::move(binder_fd), .mapping_ = std::move(mapping)};
}

}  // namespace starnix_binder
