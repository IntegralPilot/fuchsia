// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_POWER_CPP_TESTING_SCOPED_BACKGROUND_LOOP_H_
#define LIB_DRIVER_POWER_CPP_TESTING_SCOPED_BACKGROUND_LOOP_H_

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/executor.h>
#include <lib/async/dispatcher.h>

namespace fdf_power::testing {

// Runs an async loop on a background thread and cleans up when this object exits scope.
class ScopedBackgroundLoop {
 public:
  explicit ScopedBackgroundLoop()
      : loop_(&kAsyncLoopConfigNoAttachToCurrentThread), executor_(loop_.dispatcher()) {
    loop_.StartThread();
  }

  ~ScopedBackgroundLoop() {
    loop_.RunUntilIdle();
    loop_.Quit();
    loop_.Shutdown();
    loop_.JoinThreads();
  }

  async_dispatcher_t* dispatcher() const { return loop_.dispatcher(); }

  async::Executor& executor() { return executor_; }

 private:
  async::Loop loop_;
  async::Executor executor_;
};

}  // namespace fdf_power::testing

#endif  // LIB_DRIVER_POWER_CPP_TESTING_SCOPED_BACKGROUND_LOOP_H_
