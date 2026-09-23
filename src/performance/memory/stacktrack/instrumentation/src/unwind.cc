// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/backtrace.h>

namespace {

struct Frame {
  // LINT.IfChange
  uint64_t pc;
  uint64_t fp;
  // LINT.ThenChange(//src/performance/memory/stacktrack/lib/stacktrack_vmo/src/threads_table_v1.rs)
};

struct IsOnStackAlwaysYes {
  bool operator()(const arch::CallFrame* fp) const { return true; }
};

}  // namespace

extern "C" size_t stacktrack_unwind_if_deeper(uint64_t threshold_fp, Frame* out_frames,
                                              size_t max_frames) {
  using FpBacktrace = arch::FramePointerBacktrace<IsOnStackAlwaysYes, true>;
  auto backtrace = FpBacktrace::BackTrace();

  // Get the base FP (the last frame in the backtrace).
  auto it = backtrace.begin();
  uint64_t base_fp = (*it).frame_address();

  // Stack grows down, so "deeper" means a smaller FP address.
  // If threshold_fp is 0, it's the first run, so we always unwind.
  if (threshold_fp != 0 && base_fp >= threshold_fp) {
    return 0;
  }

  // Fill frames, up to max_frames.
  size_t captured = 0;
  while (it != FpBacktrace::end() && (*it).frame_address() != 0 && captured < max_frames) {
    out_frames[captured++] = Frame{
        .pc = (*it).pc,
        .fp = (*it).frame_address(),
    };
    ++it;
  }

  return captured;
}
