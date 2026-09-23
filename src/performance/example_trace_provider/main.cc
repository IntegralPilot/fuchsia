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

#include "lib/async/cpp/task.h"
#include "zircon/system/public/zircon/process.h"

size_t async_id = 0;
size_t flow_id = 0;
uint64_t data = 0;
uint64_t data2 = UINT32_MAX;
void EmitSomeEvents(async_dispatcher_t* dispatcher) {
  // The simplest trace event is an instant event, which marks a point in time. It has both a name
  // and a category to differentiate it. It will show up in the Perfetto UI as a little arrow.
  TRACE_INSTANT("example", "example_instant", TRACE_SCOPE_THREAD);

  TRACE_COUNTER("example", "example_counter", 0, "somedataseries", data++);
  TRACE_COUNTER("example", "example_counter2", 1, "someotherdataseries", data2--);

  // Only slightly more complicated is a duration event, which marks _two_ points in time.
  // It will appear in the viewer as a block
  TRACE_DURATION_BEGIN("example", "example_duration");
  usleep(50000);
  // It needs to be matched by a duration end with the same name and category.
  TRACE_DURATION_END("example", "example_duration");
  // Alternatively, you can use the RAII macro which will automatically handle this for you when it
  // goes out of scope.
  TRACE_DURATION("example", "example_duration_raii");
  usleep(25000);

  // You can use async events to record an operation that may occur across threads.
  // In the viewer, the events will appear linearly on their own track based on the name of the
  // async begin event as opposed to the thread they were emitted on.
  TRACE_ASYNC_BEGIN("example", "example_async", async_id);
  TRACE_ASYNC_BEGIN("example", "example_async2", async_id + 1);
  std::thread thread1([id = async_id] {
    usleep(5000);
    TRACE_ASYNC_INSTANT("example", "example_async_instant1", id);
  });
  std::thread thread2([id = async_id] {
    usleep(10000);
    TRACE_ASYNC_INSTANT("example", "example_async_instant2", id);
  });
  std::thread thread3([id = async_id] {
    usleep(5000);
    TRACE_ASYNC_INSTANT("example", "example_async_instant3", id + 1);
  });
  std::thread thread4([id = async_id] {
    usleep(10000);
    TRACE_ASYNC_INSTANT("example", "example_async_instant4", id + 1);
  });
  thread1.join();
  thread2.join();
  thread3.join();
  thread4.join();
  TRACE_ASYNC_END("example", "example_async", async_id);
  TRACE_ASYNC_END("example", "example_async2", async_id + 1);
  async_id += 2;

  // Flow events allow you to link together duration events that may occur across threads or
  // abstraction layers. Each flow event must be enclosed by a duration event. The viewer then
  // displays the flow events as arrows connecting each of the duration events.
  TRACE_FLOW_BEGIN("example", "example_flow", flow_id);
  TRACE_FLOW_BEGIN("example", "example_flow2", flow_id + 1);
  std::thread thread5([id = flow_id] {
    TRACE_DURATION("example", "thread5");
    usleep(1000);
    TRACE_FLOW_STEP("example", "example_flow_step1", id);
  });
  std::thread thread6([id = flow_id] {
    TRACE_DURATION("example", "thread6");
    usleep(2000);
    TRACE_FLOW_STEP("example", "example_flow_step2", id);
  });
  std::thread thread7([id = flow_id] {
    TRACE_DURATION("example", "thread7");
    usleep(3000);
    TRACE_FLOW_STEP("example", "example_flow_step3", id + 1);
  });
  std::thread thread8([id = flow_id] {
    TRACE_DURATION("example", "thread8");
    usleep(4000);
    TRACE_FLOW_STEP("example", "example_flow_step4", id + 1);
  });
  std::thread thread9([id = flow_id] {
    TRACE_DURATION("example", "thread9");
    usleep(5000);
    TRACE_FLOW_END("example", "example_flow", id);
  });
  std::thread thread10([id = flow_id] {
    TRACE_DURATION("example", "thread10");
    usleep(6000);
    TRACE_FLOW_END("example", "example_flow2", id + 1);
  });
  thread5.join();
  thread6.join();
  thread7.join();
  thread8.join();
  thread9.join();
  thread10.join();
  flow_id += 2;

  // Trace Events can also have arguments associated with them that will show up when you click on
  // the event in the viewer
  //
  // Null arguments are just a name and no data
  TRACE_INSTANT("example", "example_instant_args", TRACE_SCOPE_THREAD, "SomeNullArg", TA_NULL());
  // There are standard int types
  TRACE_INSTANT("example", "example_instant_args", TRACE_SCOPE_THREAD, "Someuint32",
                uint32_t{2145});
  TRACE_INSTANT("example", "example_instant_args", TRACE_SCOPE_THREAD, "Someuint64",
                uint64_t{423621626134123415});
  TRACE_INSTANT("example", "example_instant_args", TRACE_SCOPE_THREAD, "Someint32", int32_t{-7});
  TRACE_INSTANT("example", "example_instant_args", TRACE_SCOPE_THREAD, "Someint64",
                int64_t{-234516543631231});
  // Doubles
  TRACE_INSTANT("example", "example_instant_args", TRACE_SCOPE_THREAD, "Somedouble",
                double{3.1415});

  // Strings
  TRACE_INSTANT("example", "example_instant_args", TRACE_SCOPE_THREAD, "ping", "pong");

  // Pointers will format in hex in the viewer
  int i = 0;
  // You can also specifically specify the trace argument type when type deduction would otherwise
  // resolve to the wrong type.
  TRACE_INSTANT("example", "example_instant_args", TRACE_SCOPE_THREAD, "somepointer", &i,
                "someotherpointer", TA_POINTER(0xABCD));

  // Koids has their own type, but it's really just a uint64_t that has special meaning on the most
  // significant bit being 1 or not.
  TRACE_INSTANT("example", "example_instant_args", TRACE_SCOPE_THREAD, "somekoid", TA_KOID(0x0012));

  // Booleans with display as "true" or "false"
  TRACE_INSTANT("example", "example_instant_args", TRACE_SCOPE_THREAD, "somebool", true);
  TRACE_INSTANT("example", "example_instant_args", TRACE_SCOPE_THREAD, "someotherbool", false);
  async::PostDelayedTask(
      dispatcher, [dispatcher] { EmitSomeEvents(dispatcher); }, zx::msec(10));
}

int main() {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  async_dispatcher_t* dispatcher = loop.dispatcher();
  // Once we link against tracing and include the tracing client shard, all we need to do in create
  // TraceProvider object.
  trace::TraceProviderWithFdio trace_provider(dispatcher, "example_trace_provider");

  // Let's continuously emit some trace events
  async::PostTask(dispatcher, [dispatcher] { EmitSomeEvents(dispatcher); });

  // Don't forget to start the loop!
  loop.Run();
  return 0;
}
