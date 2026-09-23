// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/task.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace-provider/provider.h>
#include <lib/trace/event.h>
#include <unistd.h>

size_t counter = 1;
void EmitSomeEvents(async_dispatcher_t* dispatcher) {
  TRACE_INSTANT("test", "test_counter", TRACE_SCOPE_THREAD, "counter", uint64_t{counter++});
  async::PostTask(dispatcher, [dispatcher] { EmitSomeEvents(dispatcher); });
}

int main() {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  async_dispatcher_t* dispatcher = loop.dispatcher();
  trace::TraceProviderWithFdio trace_provider(dispatcher, "test_provider");

  // Continuously emit some trace events
  async::PostTask(dispatcher, [dispatcher] { EmitSomeEvents(dispatcher); });

  loop.Run();
  return 0;
}
