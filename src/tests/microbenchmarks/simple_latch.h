// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_TESTS_MICROBENCHMARKS_SIMPLE_LATCH_H_
#define SRC_TESTS_MICROBENCHMARKS_SIMPLE_LATCH_H_

#include <sys/syscall.h>
#include <unistd.h>

#include <atomic>

#include <linux/futex.h>

#include "lib/syslog/cpp/macros.h"

// A wrapper for the futex system call, that only supports FUTEX_WAIT and
// FUTEX_WAKE, without timeout.
inline long SimpleFutex(std::atomic<uint32_t>* uaddr, int futex_op, uint32_t val) {
  return syscall(SYS_futex, reinterpret_cast<uint32_t*>(uaddr), futex_op, val, NULL, NULL, 0);
}

// A class similar to std::latch that works across processes.
// std::latch is not guaranteed to work across processes.
class SimpleLatch {
 public:
  SimpleLatch(size_t expected) : pending_(expected), done_(0) { FX_CHECK(expected > 0); }

  // Decrement the counter atomically, in a non-blocking manner.
  void CountDown() {
    if (pending_.fetch_sub(1) == 1) {
      done_ = 1;
      long res = SimpleFutex(&done_, FUTEX_WAKE, INT_MAX);
      FX_CHECK(res >= 0);
    }
  }

  // Blocks until the counter reaches 0.
  void Wait() {
    while (done_ == 0) {
      long res = SimpleFutex(&done_, FUTEX_WAIT, 0);
      FX_CHECK(res == 0 || (res == -1 && errno == EAGAIN));
    }
    FX_CHECK(pending_ == 0);
  }

  SimpleLatch(const SimpleLatch&) = delete;
  SimpleLatch& operator=(const SimpleLatch&) = delete;

 private:
  std::atomic<size_t> pending_;
  std::atomic<uint32_t> done_;
};

#endif  // SRC_TESTS_MICROBENCHMARKS_SIMPLE_LATCH_H_
