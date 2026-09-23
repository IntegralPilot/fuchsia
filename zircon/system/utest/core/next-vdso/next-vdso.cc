// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/syscalls-next.h>

#include <zxtest/zxtest.h>

#include "../needs-next.h"

NEEDS_NEXT_SYSCALL(zx_syscall_next_1);

TEST(NextVdsoTest, BasicTest) {
  NEEDS_NEXT_SKIP(zx_syscall_next_1);

  ASSERT_OK(zx_syscall_next_1(7));
}
