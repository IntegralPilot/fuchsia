// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fasttime/internal/time.h>

#include "data-time-values.h"
#include "private.h"

// By default, when we get clock monotonic, simply transform the tick counter
// using the user-mode resident VDSO version of zx_ticks_get.
__EXPORT zx_instant_mono_t _zx_clock_get_monotonic(void) {
  return fasttime::internal::compute_monotonic_time<
      fasttime::internal::FasttimeVerificationMode::kSkip>(DATA_TIME_VALUES);
}

VDSO_INTERFACE_FUNCTION(zx_clock_get_monotonic);

// If the registers needed to query ticks are not available in user-mode, or
// kernel command line args have been passed to force zx_ticks_get to always be
// a syscall, then the kernel can choose to use this alternate implementation of
// zx_clock_get_monotonic instead.  It will perform the transformation from
// ticks to clock mono in user mode (just like the default version), but it will
// query its ticks from the via_kernel version of zx_ticks_get.
VDSO_KERNEL_EXPORT zx_instant_mono_t CODE_clock_get_monotonic_via_kernel_ticks(void) {
  affine::Ratio ticks_to_time_ratio(DATA_TIME_VALUES.ticks_to_time_numerator,
                                    DATA_TIME_VALUES.ticks_to_time_denominator);
  return ticks_to_time_ratio.Scale(SYSCALL_zx_ticks_get_via_kernel());
}

// Note: See alternates.ld for a definition of
// CODE_clock_get_monotonic_via_kernel, which is an alias for
// SYSCALL_zx_clock_get_monotonic_via_kernel.  This is a version of
// zx_clock_get_monotonic which can be selected by the vDSO builder if kernel
// command line args have been passed which indicate that zx_clock_get_monotonic
// should _always_ be a syscall.
