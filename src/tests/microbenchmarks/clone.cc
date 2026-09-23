// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/syslog/cpp/macros.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

#include <vector>

#include <fbl/string_printf.h>
#include <perftest/perftest.h>

#include "scoped_mapping.h"
#include "simple_latch.h"

#ifndef MFD_CLOEXEC
#define MFD_CLOEXEC 0x0001U
#endif

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
  SimpleLatch pending_children;
  SimpleLatch parent_done;

  Sync() : pending_children(1), parent_done(1) {}
};

bool CloneWithMappings(perftest::RepeatState* state, int count) {
  std::vector<void*> mappings;
  mappings.reserve(count * 2);

  int fd = static_cast<int>(syscall(SYS_memfd_create, "test", MFD_CLOEXEC));
  FX_CHECK(fd >= 0);
  FX_CHECK(ftruncate(fd, 4096) == 0);

  for (int i = 0; i < count; i++) {
    void* p1 = mmap(NULL, 4096, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    FX_CHECK(p1 != MAP_FAILED);
    mappings.push_back(p1);

    void* p2 = mmap(NULL, 4096, PROT_READ | PROT_WRITE, MAP_PRIVATE, fd, 0);
    FX_CHECK(p2 != MAP_FAILED);
    mappings.push_back(p2);
  }

  ScopedMapping mapping(sizeof(Sync), PROT_READ | PROT_WRITE,
                        MAP_SHARED | MAP_ANONYMOUS | MAP_POPULATE);

  state->DeclareStep("sys_clone");
  state->DeclareStep("wait");

  while (state->KeepRunning()) {
    Sync* sync = new (reinterpret_cast<void*>(mapping.base())) Sync();

    // clone without CLONE_THREAD
    pid_t pid = static_cast<pid_t>(syscall(SYS_clone, SIGCHLD, 0, 0, 0, 0));
    FX_CHECK(pid >= 0);

    if (pid == 0) {
      sync->pending_children.CountDown();
      sync->parent_done.Wait();
      _exit(EXIT_SUCCESS);
    }

    sync->pending_children.Wait();

    state->NextStep();
    sync->parent_done.CountDown();

    WaitForChild(pid);
    sync->~Sync();
  }

  for (void* p : mappings) {
    FX_CHECK(munmap(p, 4096) == 0);
  }
  FX_CHECK(close(fd) == 0);

  return true;
}

void RegisterTests() {
  for (int count : {128, 1024, 4096}) {
    auto test_name = fbl::StringPrintf("CloneWithMappings/%d", count);
    perftest::RegisterTest(test_name.c_str(), CloneWithMappings, count);
  }
}
PERFTEST_CTOR(RegisterTests)

}  // namespace
