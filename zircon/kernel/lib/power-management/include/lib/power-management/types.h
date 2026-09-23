// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_POWER_MANAGEMENT_INCLUDE_LIB_POWER_MANAGEMENT_TYPES_H_
#define ZIRCON_KERNEL_LIB_POWER_MANAGEMENT_INCLUDE_LIB_POWER_MANAGEMENT_TYPES_H_

#include <cstdint>

#include <ffl/fixed.h>

namespace power_management {

// The normalized processing rate of a CPU, relative to the fastest CPU in the system.
using ProcessingRate = ffl::Fixed<int64_t, 31>;

// The normalized utilization of a CPU by a task or set of tasks.
using Utilization = ffl::Fixed<int64_t, 31>;

// A time point on an indeterminate timeline.
using Time = ffl::Fixed<int64_t, 0>;

}  // namespace power_management

#endif  // ZIRCON_KERNEL_LIB_POWER_MANAGEMENT_INCLUDE_LIB_POWER_MANAGEMENT_TYPES_H_
