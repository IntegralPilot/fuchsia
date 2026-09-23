// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_KERNEL_LIB_SCHED_INCLUDE_LIB_SCHED_TIME_H_
#define ZIRCON_KERNEL_LIB_SCHED_INCLUDE_LIB_SCHED_TIME_H_

#include <zircon/time.h>

#include <ffl/fixed.h>

namespace sched {

// Fixed-point wrappers for cleaner time arithmetic when dealing with other
// fixed-point quantities.
using Duration = ffl::Fixed<zx_duration_mono_t, 0>;
using Time = ffl::Fixed<zx_instant_mono_t, 0>;

}  // namespace sched

#endif  // ZIRCON_KERNEL_LIB_SCHED_INCLUDE_LIB_SCHED_TIME_H_
