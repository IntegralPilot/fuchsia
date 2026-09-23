// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/task.h>
#include <lib/trace-provider/provider.h>
#include <lib/trace/event.h>

#include "src/performance/trace_bench/tracee_config.h"

namespace {
uint8_t blob[4096] = {0};

void EmitSomeEvents(async_dispatcher_t* dispatcher) {
  // We want to batch at least some trace event writes so we don't get too much async loop overhead,
  // but too many delays the async task of emptying the buffer from running.
  //
  // We batch about 16MB of data at a time, using blob events 200 times per second.
  // Doing the math, that's about 16 GiB/second of trace data, likely more that we can actually
  // write. Experimentally, that's also significnatly faster than we're able to read out the trace
  // data (< 5 GiB/second).
  //
  // Experimentally, waking to write every 1ms lets us hit saturate the read side while still idling
  // some of the time.
  async::PostTaskForTime(
      dispatcher, [dispatcher] { EmitSomeEvents(dispatcher); }, zx::deadline_after(zx::msec(1)));

  // Write about 16MB of data
  for (int i = 0; i < 4000; i++) {
    // Use a 4K sized blob event to reduce the amount of cpu overhead -- most of this should just be
    // memcpy'ing zeros.
    TRACE_BLOB_EVENT("benchmark", "blob", blob, sizeof(blob));
  }
}
}  // namespace

int main() {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  async_dispatcher_t* dispatcher = loop.dispatcher();
  trace::TraceProviderWithFdio trace_provider(dispatcher, "tracee");

  // Retrieve configuration
  auto c = tracee_config::Config::TakeFromStartupHandle();

  if (c.emit_trace_data()) {
    // Let's continuously emit some trace events
    async::PostTask(dispatcher, [dispatcher] { EmitSomeEvents(dispatcher); });
  }

  loop.Run();
  return 0;
}
