// Copyright 2026 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <starnix_asm_utils/asm.h>

// LINT.IfChange(aarch64_state_offsets)
#define FPCR_OFFSET 512
#define FPSR_OFFSET 516
// LINT.ThenChange(aarch64.rs:aarch64_state_offsets)

// LINT.IfChange(aarch32_state_offsets)
#define FPCR_OFFSET_32 256
#define FPSR_OFFSET_32 260
// LINT.ThenChange(aarch64.rs:aarch32_state_offsets)

FUNCTION(save_extended_pstate)
    // state_addr is in x0
    ldr x1, [x0] // state_ptr is in x1
    stp  q0,  q1, [x1, #( 0 * 32)]
    stp  q2,  q3, [x1, #( 1 * 32)]
    stp  q4,  q5, [x1, #( 2 * 32)]
    stp  q6,  q7, [x1, #( 3 * 32)]
    stp  q8,  q9, [x1, #( 4 * 32)]
    stp q10, q11, [x1, #( 5 * 32)]
    stp q12, q13, [x1, #( 6 * 32)]
    stp q14, q15, [x1, #( 7 * 32)]
    stp q16, q17, [x1, #( 8 * 32)]
    stp q18, q19, [x1, #( 9 * 32)]
    stp q20, q21, [x1, #(10 * 32)]
    stp q22, q23, [x1, #(11 * 32)]
    stp q24, q25, [x1, #(12 * 32)]
    stp q26, q27, [x1, #(13 * 32)]
    stp q28, q29, [x1, #(14 * 32)]
    stp q30, q31, [x1, #(15 * 32)]
    mrs x2, fpcr
    mrs x3, fpsr
    str w2, [x1, #(FPCR_OFFSET)]
    str w3, [x1, #(FPSR_OFFSET)]
    ret
END_FUNCTION(save_extended_pstate)

FUNCTION(restore_extended_pstate)
    // state_addr is in x0
    ldr x1, [x0] // state_ptr is in x1
    ldp  q0,  q1, [x1, #( 0 * 32)]
    ldp  q2,  q3, [x1, #( 1 * 32)]
    ldp  q4,  q5, [x1, #( 2 * 32)]
    ldp  q6,  q7, [x1, #( 3 * 32)]
    ldp  q8,  q9, [x1, #( 4 * 32)]
    ldp q10, q11, [x1, #( 5 * 32)]
    ldp q12, q13, [x1, #( 6 * 32)]
    ldp q14, q15, [x1, #( 7 * 32)]
    ldp q16, q17, [x1, #( 8 * 32)]
    ldp q18, q19, [x1, #( 9 * 32)]
    ldp q20, q21, [x1, #(10 * 32)]
    ldp q22, q23, [x1, #(11 * 32)]
    ldp q24, q25, [x1, #(12 * 32)]
    ldp q26, q27, [x1, #(13 * 32)]
    ldp q28, q29, [x1, #(14 * 32)]
    ldp q30, q31, [x1, #(15 * 32)]
    ldr w2, [x1, #(FPCR_OFFSET)]
    ldr w3, [x1, #(FPSR_OFFSET)]
    msr fpcr, x2
    msr fpsr, x3
    ret
END_FUNCTION(restore_extended_pstate)

FUNCTION(save_extended_aarch32_pstate)
    // state_addr is in x0
    ldr x1, [x0] // state_ptr is in x1
    stp  q0,  q1, [x1, #( 0 * 32)]
    stp  q2,  q3, [x1, #( 1 * 32)]
    stp  q4,  q5, [x1, #( 2 * 32)]
    stp  q6,  q7, [x1, #( 3 * 32)]
    stp  q8,  q9, [x1, #( 4 * 32)]
    stp q10, q11, [x1, #( 5 * 32)]
    stp q12, q13, [x1, #( 6 * 32)]
    stp q14, q15, [x1, #( 7 * 32)]
    mrs x2, fpcr
    mrs x3, fpsr
    str w2, [x1, #(FPCR_OFFSET_32)]
    str w3, [x1, #(FPSR_OFFSET_32)]
    ret
END_FUNCTION(save_extended_aarch32_pstate)

FUNCTION(restore_extended_aarch32_pstate)
    // state_addr is in x0
    ldr x1, [x0] // state_ptr is in x1
    ldp  q0,  q1, [x1, #( 0 * 32)]
    ldp  q2,  q3, [x1, #( 1 * 32)]
    ldp  q4,  q5, [x1, #( 2 * 32)]
    ldp  q6,  q7, [x1, #( 3 * 32)]
    ldp  q8,  q9, [x1, #( 4 * 32)]
    ldp q10, q11, [x1, #( 5 * 32)]
    ldp q12, q13, [x1, #( 6 * 32)]
    ldp q14, q15, [x1, #( 7 * 32)]
    ldr w2, [x1, #(FPCR_OFFSET_32)]
    ldr w3, [x1, #(FPSR_OFFSET_32)]
    msr fpcr, x2
    msr fpsr, x3
    ret
END_FUNCTION(restore_extended_aarch32_pstate)
