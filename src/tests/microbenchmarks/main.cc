// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "main.h"

#include <cstdlib>

#if defined(__Fuchsia__)
#include <lib/scheduler/role.h>

#include "assert.h"
#endif
#include <perftest/perftest.h>

#include "round_trips.h"

// The zeroth command line argument, argv[0], used for locating this process's
// executable in order to find dependencies.
const char* argv0;

int main(int argc, char** argv) {
  argv0 = argc >= 1 ? argv[0] : "";

#if defined(__Fuchsia__) && !defined(NO_SUBPROCESS)
  // Check for the argument used by test cases for launching subprocesses.
  if (argc == 4 && strcmp(argv[1], "--subprocess") == 0) {
    RunSubprocess(argv[2], argv[3]);
    return 0;
  }
#endif

#if defined(__Fuchsia__) && BOARD_IS_VIM3
  // On VIM3s, set the thread affinity to big cores in order to reduce variation in the results.
  // This works around the scheduler's current behaviour. While the scheduler is set up to prefer
  // scheduling threads on big cores, benchmarks get scheduled on little cores often enough that it
  // reduces the usefulness of the performance results.
  // TODO(https://fxbug.dev/42050716): Find a better way of controlling what cores are used for
  // benchmarking and potentially benchmark both big and little cores.
  ASSERT_OK(fuchsia_scheduler::SetRoleForThread(zx::thread::self(),
                                                "fuchsia.microbenchmarks.pin_to_vim3_big_cores"));
#endif

  const char* test_suite = "fuchsia.microbenchmarks";
  const char* env_test_suite = std::getenv("TEST_SUITE_LABEL");
  if (env_test_suite) {
    test_suite = env_test_suite;
  }

  return perftest::PerfTestMain(argc, argv, test_suite);
}
