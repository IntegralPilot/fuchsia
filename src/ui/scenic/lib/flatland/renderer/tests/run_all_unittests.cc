// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/trace-provider/provider.h>

#include "src/ui/lib/escher/test/common/gtest_escher.h"

int main(int argc, char** argv) {
  async::Loop trace_loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  trace::TraceProviderWithFdio provider(trace_loop.dispatcher());
  trace_loop.StartThread("flatland test tracing");

  testing::InitGoogleTest(&argc, argv);
  escher::test::EscherEnvironment::RegisterGlobalTestEnvironment();
  int result = RUN_ALL_TESTS();

  trace_loop.Shutdown();

  return result;
}
