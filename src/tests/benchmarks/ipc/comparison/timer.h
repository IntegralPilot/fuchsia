// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_TESTS_BENCHMARKS_IPC_COMPARISON_TIMER_H_
#define SRC_TESTS_BENCHMARKS_IPC_COMPARISON_TIMER_H_

#include <lib/zx/time.h>

#include <atomic>

// Struct holding the durations (in ticks) for a single send/recv pair.
//
// The values are aligned to 64 bytes, which is the cache line size on our benchmark devices.
struct Timing {
  alignas(64) std::atomic_int64_t send_duration = 0;
  alignas(64) std::atomic_int64_t recv_duration = 0;

  double send_seconds() const {
    return static_cast<double>(send_duration.load()) / static_cast<double>(zx_ticks_per_second());
  }
  double recv_seconds() const {
    return static_cast<double>(recv_duration.load()) / static_cast<double>(zx_ticks_per_second());
  }
};

// RAII class for timing an operation.
//
// On destruction, this class writes the duration (in ticks) to the output variable.
class Timer {
 public:
  explicit Timer(std::atomic_int64_t* output) : output_(output), start_(zx_ticks_get_boot()) {}
  ~Timer() { *output_ = zx_ticks_get_boot() - start_; }

 private:
  std::atomic_int64_t* output_;
  int64_t start_;
};
#endif  // SRC_TESTS_BENCHMARKS_IPC_COMPARISON_TIMER_H_
