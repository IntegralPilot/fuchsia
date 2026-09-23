// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/inspect/component/cpp/component.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/macros.h>

#include <chrono>
#include <thread>

#include "examples/components/config_from_parent/cpp/example_config.h"

int main(int argc, const char* argv[], char* envp[]) {
  auto c = example_config::Config::TakeFromStartupHandle();
  FX_LOGS(INFO) << "Hello, " << c.greeting() << "! (from C++)";

  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);

  auto context = sys::ComponentContext::CreateAndServeOutgoingDirectory();
  inspect::ComponentInspector inspector(loop.dispatcher(), {});
  inspect::Node config_node = inspector.root().CreateChild("config");
  c.RecordInspect(&config_node);

  loop.Run();

  return 0;
}
