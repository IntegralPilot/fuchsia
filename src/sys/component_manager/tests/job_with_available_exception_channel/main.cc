// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/zx/job.h>
#include <zircon/status.h>

#include "src/lib/test-suite/test_suite.h"

int main(int argc, const char** argv) {
  fuchsia::test::Status result = fuchsia::test::Status::PASSED;

  zx::channel excp;
  zx_status_t status = zx::job::default_job()->create_exception_channel(0, &excp);
  if (status != ZX_OK) {
    printf("zx::job::default_job()->create_exception_channel failed with %d (%s)\n", status,
           zx_status_get_string(status));
    result = fuchsia::test::Status::FAILED;
  }

  std::vector<example::TestInput> inputs = {
      {.name = "JobWithAvailableExceptionChannel.Smoke", .status = result},
  };

  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  auto context = sys::ComponentContext::CreateAndServeOutgoingDirectory();

  example::TestSuite suite(&loop, std::move(inputs));
  context->outgoing()->AddPublicService(suite.GetHandler());

  loop.Run();
  return 0;
}
