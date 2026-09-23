// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/concurrent/seqlock.inc.h>

#include "clock-common.h"
#include "data-constants.h"
#include "private.h"

namespace {

template <typename ClockTransformationType>
inline zx_status_t zx_clock_get_details_mapped_impl(const void* clock_addr, uint64_t options,
                                                    void* out_details) {
  // We only understand v1 details structures right now.
  if ((((options & ZX_CLOCK_ARGS_VERSION_MASK) >> ZX_CLOCK_ARGS_VERSION_SHIFT) != 1) ||
      (out_details == nullptr)) {
    return ZX_ERR_INVALID_ARGS;
  }

  zx_clock_details_v1_t* out = reinterpret_cast<zx_clock_details_v1_t*>(out_details);
  return reinterpret_cast<const ClockTransformationType*>(clock_addr)->GetDetails(out);
}

}  // namespace

__EXPORT zx_status_t _zx_clock_get_details_mapped(const void* clock_addr, uint64_t options,
                                                  void* out_details) {
  return zx_clock_get_details_mapped_impl<VdsoClockTransformation>(clock_addr, options,
                                                                   out_details);
}

VDSO_INTERFACE_FUNCTION(zx_clock_get_details_mapped);

// If the registers needed to query ticks are not available in user-mode,
// provide a version of get_details which falls back on the kernel syscall
// version of tick-fetching.
VDSO_KERNEL_EXPORT zx_status_t CODE_clock_get_details_mapped_via_kernel(const void* clock_addr,
                                                                        uint64_t options,
                                                                        void* out_details) {
  return zx_clock_get_details_mapped_impl<VdsoForcedSyscallClockTransformation>(clock_addr, options,
                                                                                out_details);
}
