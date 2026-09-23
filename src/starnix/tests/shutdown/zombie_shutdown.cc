// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <stdio.h>
#include <sys/wait.h>
#include <unistd.h>

int main() {
  pid_t pid = fork();
  if (pid == 0) {
    // Child process exits immediately, leaving a zombie.
    return 0;
  }

  // Block until the child has exited but leave it as a zombie.
  siginfo_t info;
  waitid(P_PID, pid, &info, WEXITED | WNOWAIT);

  // The child is now a zombie. Signal ready to the Rust test runner.
  printf("[ZOMBIE_READY]\n");
  fflush(stdout);

  // Sleep forever to keep the parent alive.
  while (true) {
    sleep(1);
  }
}
