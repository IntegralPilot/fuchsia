// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/syslog/cpp/macros.h>

#include <gtest/gtest.h>

#include "src/lib/fxl/command_line.h"
#include "src/lib/fxl/log_settings_command_line.h"
#include "src/microfuchsia/tee/lib/dev_urandom_compat/dev_urandom_compat.h"

int main(int argc, char** argv) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  loop.StartThread("test-interest-listener-thread");
  if (!fxl::SetLogSettingsFromCommandLine(fxl::CommandLineFromArgcArgv(argc, argv),
                                          loop.dispatcher())) {
    FX_LOGS(ERROR) << "Failed to parse log settings from command-line";
    return EXIT_FAILURE;
  }

  // Setting this flag to true causes googletest to *generate* and log the random seed.
  GTEST_FLAG_SET(shuffle, true);

  zx_status_t status = register_dev_urandom_compat();
  if (status != ZX_OK) {
    FX_LOGS(ERROR) << "Failed to register /dev/urandom compat device: " << status;
    return EXIT_FAILURE;
  }

  testing::InitGoogleTest(&argc, argv);
  return RUN_ALL_TESTS();
}
