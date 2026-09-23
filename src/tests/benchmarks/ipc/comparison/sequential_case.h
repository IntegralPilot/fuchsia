// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_TESTS_BENCHMARKS_IPC_COMPARISON_SEQUENTIAL_CASE_H_
#define SRC_TESTS_BENCHMARKS_IPC_COMPARISON_SEQUENTIAL_CASE_H_

#include <lib/zx/channel.h>

#include <barrier>
#include <semaphore>

#include "config.h"
#include "timer.h"

// Benchmark sending messages sequentially.
//
// This case demonstrates the behavior of a proposed change to channel behavior: allowing channels
// to return ZX_ERR_SHOULD_WAIT when another write will reach a configured channel limit. The idea
// is that messages can be sent sequentially without awaiting explicit acknowledgment.
class SequentialCase {
 public:
  explicit SequentialCase(SequentialConfig config)
      : config_(config),
        start_barrier_(2),
        stop_barrier_(2),
        chan_capacity_(config.channel_capacity) {}

  void send(zx::channel chan, Timing* cur_timing);
  void recv(zx::channel chan, Timing* cur_timing);

 private:
  SequentialConfig config_;

  std::barrier<> start_barrier_;
  std::barrier<> stop_barrier_;
  std::counting_semaphore<> chan_capacity_;
};

#endif  // SRC_TESTS_BENCHMARKS_IPC_COMPARISON_SEQUENTIAL_CASE_H_
