// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_TESTS_BENCHMARKS_IPC_COMPARISON_BATCH_CASE_H_
#define SRC_TESTS_BENCHMARKS_IPC_COMPARISON_BATCH_CASE_H_

#include <lib/zx/channel.h>

#include <barrier>

#include "config.h"
#include "timer.h"

// Benchmark transmitting large messages in batches.
//
// In this test, messages will be transmitted in batches. Each batch must be acknowledged by the
// receiving end before the next batch is sent.
class BatchCase {
 public:
  explicit BatchCase(BatchConfig config) : config_(config), start_barrier_(2), stop_barrier_(2) {}

  void send(zx::channel chan, Timing* cur_timing);
  void recv(zx::channel chan, Timing* cur_timing);

 private:
  BatchConfig config_;

  std::barrier<> start_barrier_;
  std::barrier<> stop_barrier_;
};

#endif  // SRC_TESTS_BENCHMARKS_IPC_COMPARISON_BATCH_CASE_H_
