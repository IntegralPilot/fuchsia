// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_TESTS_BENCHMARKS_IPC_COMPARISON_STREAM_CASE_H_
#define SRC_TESTS_BENCHMARKS_IPC_COMPARISON_STREAM_CASE_H_

#include <lib/zx/channel.h>

#include <barrier>

#include "config.h"
#include "timer.h"

// Benchmark transmitting large messages in a socket.
//
// In this case, length-prefixed messages are sent over a Zircon socket, which has built-in
// backpressure.
class StreamCase {
 public:
  explicit StreamCase(StreamConfig config) : config_(config), start_barrier_(2), stop_barrier_(2) {}

  void send(zx::channel chan, Timing* cur_timing);
  void recv(zx::channel chan, Timing* cur_timing);

 private:
  StreamConfig config_;

  std::barrier<> start_barrier_;
  std::barrier<> stop_barrier_;
};

#endif  // SRC_TESTS_BENCHMARKS_IPC_COMPARISON_STREAM_CASE_H_
