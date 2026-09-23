// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/syslog/cpp/macros.h>
#include <stdlib.h>
#include <sys/mman.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

#include <vector>

#include <fbl/string_printf.h>
#include <perftest/perftest.h>

#include "scoped_mapping.h"
#include "simple_latch.h"

namespace {

void WaitForChild(pid_t pid) {
  int status;
  pid_t child_pid = waitpid(pid, &status, 0);
  FX_CHECK(child_pid == pid);
  FX_CHECK(WIFEXITED(status));
  FX_CHECK(WEXITSTATUS(status) == 0);
}

class Sync {
 public:
  // main process will wait on pending_children
  SimpleLatch pending_children;

  // children will wait on parent_done before exiting.
  SimpleLatch parent_done;

  Sync(size_t expected_children) : pending_children(expected_children), parent_done(1) {}
};

// Measures the time taken by calling fork once.
//
// The fork part of the measurement will include the time required for the
// child to start running and writing to a page shared with the parent
// process. The wait part of the measurement includes all the process teardown
// and signaling until the child process finishes.
bool Fork(perftest::RepeatState* state) {
  // Use a shared mapping to synchronize with the parent process.
  ScopedMapping mapping(sizeof(Sync), PROT_READ | PROT_WRITE,
                        MAP_SHARED | MAP_ANONYMOUS | MAP_POPULATE);
  state->DeclareStep("fork");
  state->DeclareStep("wait");

  while (state->KeepRunning()) {
    Sync* sync = new (reinterpret_cast<void*>(mapping.base())) Sync(1);
    pid_t pid = fork();
    FX_CHECK(pid >= 0);

    if (pid == 0) {
      sync->pending_children.CountDown();
      // Wait until the next step before exiting.
      sync->parent_done.Wait();
      _exit(EXIT_SUCCESS);
    }
    sync->pending_children.Wait();

    // At this point, all children have started executing.
    // Stop measuring fork time and start measuring wait time.
    state->NextStep();
    sync->parent_done.CountDown();

    WaitForChild(pid);
    sync->~Sync();
  }
  return true;
}

// Measures the time taken by calling fork n times.
//
// The fork part of the measurement will include the time required for all the
// children to start running and writing to a page shared with the parent
// process. The wait part of the measurement includes all the process teardown
// and signaling until all the child processes have finished.
bool ForkMultiple(perftest::RepeatState* state, size_t n) {
  std::vector<pid_t> pids(n, -1);

  // Use a shared mapping to synchronize with the parent process.
  ScopedMapping mapping(sizeof(Sync), PROT_READ | PROT_WRITE,
                        MAP_SHARED | MAP_ANONYMOUS | MAP_POPULATE);

  state->DeclareStep("fork");
  state->DeclareStep("wait");

  while (state->KeepRunning()) {
    Sync* sync = new (reinterpret_cast<void*>(mapping.base())) Sync(n);

    // Parent process will fork n times.
    for (size_t i = 0; i < n; i++) {
      pid_t pid = fork();
      FX_CHECK(pid >= 0);

      if (pid == 0) {
        sync->pending_children.CountDown();

        // Wait until the next step before exiting.
        sync->parent_done.Wait();
        _exit(EXIT_SUCCESS);
      }
      pids[i] = pid;
    }
    sync->pending_children.Wait();

    // At this point, all children have started executing.
    // Stop measuring fork time and start measuring wait time.
    state->NextStep();
    sync->parent_done.CountDown();

    for (pid_t pid : pids) {
      WaitForChild(pid);
    }
    sync->~Sync();
  }

  return true;
}

pid_t ForkRecursive(Sync* sync, size_t n) {
  FX_CHECK(n > 0);

  pid_t pid = fork();
  FX_CHECK(pid >= 0);
  if (pid == 0) {
    // Child process.
    // Fork recursively and wait for that child to finish.
    sync->pending_children.CountDown();
    if (n == 1) {
      // Last child, nothing to be done.
      sync->parent_done.Wait();
    } else {
      pid_t pid = ForkRecursive(sync, n - 1);
      sync->parent_done.Wait();
      WaitForChild(pid);
    }
    _exit(EXIT_SUCCESS);
  }

  return pid;
}

// Measures the time taken by creating a chain of n nested forks.
//
// The fork part of the measurement will include the time required for all the
// children to start running and writing to a page shared with the parent
// process. The wait part of the measurement includes all the process teardown
// and signaling until all the child processes have finished.
bool ForkMultipleNested(perftest::RepeatState* state, size_t n) {
  // Use a shared mapping to synchronize with the parent process.
  ScopedMapping mapping(sizeof(Sync), PROT_READ | PROT_WRITE,
                        MAP_SHARED | MAP_ANONYMOUS | MAP_POPULATE);

  state->DeclareStep("fork");
  state->DeclareStep("wait");

  while (state->KeepRunning()) {
    Sync* sync = new (reinterpret_cast<void*>(mapping.base())) Sync(n);

    pid_t pid = ForkRecursive(sync, n);

    sync->pending_children.Wait();
    // At this point, all children have started executing.
    // Stop measuring fork time and start measuring wait time.
    state->NextStep();
    sync->parent_done.CountDown();

    WaitForChild(pid);

    sync->~Sync();
  }
  return true;
}

void RegisterTests() {
  perftest::RegisterTest("Fork", Fork);

  for (size_t length : {8, 64}) {
    auto test_name = fbl::StringPrintf("ForkMultiple/%zu", length);
    perftest::RegisterTest(test_name.c_str(), ForkMultiple, length);
  }

  for (size_t length : {8, 64}) {
    auto test_name = fbl::StringPrintf("ForkMultipleNested/%zu", length);
    perftest::RegisterTest(test_name.c_str(), ForkMultipleNested, length);
  }
}
PERFTEST_CTOR(RegisterTests)

}  // namespace
