// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/performance/cpu_profiler/process_watcher.h"

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/zx/job.h>
#include <lib/zx/process.h>
#include <lib/zx/result.h>
#include <lib/zx/thread.h>
#include <zircon/types.h>

#include <atomic>
#include <thread>
#include <utility>

#include <gtest/gtest.h>

// Monitor a process and ensure it gets notified when it starts a thread.
//
// Note that the starting thread is paused, but not the starter thread. That ensures we don't
// accidentally deadlock ourself when handling the debug exception.
std::atomic<bool> saw_child;

TEST(ProcessWatcherTests, SelfThreads) {
  saw_child = false;
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  zx::unowned_process self = zx::process::self();

  profiler::ProcessWatcher watcher{
      std::move(self),
      [](zx_koid_t, zx_koid_t, zx::thread t) { saw_child = true; },
      [](zx_koid_t, zx_koid_t) {},
  };
  std::thread thread_spawner{[]() {
    for (;;) {
      if (saw_child) {
        return;
      }
      std::thread t([]() { return 0; });
      t.join();
    }
  }};

  thread_spawner.detach();
  zx::result<> watch_res = watcher.Watch(loop.dispatcher());
  ASSERT_TRUE(watch_res.is_ok());

  while (!saw_child) {
    loop.RunUntilIdle();
  }
  EXPECT_TRUE(saw_child);
}
