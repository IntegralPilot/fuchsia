// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZXTEST_BASE_TIMER_H_
#define ZXTEST_BASE_TIMER_H_

#include <cstdint>

namespace zxtest::internal {

// Helper class to measure a timer interval.
class Timer {
 public:
  Timer();

  void Reset();

  // Gets the amount of milliseconds since |start_|.
  int64_t GetElapsedTime() const;

 private:
  uint64_t start_;
};

}  // namespace zxtest::internal

#endif  // ZXTEST_BASE_TIMER_H_
