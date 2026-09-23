// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <sys/uio.h>

#include <gtest/gtest.h>

namespace {

TEST(ProcessVmReadv, zeros) {
  ASSERT_EQ(process_vm_readv(0, nullptr, 0, nullptr, 0, 0), 0);

  char src[1024] = "This is a valid input buffer";
  char dst[1024] = "";
  iovec remote = {src, sizeof src};
  iovec local = {dst, sizeof dst};

  // Invalid local, with a valid remote.
  ASSERT_EQ(process_vm_readv(0, nullptr, 0, &remote, 1, 0), 0);

  // Invalid remote, with a valid local.
  ASSERT_EQ(process_vm_readv(0, &local, 1, nullptr, 0, 0), 0);
}

}  // namespace
