// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <thread>

#include <zxtest/zxtest.h>

#include "src/lib/debug/backtrace-request.h"

// We can't easily verify the backtrace contents, but this at least checks
// that we properly resume after requesting a backtrace. If we either hang
// or get killed the unittest runner will detect it and report a failure.
TEST(BacktraceRequest, RequestResumesAllThreads) {
  std::thread t([]() { backtrace_request_all_threads(); });
  t.join();
}

TEST(BacktraceRequest, RequestResumesCurrentThread) {
  std::thread t([]() { backtrace_request_current_thread(); });
  t.join();
}
