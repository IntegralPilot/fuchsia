// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/concurrent/seqlock.inc.h>

#include "clock-common.h"
#include "data-constants.h"
#include "private.h"

namespace {

template <typename ClockTransformationType>
inline zx_status_t zx_clock_read_mapped_impl(const void* clock_addr, zx_time_t* out_now) {
  if (out_now == nullptr) {
    return ZX_ERR_INVALID_ARGS;
  }
  return reinterpret_cast<const ClockTransformationType*>(clock_addr)->Read(out_now);
}

}  // namespace

__EXPORT zx_status_t _zx_clock_read_mapped(const void* clock_addr, zx_time_t* out_now) {
  return zx_clock_read_mapped_impl<VdsoClockTransformation>(clock_addr, out_now);
}

VDSO_INTERFACE_FUNCTION(zx_clock_read_mapped);

// If the registers needed to query ticks are not available in user-mode,
// provide a version of get_details which falls back on the kernel syscall
// version of tick-fetching.
VDSO_KERNEL_EXPORT zx_status_t CODE_clock_read_mapped_via_kernel(const void* clock_addr,
                                                                 zx_time_t* out_now) {
  return zx_clock_read_mapped_impl<VdsoForcedSyscallClockTransformation>(clock_addr, out_now);
}
