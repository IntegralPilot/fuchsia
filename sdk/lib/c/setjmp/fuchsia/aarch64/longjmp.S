// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/asm.h>
#include <zircon/tls.h>

#include "asm-linkage.h"
#include "../jmp_buf.h"


.macro restore_pair reg1, reg2, index
  ldp \reg1, \reg2, [x0, #8*\index]
  .cfi_same_value \reg1
  .cfi_same_value \reg2
.endm

jmp_buf.llvm_libc_function longjmp
  // The caller's x0 value is actually recoverable throughout, though
  // its default state is undefined (call-clobbered).  Likewise, the
  // caller's x1 value is recoverable until the apparent caller is setjmp.
  .cfi_same_value x0
  .cfi_same_value x1

  // The sanitizer runtime wants to be informed of non-local exits.
  // Call __asan_handle_no_return() before doing the actual longjmp.
#if __has_feature(address_sanitizer)
  // Save our incoming argument registers and return address on the stack
  // around calling __asan_handle_no_return.  We save the zero word (xzr)
  // just to keep the stack aligned correctly as required by the ABI.
  stp.spill x0, x1
  stp.spill x30, xzr
  .label LIBC_ASM_LINKAGE(longjmp_sanitizer_call), global
  bl __asan_handle_no_return
  .label LIBC_ASM_LINKAGE(longjmp_sanitizer_return), global
  ldp.reload x30, xzr
  ldp.reload x0, x1
#endif

  // Find the manglers.
  adr_global x2, LIBC_ASM_LINKAGE(gJmpBufManglers)

  // Load the words that need to be demangled into temporaries, along with
  // the corresponding mangler words.
  ldp x3, x4, [x0, #8*JB_PC]  // x3 <= mangled PC (x30); x4 <= mangled FP
  ldp x7, x8, [x2, #8*JB_PC]  // x7 <= PC mangler; x8 <= FP mangler
  ldp x5, x6, [x0, #8*JB_SP]  // x5 <= mangled SP; x6 <= mangled SCSP (x18)
  ldp x9, x2, [x2, #8*JB_SP]  // x9 <= SP mangler; x2 <= SCSP mangler

  // The next instruction clobbers the state of longjmp's caller.  So from here
  // on, we'll use CFI that unwinds to setjmp's caller instead.  Both callers
  // have the same x0 value.  The CFA rule is unchanged and continues to report
  // longjmp's CFA (the SP on entry).  The setjmp caller's x1 was never saved.
  .cfi_undefined x1

  // CFI to compute x\regno as (x\tmpno ^ x\mangler).
  .macro jb_cfi_mangled_temp regno, tmpno, mangler
    .cfi_escape DW_CFA_val_expression, \regno, 5, \
                DW_OP_breg(\tmpno), 0, DW_OP_breg(\mangler), 0, DW_OP_xor
  .endm

  jb_cfi_mangled_temp 30, 3, 7
  jb_cfi_mangled_temp 29, 4, 8
  jb_cfi_mangled_temp 31, 5, 9
  jb_cfi_mangled_temp 18, 6, 2

  jmp_buf.cfi 0, 19, JB_X(19)
  jmp_buf.cfi 0, 20, JB_X(20)
  jmp_buf.cfi 0, 21, JB_X(21)
  jmp_buf.cfi 0, 22, JB_X(22)
  jmp_buf.cfi 0, 23, JB_X(23)
  jmp_buf.cfi 0, 24, JB_X(24)
  jmp_buf.cfi 0, 25, JB_X(25)
  jmp_buf.cfi 0, 26, JB_X(26)
  jmp_buf.cfi 0, 27, JB_X(27)
  jmp_buf.cfi 0, 28, JB_X(28)
  jmp_buf.cfi 0, 64+8, JB_D(8)
  jmp_buf.cfi 0, 64+9, JB_D(9)
  jmp_buf.cfi 0, 64+10, JB_D(10)
  jmp_buf.cfi 0, 64+11, JB_D(11)
  jmp_buf.cfi 0, 64+12, JB_D(12)
  jmp_buf.cfi 0, 64+13, JB_D(13)
  jmp_buf.cfi 0, 64+14, JB_D(14)

  // Restore all the vanilla callee-saves registers.
  restore_pair x19, x20, JB_X(19)
  restore_pair x21, x22, JB_X(21)
  restore_pair x23, x24, JB_X(23)
  restore_pair x25, x26, JB_X(25)
  restore_pair x27, x28, JB_X(27)
  restore_pair  d8,  d9, JB_D(8)
  restore_pair d10, d11, JB_D(10)
  restore_pair d12, d13, JB_D(12)
  restore_pair d14, d15, JB_D(14)

  // Demangle temporaries.  Be careful not to leak these values on return!
  eor x5, x5, x9         // SP
  .cfi_register sp, x5
  eor x6, x6, x2         // Shadow call stack pointer
  .cfi_register x18, x6
  eor x4, x4, x8         // FP (x29)
  .cfi_register x29, x4
  eor x2, x3, x7         // PC (LR / x30)
  .cfi_register x30, x2

  // x9 will now hold the saved checksum to be checked against.
  ldr x9, [x0, #8*JB_CHECKSUM]

  // Accumulate the CRC32C of all the saved registers (post-demangling) in w8,
  // in the same order setjmp computed them: sp, x18, x19..x28, x29, x30.
  crc32cx w8, wzr, x5
  .irp n,6,19,20,21,22,23,24,25,26,27,28,4,2
    crc32cx w8, w8, x\n
  .endr
  .irp n,8,9,10,11,12,13,14,15
    fmov x7, d\n
    crc32cx w8, w8, x7
  .endr

  // Finally, insert the low bits of the mangled PC into the high bits of the
  // checksum, as setjmp does.
  bfi x8, x3, #32, #32

  // If the checksums don't match, do a tail call to panic.
  cmp x8, x9
  bne LIBC_ASM_LINKAGE(longjmp_corrupted)

  // Restore the fixed registers from the demangled temporaries.
  mov x30, x2
  .cfi_same_value x30
  mov x18, x6
  .cfi_same_value x18
  mov x29, x4
  .cfi_same_value x29

  // Restore SP last.  After this, the CFA is the setjmp frame's CFA again.
  mov sp, x5
  .cfi_restore sp

  // Don't leak the raw values.  Temporaries that just have copies of
  // the jmp_buf words don't need to be cleared.
  mov x2, xzr
  mov x4, xzr
  mov x5, xzr
  mov x6, xzr

  // w0 = w1 ?: 1
  cmp w1, wzr
  csinc w0, w1, wzr, ne
  .cfi_undefined x0

  ret

jmp_buf.end_function
