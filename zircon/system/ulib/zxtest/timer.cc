// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zxtest/base/timer.h>

#ifdef __Fuchsia__
#include <lib/zx/clock.h>
#include <lib/zx/time.h>
#else
#include <sys/time.h>
#endif

namespace zxtest {

namespace {

uint64_t now() {
#ifdef __Fuchsia__
  return (zx::clock::get_monotonic() - zx::time(0)).to_nsecs();
#else
  struct timeval tv;
  if (gettimeofday(&tv, nullptr) < 0)
    return 0u;
  return tv.tv_sec * 1000000000ull + tv.tv_usec * 1000ull;
#endif
}

}  // namespace

namespace internal {

Timer::Timer() : start_(now()) {}

void Timer::Reset() { start_ = now(); }

int64_t Timer::GetElapsedTime() const { return (now() - start_) / 1000000; }

}  // namespace internal
}  // namespace zxtest
