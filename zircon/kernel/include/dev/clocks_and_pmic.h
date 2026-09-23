// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_INCLUDE_DEV_CLOCKS_AND_PMIC_H_
#define ZIRCON_KERNEL_INCLUDE_DEV_CLOCKS_AND_PMIC_H_

#include <lib/zx/result.h>

// Prepare the platforms-specific clocks and power rails for entering a
// suspended state.  This operation is required to be both idempotent and
// thread-safe.
zx_status_t clocks_and_pmic_prepare_for_suspend();

// Prepare the platforms-specific clocks and power rails for operation
// immediately after exiting a suspended state. This operation required to be
// both idempotent and thread-safe.
zx_status_t clocks_and_pmic_wakeup_from_suspend();

#endif  // ZIRCON_KERNEL_INCLUDE_DEV_CLOCKS_AND_PMIC_H_
