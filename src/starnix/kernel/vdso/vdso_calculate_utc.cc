// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/affine/ratio.h>
#include <lib/concurrent/seqlock.inc.h>
#include <lib/fasttime/clock.h>

#include "src/starnix/kernel/vdso/vdso_calculate_time.h"
#include "src/starnix/kernel/vdso/vdso_platform.h"

// This structure implements the methods needed to satisfy the ClockTransformationAdapter concept
// used by the fasttime library to read memory-mapped clocks.
struct StarnixClockTransformationAdapter {
  static void ArchYield() {}
  static zx_instant_mono_ticks_t GetMonoTicks() { return calculate_monotonic_ticks(); }
  static zx_instant_boot_ticks_t GetBootTicks() { return calculate_boot_ticks(); }
};

using StarnixClockTransformation = fasttime::ClockTransformation<StarnixClockTransformationAdapter>;

// This is performance sensitive code. Run benchmarks before
// and after to verify the impact of changes. At the time of this writing,
// the relevant setup was:
//
// ```
// fx set ... --with-test //src/starnix/tests:gvisor:tests
// ```
//
// then for example:
//
// ```
// fx test starnix_gvisor_clock_gettime_benchmark -o
// ```
//
// See for example of impact: https://fxbug.dev/456248727
int64_t calculate_utc_time_nsec() {
  // SAFETY: initialization should ensure that `vvar` is placed at
  // start of the memory region mapped to the memory-mapped UTC clock. Note, however
  // as we declared this as a `char` in the linker script, not an array, so don't
  // forget to take a reference to convert it to a pointer. Don't ask how I know. :)
  auto* actual_utc_clock = reinterpret_cast<StarnixClockTransformation*>(&vvar);

  int64_t maybe_utc_now = 0;
  int64_t backstop_cached = 0;
  const auto status = actual_utc_clock->ReadWithBackstop(&maybe_utc_now, &backstop_cached);
  if (status != ZX_OK) {
    return kUtcInvalid;
  }

  if (maybe_utc_now != backstop_cached) {
    // Fuchsia's UTC clock is started, so pass what we read.
    return maybe_utc_now;
  }

  // Fuchsia's UTC clock is not started. Return a fake value of the UTC clock so
  // that UTC always keeps moving for Starnix programs.
  return calculate_boot_time_nsec() + backstop_cached;
}
