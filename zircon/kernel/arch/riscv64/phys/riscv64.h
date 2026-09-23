// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_RISCV64_PHYS_RISCV64_H_
#define ZIRCON_KERNEL_ARCH_RISCV64_PHYS_RISCV64_H_

#define PHYS_EXCEPTION_STATE_SIZE ((32 + 4) * 8)

#ifndef __ASSEMBLER__

#include <lib/arch/asm.h>
#include <stdint.h>

#include <phys/exception.h>

static_assert(offsetof(PhysExceptionState, regs.pc) == 0);
static_assert(sizeof(PhysExceptionState{}.regs) == 32 * 8);
static_assert(PHYS_EXCEPTION_STATE_SIZE == sizeof(PhysExceptionState));

// Defined in exception.S, calls C++ ArchPhysException.
extern "C" arch::AsmLabel ArchPhysExceptionEntry;

// Defined in exception.cc, calls generic PhysException.
extern "C" uint64_t ArchPhysException(PhysExceptionState& state);

#endif  // !__ASSEMBLER__

#endif  // ZIRCON_KERNEL_ARCH_RISCV64_PHYS_RISCV64_H_
