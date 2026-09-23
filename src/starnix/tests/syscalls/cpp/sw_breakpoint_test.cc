// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/starnix/tests/syscalls/cpp/test_helper.h"

TEST(SwBreakpointTest, Int3CompactGeneratesSIGTRAP) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([] {
    struct sigaction segv_act;
    segv_act.sa_sigaction = [](int signo, siginfo_t* info, void* ucontext) {
      if (signo == SIGTRAP) {
        _exit(EXIT_SUCCESS);
      }
      _exit(EXIT_FAILURE);
    };
    segv_act.sa_flags = SA_SIGINFO;
    SAFE_SYSCALL(sigaction(SIGTRAP, &segv_act, nullptr));
    __builtin_debugtrap();
    ADD_FAILURE() << "Expected to generate SIGTRAP.";
    exit(EXIT_FAILURE);
  });
  ASSERT_TRUE(helper.WaitForChildren());
}
