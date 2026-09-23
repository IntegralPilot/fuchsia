// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_ARM64_PHYS_SMCCC_H_
#define ZIRCON_KERNEL_ARCH_ARM64_PHYS_SMCCC_H_

#include <lib/arch/arm64/smccc.h>
#include <stdint.h>

// This is defined in assembly.  The first argument is the Function Identifier
// and the other arguments vary by function.
extern "C" uint64_t ArmSmcccCall(arch::ArmSmcccFunction function, uint64_t arg1 = 0,
                                 uint64_t arg2 = 0, uint64_t arg3 = 0);

#endif  // ZIRCON_KERNEL_ARCH_ARM64_PHYS_SMCCC_H_
