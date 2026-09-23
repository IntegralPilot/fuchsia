// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fasttime/internal/time.h>

#include "data-time-values.h"
#include "private.h"

__EXPORT zx_instant_boot_ticks_t _zx_ticks_get_boot(void) {
  return fasttime::internal::compute_boot_ticks<
      fasttime::internal::FasttimeVerificationMode::kSkip>(DATA_TIME_VALUES);
}

VDSO_INTERFACE_FUNCTION(zx_ticks_get_boot);
