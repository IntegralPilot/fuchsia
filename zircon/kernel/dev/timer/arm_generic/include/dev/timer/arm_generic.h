// Copyright 2016 The Fuchsia Authors
// Copyright (c) 2013, Google Inc. All rights reserved.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_DEV_TIMER_ARM_GENERIC_INCLUDE_DEV_TIMER_ARM_GENERIC_H_
#define ZIRCON_KERNEL_DEV_TIMER_ARM_GENERIC_INCLUDE_DEV_TIMER_ARM_GENERIC_H_

#include <lib/affine/ratio.h>
#include <lib/zbi-format/driver-config.h>
#include <sys/types.h>
#include <zircon/types.h>

template <bool AllowDebugPrint = false>
affine::Ratio arm_generic_timer_compute_conversion_factors(uint32_t cntfrq);

// Initializes the driver.
void ArmGenericTimerInit(const zbi_dcfg_arm_generic_timer_driver_t& config);

// Another shot at initialization post interrupt controller and VM.
void ArmGenericTimerInitPostVm(const zbi_dcfg_arm_generic_timer_driver_t& config);

// Indicates that we should be using the physical view of the system timer
// instead of the virtual view.
bool ArmUsePhysTimerInVdso();

#endif  // ZIRCON_KERNEL_DEV_TIMER_ARM_GENERIC_INCLUDE_DEV_TIMER_ARM_GENERIC_H_
