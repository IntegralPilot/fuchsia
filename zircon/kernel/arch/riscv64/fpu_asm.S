// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/asm.h>
#include <arch/regs.h>
#include <arch/riscv64.h>

// Put these routines in this file separate from others so the F and D bits
// of the architecture can be added to allow for floating point instructions
.option arch, +f, +d

// Invoke `op N` for N in [0,31).
.macro on_fpu_regs op
  .irp n,0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    \op \n
  .endr
.endm

// Note these functions don't emit any CFI directives for the f<N> registers.
// Those register values are never part of the kernel's own program state per
// se, so there is nothing to say about them.  They are just hardware state
// that's external to the kernel as a program per se, like the CSRs.

// Zero out the FPU single and double precision hardware state
// void riscv64_fpu_zero();
.function riscv64_fpu_zero, global
  .llvm.stack_size 0

  // Enable the FPU so we don't trap while setting it.
  li a0, RISCV64_CSR_SSTATUS_FS_DIRTY
  csrs sstatus, a0

  csrw fcsr, zero

  .macro fmv.d.zero n
    fmv.d.x f\n, zero
  .endm
  on_fpu_regs fmv.d.zero

  // Set the FPU hardware to the initial state.  Clear the top bit
  // of the two bit field to go from dirty (0b11) to initial (0b01)
  // via the clean state (0b10).
  li a0, RISCV64_CSR_SSTATUS_FS_CLEAN
  csrc sstatus, a0

  ret
.end_function

// void riscv64_fpu_save(riscv64_fpu_state *state)
.function riscv64_fpu_save, global
  .llvm.stack_size 0

  .macro fsd.a0 n
    fsd f\n, 8*\n(a0)
  .endm
  on_fpu_regs fsd.a0

  csrr a1, fcsr
  sw a1, 8*32(a0)

  ret
.end_function

// void riscv64_fpu_restore(const riscv64_fpu_state *state)
.function riscv64_fpu_restore, global
  .llvm.stack_size 0

  .macro fld.a0 n
    fld f\n, 8*\n(a0)
  .endm
  on_fpu_regs fld.a0

  lw a1, 8*32(a0)
  csrw fcsr, a1

  ret
.end_function
