// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/syslog/cpp/macros.h>

#include <gtest/gtest.h>

#ifdef __Fuchsia__
#include <lib/async-loop/cpp/loop.h>
#endif

#include <sanitizer/lsan_interface.h>

#include "test_settings.h"

extern "C" decltype(__lsan_do_leak_check) __lsan_do_leak_check [[gnu::weak]];
extern "C" decltype(__lsan_disable) __lsan_disable [[gnu::weak]];

int main(int argc, char** argv) {
#ifdef __Fuchsia__
  // A normal invocation of the gtest binary on Fuchsia uses a dispatcher
  // thread to handle status reporting.  But this shouldn't be done for the
  // internal re-invocation to run a specific death test expression, where a
  // background thread existing might interfere with the test logic.
  if (fxl::CommandLineFromArgcArgv(argc, argv).HasOption("gtest_internal_run_death_test")) {
    // When linked with LeakSanitizer, the gtest machinery will allocate
    // various things in the child that appear to be leaked when the child
    // exits.  So force a leak check at startup to catch anything real, and
    // then disable lsan for future allocations.
    if (&__lsan_disable) {
      __lsan_do_leak_check();
      __lsan_disable();
    }
  } else {
    async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
    loop.StartThread("test-interest-listener-thread");
    if (!fxl::SetTestSettings(argc, argv, loop.dispatcher())) {
      FX_LOGS(ERROR) << "Failed to parse log settings from command-line";
      return EXIT_FAILURE;
    }
  }
#endif

  if (!fxl::SetTestSettings(argc, argv)) {
    FX_LOGS(ERROR) << "Failed to parse log settings from command-line";
    return EXIT_FAILURE;
  }

  // Setting this flag to true causes googletest to *generate* and log the random seed.
  GTEST_FLAG_SET(shuffle, true);

  testing::InitGoogleTest(&argc, argv);
  return RUN_ALL_TESTS();
}
