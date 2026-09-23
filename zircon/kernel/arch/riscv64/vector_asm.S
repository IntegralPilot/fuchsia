// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/asm.h>
#include <arch/regs.h>
#include <arch/riscv64.h>
#include <arch/riscv64/vector.h>

// Put these routines in this file separate from others so the V extension can
// be enabled in isolation.
.option arch, +v

// Zero out the vector hardware state.
// void riscv64_vector_zero();
.function riscv64_vector_zero, global
  .llvm.stack_size 0

  // Enable the vector so we don't trap while setting it.
  li a0, RISCV64_CSR_SSTATUS_VS_DIRTY
  csrs sstatus, a0

  csrw vcsr, zero

  // Operate on groups of 8 vectors of element byte width at a time.
  // (LMUL=8, SEW=8)
  csrr a0, vlenb
  slli a0, a0, 3
  vsetvli zero, a0, e8, m8, ta, ma

  vmv.v.i v0, 0
  vmv.v.i v8, 0
  vmv.v.i v16, 0
  vmv.v.i v24, 0

  // It is recommended that at reset, vtype.vill is set, the remaining bits in
  // vtype are zero, and vl is set to zero - so best to mirror that here.
  //
  // See 3.11. State of Vector Extension at Reset in the V spec.
  li a0, RISCV64_CSR_VTYPE_VILL
  vsetvl t0, x0, a0

  // Set the vector hardware to the initial state. Clear the top bit
  // of the two bit field to go from dirty (0b11) to initial (0b01)
  // via the clean state (0b10).
  li a0, RISCV64_CSR_SSTATUS_VS_CLEAN
  csrc sstatus, a0

  ret
.end_function

// void riscv64_vector_save(riscv64_vector_state *state)
.function riscv64_vector_save, global
  .llvm.stack_size 0

  // Any vector operation will reset `vstart`, so be sure to save this value
  // first. As another minor optimization we can zero out `vstart` after we
  // read it for the correctness of the stores below.
  csrrw a1, vstart, zero
  sd a1, RISCV64_VECTOR_STATE_VSTART(a0)

  csrr a1, vcsr
  sd a1, RISCV64_VECTOR_STATE_VCSR(a0)

  csrr a1, vl
  sd a1, RISCV64_VECTOR_STATE_VL(a0)

  csrr a1, vtype
  sd a1, RISCV64_VECTOR_STATE_VTYPE(a0)

  // For indexing into the memory at which we want to store 8 vector registers
  // at a time, compute 8 * VLENB.
  csrr a1, vlenb
  slli a1, a1, 3

  // `vstart` was zeroed in the csrrw above.
  addi t0, a0, RISCV64_VECTOR_STATE_V
  vs8r.v  v0, (t0)
  add t0, t0, a1
  vs8r.v  v8, (t0)
  add t0, t0, a1
  vs8r.v  v16, (t0)
  add t0, t0, a1
  vs8r.v  v24, (t0)

  ret
.end_function

// void riscv64_vector_restore(const riscv64_vector_state *state)
.function riscv64_vector_restore, global
  .llvm.stack_size 0

  lw a1, RISCV64_VECTOR_STATE_VCSR(a0)
  csrw vcsr, a1

  lw a1, RISCV64_VECTOR_STATE_VL(a0)
  lw a2, RISCV64_VECTOR_STATE_VTYPE(a0)
  vsetvl zero, a1, a2

  // For indexing into the memory from which we want to load 8 vector
  // registers at a time, compute 8 * VLENB.
  csrr a1, vlenb
  slli a1, a1, 3

  // `vstart` would have been reset by the `vsetvl` call above.
  addi t0, a0, RISCV64_VECTOR_STATE_V
  vl8r.v  v0, (t0)
  add t0, t0, a1
  vl8r.v  v8, (t0)
  add t0, t0, a1
  vl8r.v  v16, (t0)
  add t0, t0, a1
  vl8r.v  v24, (t0)

  // Any vector operation will reset `vstart`, so be sure to restore this
  // value last.
  lw a1, RISCV64_VECTOR_STATE_VSTART(a0)
  csrw vstart, a1

  ret
.end_function
