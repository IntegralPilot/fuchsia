// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_KERNEL_LIB_USERABI_VDSO_CLOCK_COMMON_H_
#define ZIRCON_KERNEL_LIB_USERABI_VDSO_CLOCK_COMMON_H_

#include <lib/arch/intrin.h>
#include <lib/fasttime/clock.h>
#include <lib/fasttime/internal/time.h>

#include "data-time-values.h"
#include "private.h"

struct VdsoClockTransformationAdapter {
  static inline void ArchYield() { arch::Yield(); }

  static zx_instant_mono_ticks_t GetMonoTicks() {
    return fasttime::internal::compute_monotonic_ticks<
        fasttime::internal::FasttimeVerificationMode::kSkip>(DATA_TIME_VALUES);
  }

  static zx_instant_boot_ticks_t GetBootTicks() {
    return fasttime::internal::compute_boot_ticks<
        fasttime::internal::FasttimeVerificationMode::kSkip>(DATA_TIME_VALUES);
  }
};

struct VdsoForcedSyscallClockTransformationAdapter {
  static inline void ArchYield() {}
  static zx_instant_mono_ticks_t GetMonoTicks() { return SYSCALL_zx_ticks_get_via_kernel(); }
  static zx_instant_boot_ticks_t GetBootTicks() { return SYSCALL_zx_ticks_get_boot_via_kernel(); }
};

using VdsoClockTransformation = fasttime::ClockTransformation<VdsoClockTransformationAdapter>;
using VdsoForcedSyscallClockTransformation =
    fasttime::ClockTransformation<VdsoForcedSyscallClockTransformationAdapter>;

#endif  // ZIRCON_KERNEL_LIB_USERABI_VDSO_CLOCK_COMMON_H_
