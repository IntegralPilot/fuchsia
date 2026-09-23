// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_MEMORY_SAMPLER_INSTRUMENTATION_SCOPED_REENTRANCY_GUARD_H_
#define SRC_PERFORMANCE_MEMORY_SAMPLER_INSTRUMENTATION_SCOPED_REENTRANCY_GUARD_H_

#include <cassert>

namespace memory_sampler {
// Scoped guard to detect reentrancy. It is meant to detect and
// prevent unbounded recursion when allocating memory from within the
// memory allocation hooks.
//
// Note: at the time of writing this comment, Scudo disallows
// allocating memory from within the memory allocation hooks.
class ScopedReentrancyGuard {
 public:
  ScopedReentrancyGuard() {
    assert(!executing_);
    executing_ = true;
  }
  ~ScopedReentrancyGuard() { executing_ = false; }

  ScopedReentrancyGuard(const ScopedReentrancyGuard&) = delete;
  ScopedReentrancyGuard& operator=(const ScopedReentrancyGuard&) = delete;

  static bool WouldReenter() { return executing_; }

 private:
  inline static thread_local bool executing_ = false;
};
}  // namespace memory_sampler

#endif  // SRC_PERFORMANCE_MEMORY_SAMPLER_INSTRUMENTATION_SCOPED_REENTRANCY_GUARD_H_
