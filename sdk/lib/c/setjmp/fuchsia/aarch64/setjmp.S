// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/asm.h>
#include <zircon/tls.h>

#include "asm-linkage.h"
#include "../jmp_buf.h"


jmp_buf.llvm_libc_function setjmp
  // The caller's x0 value is actually recoverable throughout, though
  // its default state is undefined (call-clobbered).
  .cfi_same_value x0

  // Load the manglers into temporary registers.  We'll ensure that by return
  // no registers contain the raw manglers, so those values never leak.
  adr_global x1, LIBC_ASM_LINKAGE(gJmpBufManglers)
  // x2 <= PC mangler; x3 <= FP mangler
  ldp x2, x3, [x1, #8*JB_PC]
  // x1 <= SP mangler; x4 <= SCSP mangler
  ldp x1, x4, [x1, #8*JB_SP]

  // Store all the vanilla callee-saves registers.
  stp x19, x20, [x0, #8*JB_X(19)]
  stp x21, x22, [x0, #8*JB_X(21)]
  stp x23, x24, [x0, #8*JB_X(23)]
  stp x25, x26, [x0, #8*JB_X(25)]
  stp x27, x28, [x0, #8*JB_X(27)]
  stp  d8,  d9, [x0, #8*JB_D(8)]
  stp d10, d11, [x0, #8*JB_D(10)]
  stp d12, d13, [x0, #8*JB_D(12)]
  stp d14, d15, [x0, #8*JB_D(14)]

  eor x2, x2, x30      // Mangled PC (LR).
  eor x3, x3, x29      // Mangled FP.
  stp x2, x3, [x0, #8*JB_PC]

  eor x4, x4, x18      // Mangled shadow call stack pointer.
  mov x3, sp
  eor x1, x1, x3       // Mangled SP.
  stp x1, x4, [x0, #8*JB_SP]

  // Accumulate the CRC32C of all the saved registers (pre-mangling) in w3 in
  // the order longjmp will expect: sp, x18, x19..x28, x29, x30.  This clobbers
  // the raw SP value, so the only values leaked in registers are the values
  // going directly into the jmp_buf.
  crc32cx w3, wzr, x3
  .irp n,18,19,20,21,22,23,24,25,26,27,28,29,30
    crc32cx w3, w3, x\n
  .endr
  .irp n,8,9,10,11,12,13,14,15
    fmov x1, d\n
    crc32cx w3, w3, x1
  .endr

  // The CRC32C only occupies the low 32 bits of the checksum word, leaving the
  // high 32 bits unused.  So swizzle in the low bits of the mangled PC for
  // good measure.
  bfi x3, x2, #32, #32
  str x3, [x0, #8*JB_CHECKSUM]

  mov w0, wzr
  .cfi_undefined x0
  ret

jmp_buf.end_function
