// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fasttime/internal/time.h>

#include "data-time-values.h"
#include "private.h"

// By default, zx_clock_get_boot will use libfasttime to compute the ticks value in user-mode.
__EXPORT zx_instant_boot_t _zx_clock_get_boot(void) {
  return fasttime::internal::compute_boot_time<fasttime::internal::FasttimeVerificationMode::kSkip>(
      DATA_TIME_VALUES);
}

VDSO_INTERFACE_FUNCTION(zx_clock_get_boot);

// If the registers needed to query ticks are not available in user-mode, or
// kernel command line args have been passed to force zx_ticks_get_boot to always be
// a syscall, then the kernel can choose to use this alternate implementation of
// zx_clock_get_boot instead.  It will perform the transformation from
// ticks to clock boot in user mode (just like the default version), but it will
// query its ticks from the via_kernel version of zx_ticks_get_boot.
VDSO_KERNEL_EXPORT zx_instant_boot_t CODE_clock_get_boot_via_kernel_ticks(void) {
  affine::Ratio ticks_to_time_ratio(DATA_TIME_VALUES.ticks_to_time_numerator,
                                    DATA_TIME_VALUES.ticks_to_time_denominator);
  return ticks_to_time_ratio.Scale(SYSCALL_zx_ticks_get_boot_via_kernel());
}

// Note: See alternates.ld for a definition of
// CODE_clock_get_boot_via_kernel, which is an alias for
// SYSCALL_zx_clock_get_boot_via_kernel.  This is a version of
// zx_clock_get_boot which can be selected by the vDSO builder if kernel
// command line args have been passed which indicate that zx_clock_get_boot
// should _always_ be a syscall.
