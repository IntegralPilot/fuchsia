// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fasttime/internal/time.h>

#include "data-time-values.h"
#include "private.h"

__EXPORT zx_instant_mono_ticks_t _zx_ticks_get(void) {
  return fasttime::internal::compute_monotonic_ticks<
      fasttime::internal::FasttimeVerificationMode::kSkip>(DATA_TIME_VALUES);
}

VDSO_INTERFACE_FUNCTION(zx_ticks_get);

// Note: See alternates.ld for a definition of CODE_ticks_get_via_kernel, which
// is an alias for SYSCALL_zx_ticks_get_via_kernel.  This is a version of
// zx_ticks_get which goes through a forced syscall.  It is selected by the vDSO
// builder at runtime for use on platforms where the hardware tick counter is
// not directly accessible by user mode code.
