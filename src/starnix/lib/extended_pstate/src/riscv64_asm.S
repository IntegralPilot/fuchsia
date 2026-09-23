// Copyright 2023 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <starnix_asm_utils/asm.h>

// Offsets for `State` fields.
// LINT.IfChange(riscv64_state_offsets)
#define STATE_FCSR_OFFSET 256
#define STATE_V_REGISTERS_OFFSET 272
#define STATE_VCSRS_OFFSET 784
// LINT.ThenChange(riscv64.rs:riscv64_state_offsets)

// Offsets for `RiscvVectorCsrs` fields.
// LINT.IfChange(riscv64_vector_csrs_offsets)
#define CSRS_STRUCT_VCSR (0)
#define CSRS_STRUCT_VSTART (1 * 8)
#define CSRS_STRUCT_VL (2 * 8)
#define CSRS_STRUCT_VTYPE (3 * 8)
// LINT.ThenChange(riscv64.rs:riscv64_vector_csrs_offsets)

FUNCTION(save_extended_pstate)
    // state_addr is in a0
    ld t0, 0(a0) // t0 is state_ptr (pointer to State)

    // Save FP registers
    fsd  f0,  0 * 8(t0)
    fsd  f1,  1 * 8(t0)
    fsd  f2,  2 * 8(t0)
    fsd  f3,  3 * 8(t0)
    fsd  f4,  4 * 8(t0)
    fsd  f5,  5 * 8(t0)
    fsd  f6,  6 * 8(t0)
    fsd  f7,  7 * 8(t0)
    fsd  f8,  8 * 8(t0)
    fsd  f9,  9 * 8(t0)
    fsd f10, 10 * 8(t0)
    fsd f11, 11 * 8(t0)
    fsd f12, 12 * 8(t0)
    fsd f13, 13 * 8(t0)
    fsd f14, 14 * 8(t0)
    fsd f15, 15 * 8(t0)
    fsd f16, 16 * 8(t0)
    fsd f17, 17 * 8(t0)
    fsd f18, 18 * 8(t0)
    fsd f19, 19 * 8(t0)
    fsd f20, 20 * 8(t0)
    fsd f21, 21 * 8(t0)
    fsd f22, 22 * 8(t0)
    fsd f23, 23 * 8(t0)
    fsd f24, 24 * 8(t0)
    fsd f25, 25 * 8(t0)
    fsd f26, 26 * 8(t0)
    fsd f27, 27 * 8(t0)
    fsd f28, 28 * 8(t0)
    fsd f29, 29 * 8(t0)
    fsd f30, 30 * 8(t0)
    fsd f31, 31 * 8(t0)

    // Save fcsr
    csrr t1, fcsr
    sw t1, STATE_FCSR_OFFSET(t0)

    addi a0, t0, STATE_V_REGISTERS_OFFSET
    addi a1, t0, STATE_VCSRS_OFFSET

    // Save Vector CSR registers
    csrrw   t0, vstart, zero
    sd      t0, CSRS_STRUCT_VSTART(a1)

    csrr    t0, vcsr
    sd      t0, CSRS_STRUCT_VCSR(a1)

    csrr    t0, vl
    sd      t0, CSRS_STRUCT_VL(a1)

    csrr    t0, vtype
    sd      t0, CSRS_STRUCT_VTYPE(a1)

    // Save V registers, 8 at a time. `vstart` was zeroed by the `csrrw` above.
    csrr    t0, vlenb
    slli    t0, t0, 3
    vs8r.v  v0, (a0)
    add     a0, a0, t0
    vs8r.v  v8, (a0)
    add     a0, a0, t0
    vs8r.v  v16, (a0)
    add     a0, a0, t0
    vs8r.v  v24, (a0)

    ret
END_FUNCTION(save_extended_pstate)

FUNCTION(restore_extended_pstate)
    // state_addr is in a0
    ld t0, 0(a0) // t0 is state_ptr (pointer to State)

    // Restore FP registers
    fld  f0,  0 * 8(t0)
    fld  f1,  1 * 8(t0)
    fld  f2,  2 * 8(t0)
    fld  f3,  3 * 8(t0)
    fld  f4,  4 * 8(t0)
    fld  f5,  5 * 8(t0)
    fld  f6,  6 * 8(t0)
    fld  f7,  7 * 8(t0)
    fld  f8,  8 * 8(t0)
    fld  f9,  9 * 8(t0)
    fld f10, 10 * 8(t0)
    fld f11, 11 * 8(t0)
    fld f12, 12 * 8(t0)
    fld f13, 13 * 8(t0)
    fld f14, 14 * 8(t0)
    fld f15, 15 * 8(t0)
    fld f16, 16 * 8(t0)
    fld f17, 17 * 8(t0)
    fld f18, 18 * 8(t0)
    fld f19, 19 * 8(t0)
    fld f20, 20 * 8(t0)
    fld f21, 21 * 8(t0)
    fld f22, 22 * 8(t0)
    fld f23, 23 * 8(t0)
    fld f24, 24 * 8(t0)
    fld f25, 25 * 8(t0)
    fld f26, 26 * 8(t0)
    fld f27, 27 * 8(t0)
    fld f28, 28 * 8(t0)
    fld f29, 29 * 8(t0)
    fld f30, 30 * 8(t0)
    fld f31, 31 * 8(t0)

    // Restore fcsr
    lw t1, STATE_FCSR_OFFSET(t0)
    fscsr t1

    // Restore vector registers
    addi a0, t0, STATE_V_REGISTERS_OFFSET
    addi a1, t0, STATE_VCSRS_OFFSET

    // Restore Vector CSR registers
    lw      t0, CSRS_STRUCT_VCSR(a1)
    csrw    vcsr, a1

    lw      t0, CSRS_STRUCT_VL(a1)
    lw      t1, CSRS_STRUCT_VTYPE(a1)
    vsetvl  x0, t0, t1

    // Restore V registers, 8 at a time. `vstart` was zeroed by the `vsetvl` above.
    csrr   t0, vlenb
    slli   t0, t0, 3
    vl8r.v v0, (a0)
    add    a0, a0, t0
    vl8r.v v8, (a0)
    add    a0, a0, t0
    vl8r.v v16, (a0)
    add    a0, a0, t0
    vl8r.v v24, (a0)

    // `vsetvl` and `vl8r.v` reset `vstart`, so this register has to be restored the last.
    lw      a1, CSRS_STRUCT_VSTART(a1)
    csrw    vstart, a1

    ret
END_FUNCTION(restore_extended_pstate)
