// Copyright 2021 The Fuchsia Authors
// Copyright (c) 2016, Google, Inc. All rights reserved
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_DEV_INTERRUPT_GIC_V2_INCLUDE_DEV_INTERRUPT_ARM_GICV2_INIT_H_
#define ZIRCON_KERNEL_DEV_INTERRUPT_GIC_V2_INCLUDE_DEV_INTERRUPT_ARM_GICV2_INIT_H_

#include <lib/zbi-format/driver-config.h>

// Early initialization routines for the driver.
void ArmGicInitEarly(const zbi_dcfg_arm_gic_v2_driver_t& config);

// Post VM initialization step, able to use the VM to map registers.
void ArmGicInitPostVm(const zbi_dcfg_arm_gic_v2_driver_t& config);

// Any post threading initialization.
void ArmGicInitLate(const zbi_dcfg_arm_gic_v2_driver_t& config);

#endif  // ZIRCON_KERNEL_DEV_INTERRUPT_GIC_V2_INCLUDE_DEV_INTERRUPT_ARM_GICV2_INIT_H_
