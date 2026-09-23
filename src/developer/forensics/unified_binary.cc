// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/syslog/cpp/macros.h>

#include <cstdlib>

#include "src/developer/forensics/exceptions/handler/main.h"
#include "src/developer/forensics/exceptions/main.h"
#include "src/developer/forensics/feedback/main.h"
#include "src/developer/forensics/feedback_data/system_log_recorder/main.h"

int main(int argc, const char** argv) {
  // For components' main executables, argv[0] is the binary path.
  // For sub-processes, argv[0] is the process name.
  FX_CHECK(argc >= 1);

  const std::string argv0 = std::string(argv[0]);
  if (argv0 == "/pkg/bin/exceptions") {
    return ::forensics::exceptions::main();
  }
  if (argv0.rfind("exception_handler_") == 0) {
    FX_CHECK(argc >= 2);
    return ::forensics::exceptions::handler::main(argv0, argv[1]);
  }
  if (argv0 == "/pkg/bin/feedback") {
    return ::forensics::feedback::main();
  }
  if (argv0 == "system_log_recorder") {
    return ::forensics::feedback_data::system_log_recorder::main();
  }

  return EXIT_FAILURE;
}
