// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/asm.h>
#include <zircon/tls.h>

#include "../jmp_buf.h"

jmp_buf.llvm_libc_function setjmp
  .cfi_same_value a0

  // Each mangled register uses a temporary for the mangler / mangled value.
  .macro on_mangled macro
    \macro ra, JB_PC, a2
    \macro fp, JB_FP, a3
    \macro sp, JB_SP, a4
    \macro gp, JB_SCSP, a5
  .endm

  // Load the manglers into registers.
  lla a1, LIBC_ASM_LINKAGE(gJmpBufManglers)
  .macro load_mangler reg, index, temp
    ld \temp, \index*8(a1)
  .endm
  on_mangled load_mangler

  // Store the vanilla call-saved registers, accumlating XOR of them all in a1.
  .irp n,1,2
    sd s\n, JB_S(\n)*8(a0)
  .endr
  xor a1, s1, s2
  .irp n,3,4,5,6,7,8,9,10,11
    sd s\n, JB_S(\n)*8(a0)
    xor a1, a1, s\n
  .endr

  // Mangle the values into the temporaries.
  .macro mangle reg, index, temp
    xor \temp, \temp, \reg
  .endm
  on_mangled mangle

  // Store the mangled values and fold them into the checksum.
  .macro store_mangled reg, index, temp
    sd \temp, \index*8(a0)
    xor a1, a1, \temp
  .endm
  on_mangled store_mangled

#ifndef __riscv_float_abi_soft
  // Store call-saved FP registers.  The checksum XOR needs an integer register
  // so each FP register is moved into an integer register to be stored.  There
  // are enough call-clobbered registers to give each one its own temporary.
  .macro on_fs macro
    \macro 0, a2
    \macro 1, a3
    \macro 2, a4
    \macro 3, a5
    \macro 4, a6
    \macro 5, a7
    \macro 6, t0
    \macro 7, t1
    \macro 8, t2
    \macro 9, t3
    \macro 10, t4
    \macro 11, t5
  .endm
  .macro fetch_fs n, temp
    fmv.x.d \temp, fs\n
  .endm
  .macro store_fs n, temp
    sd \temp, JB_FS(\n)*8(a0)
    xor a1, a1, \temp
  .endm
  on_fs fetch_fs
  on_fs store_fs
#endif

  // Finally, store the checksum word.
  sd a1, JB_CHECKSUM*8(a0)

  mv a0, zero
  .cfi_undefined a0
  ret

jmp_buf.end_function
