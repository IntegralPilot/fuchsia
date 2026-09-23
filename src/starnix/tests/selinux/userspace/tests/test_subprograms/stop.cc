// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <signal.h>

#include <gtest/gtest.h>

#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"

void stop() { ASSERT_THAT(raise(SIGSTOP), SyscallSucceeds()); }

int main(int argc, char** argv) {
  stop();
  return ::testing::Test::HasFailure() ? 1 : 0;
}
