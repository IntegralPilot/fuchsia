// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/asm.h>
#include <zircon/tls.h>

#include "../jmp_buf.h"

jmp_buf.llvm_libc_function longjmp
  // The caller's a0 value is actually recoverable throughout, though
  // its default state is undefined (call-clobbered).  Likewise, the
  // caller's a1 value is recoverable until the apparent caller is setjmp.
  .cfi_same_value a0
  .cfi_same_value a1

  // The sanitizer runtime wants to be informed of non-local exits.
  // Call __asan_handle_no_return() before doing the actual longjmp.
#if __has_feature(address_sanitizer)
  // Save our incoming argument registers and return address on the stack
  // around calling __asan_handle_no_return.
  add sp, sp, -32
  .cfi_adjust_cfa_offset 32
  sd ra, 0(sp)
  .cfi_rel_offset ra, 0
  sd a0, 8(sp)
  .cfi_rel_offset a0, 8
  sd a1, 16(sp)
  .cfi_rel_offset a1, 16
  .label LIBC_ASM_LINKAGE(longjmp_sanitizer_call), global
  call __asan_handle_no_return
  .label LIBC_ASM_LINKAGE(longjmp_sanitizer_return), global
  ld ra, 0(sp)
  .cfi_same_value ra
  ld a0, 8(sp)
  .cfi_same_value a0
  ld a1, 16(sp)
  .cfi_same_value a1
  add sp, sp, 32
  .cfi_adjust_cfa_offset -32
#endif

  // Find the manglers.
  lla a2, LIBC_ASM_LINKAGE(gJmpBufManglers)

  // Load the saved checksum.  It's just an XOR of saved values, so we'll XOR
  // all the values with it and that should yield zero at the end.
  ld t0, JB_CHECKSUM*8(a0)

  // Each register that's stored mangled has a corresponding mangler word.
  // The mangled values go into a3..a6 with the manglers in t3..t6.
  .macro on_mangled macro // reg, regno, n, index
    \macro ra, 1, 3, JB_PC    // ra = x1
    \macro s0, 8, 4, JB_FP    // s0 (fp) = x8
    \macro sp, 2, 5, JB_SP    // sp = x2
    \macro gp, 3, 6, JB_SCSP  // gp (scsp) = x3
  .endm

  // The PC (ra) and FP (s0) need to be demangled via temporaries first.
  .macro load_mangled reg, regno, n, index
    ld a\n, 8*\index(a0)
  .endm
  .macro load_mangler reg, regno, n, index
    ld t\n, 8*\index(a2)
  .endm
  on_mangled load_mangled
  on_mangled load_mangler

  // The next instruction clobbers the state of longjmp's caller.  So from here
  // on, we'll use CFI that unwinds to setjmp's caller instead.  Both callers
  // have the same a0 value (the jmp_buf).  The setjmp caller's a1 value cannot
  // be recovered.
  .cfi_undefined a1

  // CFI to compute x\regno as (a\tmpno ^ t\mangler).
  .macro jb_cfi_mangled reg, regno, n, index
    // a3..a6 = x13..x16, t3..t6 = x28..x31
    .cfi_escape DW_CFA_val_expression, \regno, 5, \
                DW_OP_breg(10 + \n), 0, DW_OP_breg(25 + \n), 0, DW_OP_xor
  .endm
  on_mangled jb_cfi_mangled

  // CFI to find s\n at JB_S(\n)*8(a0).
  jmp_buf.cfi 10, 9, JB_S(1)  // s1 = x9
  .irp n,2,3,4,5,6,7,8,9,10,11
    jmp_buf.cfi 10, (16 + \n), JB_S(\n)  // s2..s11 = x18..x27
  .endr

#ifndef __riscv_float_abi_soft
  // CFI to find fs\n at JB_FS(\n)*8(a0).
  .irp n,0,1
    jmp_buf.cfi 10, (32 + 8 + \n), JB_FS(\n)
  .endr
  .irp n,2,3,4,5,6,7,8,9,10,11
    jmp_buf.cfi 10, (32 + 18 + \n - 2), JB_FS(\n)
  .endr
#endif

#ifndef __riscv_float_abi_soft
  // Restore the call-saved FP registers.  First load each one into an integer
  // register, clobbering all the call-saved integer registers.  Then fold the
  // bits into the checksum and move them into the corresponding FP register.
  ld t1, JB_FS(0)*8(a0)  // Don't clobber s0, which is the fp.
  .irp n,1,2,3,4,5,6,7,8,9,10,11
    ld s\n, JB_FS(\n)*8(a0)
  .endr
  xor t0, t0, t1
  fmv.d.x fs0, t1
  .cfi_same_value fs0
  .irp n,1,2,3,4,5,6,7,8,9,10,11
    xor t0, t0, s\n
    fmv.d.x fs\n, s\n
    .cfi_same_value fs\n
  .endr
#endif

  // Restore the vanilla call-saved registers.
  .irp n,1,2,3,4,5,6,7,8,9,10,11
    ld s\n, JB_S(\n)*8(a0)
    .cfi_same_value s\n
  .endr

  // Fold all those into the checksum.
  .irp n,1,2,3,4,5,6,7,8,9,10,11
    xor t0, t0, s\n
  .endr

  // Fold the mangled values loaded earlier into the checksum.
  .macro csum_mangled reg, regno, n, index
    xor t0, t0, a\n
  .endm
  on_mangled csum_mangled

  // Everything has been XOR'd together with the saved checksum value.
  // So it should now be zero.  If not, do a tail call to panic.
  bnez t0, .Lcorrupt

  // Demangle and restore the fixed registers.  When the setjmp frame's SP
  // (also its CFA) is restored, it becomes the CFA again here.
  .macro restore_demangled reg, regno, n, index
    xor \reg, a\n, t\n
    .cfi_restore \reg
  .endm
  on_mangled restore_demangled

  // Don't leak raw values.
  .macro clear_demangled reg, regno, n, index
    mv a\n, zero
    mv t\n, zero
  .endm
  on_mangled clear_demangled

  seqz a0, a1                   // a0 = (a1 == 0) ? 1 : 0
  .cfi_undefined a0             // Caller's a0 is no longer recoverable.
  add a0, a0, a1                // a0 += a1; AKA a0 = (a1 == 0) ? 1 : a1
  ret

.Lcorrupt:
  tail LIBC_ASM_LINKAGE(longjmp_corrupted)

jmp_buf.end_function
