// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace-provider/provider.h>
#include <lib/trace/event.h>
#include <unistd.h>

#include <thread>

int main() {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  async_dispatcher_t* dispatcher = loop.dispatcher();
  std::unique_ptr<trace::TraceProviderWithFdio> trace_provider;
  FX_CHECK(trace::TraceProviderWithFdio::CreateSynchronously(dispatcher, nullptr, &trace_provider,
                                                             nullptr));
  FX_CHECK(trace_provider->is_valid());

  // Run before emitting trace events so that the already-started trace session is observed.
  loop.RunUntilIdle();

  TRACE_INSTANT("test_puppet", "puppet_instant", TRACE_SCOPE_THREAD);

  TRACE_COUNTER("test_puppet", "puppet_counter", 0, "somedataseries", int32_t{1});
  TRACE_COUNTER("test_puppet", "puppet_counter2", 1, "someotherdataseries", UINT64_MAX - 1);

  TRACE_DURATION_BEGIN("test_puppet", "puppet_duration");
  TRACE_DURATION_END("test_puppet", "puppet_duration");

  TRACE_DURATION("test_puppet", "puppet_duration_raii");

  TRACE_ASYNC_BEGIN("test_puppet", "puppet_async", 1);
  std::thread instant_thread(
      [] { TRACE_ASYNC_INSTANT("test_puppet", "puppet_async_instant1", 1); });
  instant_thread.join();
  TRACE_ASYNC_END("test_puppet", "puppet_async", 1);

  TRACE_FLOW_BEGIN("test_puppet", "puppet_flow", 2);
  std::thread flow_thread([] {
    TRACE_DURATION("test_puppet", "flow_thread");
    TRACE_FLOW_STEP("test_puppet", "puppet_flow_step1", 2);
  });
  flow_thread.join();
  TRACE_FLOW_END("test_puppet", "puppet_flow", 2);

  TRACE_INSTANT("test_puppet", "puppet_instant_args", TRACE_SCOPE_THREAD, "SomeNullArg", TA_NULL());
  TRACE_INSTANT("test_puppet", "puppet_instant_args", TRACE_SCOPE_THREAD, "SomeUint32",
                uint32_t{2145});
  TRACE_INSTANT("test_puppet", "puppet_instant_args", TRACE_SCOPE_THREAD, "SomeUint64",
                uint64_t{423621626134123415});
  TRACE_INSTANT("test_puppet", "puppet_instant_args", TRACE_SCOPE_THREAD, "SomeInt32", int32_t{-7});
  TRACE_INSTANT("test_puppet", "puppet_instant_args", TRACE_SCOPE_THREAD, "SomeInt64",
                int64_t{-234516543631231});
  TRACE_INSTANT("test_puppet", "puppet_instant_args", TRACE_SCOPE_THREAD, "SomeDouble",
                double{3.141592653589793});
  TRACE_INSTANT("test_puppet", "puppet_instant_args", TRACE_SCOPE_THREAD, "SomeString", "pong");
  TRACE_INSTANT("test_puppet", "puppet_instant_args", TRACE_SCOPE_THREAD, "SomeBool", true);
  TRACE_INSTANT("test_puppet", "puppet_instant_args", TRACE_SCOPE_THREAD, "SomePointer",
                TA_POINTER(4096));
  TRACE_INSTANT("test_puppet", "puppet_instant_args", TRACE_SCOPE_THREAD, "SomeKoid", TA_KOID(10));

  // Run some more before exiting just in case we need to emit a buffer full notification.
  loop.RunUntilIdle();

  return 0;
}
