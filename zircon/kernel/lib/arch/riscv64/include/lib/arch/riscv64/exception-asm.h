// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_ARCH_RISCV64_INCLUDE_LIB_ARCH_RISCV64_EXCEPTION_ASM_H_
#define ZIRCON_KERNEL_LIB_ARCH_RISCV64_INCLUDE_LIB_ARCH_RISCV64_EXCEPTION_ASM_H_

// This file provides macros primarily useful for the assembly code that
// defines an exception vector routine that stvec points to.

#include <lib/arch/asm.h>

#ifdef __ASSEMBLER__  // clang-format off

// This sets up the CFI state to represent the vector entry point conditions.
// It should come after `.cfi_startproc simple`.  The code that saves the
// interrupted registers should then use CFI directives to stay consistent.
// NOTE! This doesn't define a CFA rule, since no stack-switching has been
// done.  That rule must be defined appropriately for what *will* become the
// exception handler's stack frame.
.macro .cfi.stvec
  // The "caller" is actually interrupted register state.  This means the
  // "return address" will be treated as the precise PC of the "caller", rather
  // than as its return address that's one instruction after the call site.
  .cfi_signal_frame

  // 64 is the canonical "alternate frame return column".
  .cfi_return_column 64
  // The interrupted PC is found in the sepc CSR, which has a DWARF number.
#ifndef __clang__  // TODO(https://fxbug.dev/42073127)
  .cfi_register 64, sepc
  // The previous sepc value is no longer available.
  .cfi_undefined sepc
#endif

  // All other registers still have their interrupted state.
  .cfi.all_integer .cfi_same_value
  .cfi.all_vectorfp .cfi_same_value
.endm

#endif  // clang-format on

#endif  // ZIRCON_KERNEL_LIB_ARCH_RISCV64_INCLUDE_LIB_ARCH_RISCV64_EXCEPTION_ASM_H_
