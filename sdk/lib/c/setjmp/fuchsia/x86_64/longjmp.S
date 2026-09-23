// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/asm.h>
#include <zircon/tls.h>

#include "asm-linkage.h"
#include "../jmp_buf.h"


.macro restore reg, index
  mov 8*\index(%rdi), \reg
  .cfi_same_value \reg
.endm

.macro demangle index, reg
  xor LIBC_ASM_LINKAGE(gJmpBufManglers)+8*\index(%rip), \reg
.endm

jmp_buf.llvm_libc_function longjmp
  // The caller's %rdi remains in place throughout, and is the same for setjmp.
  .cfi_same_value %rdi

  // The sanitizer runtime wants to be informed of non-local exits.
  // Call __asan_handle_no_return() before doing the actual longjmp.
#if __has_feature(address_sanitizer)
  // Save our incoming argument registers on the stack around calling
  // __asan_handle_no_return.  The incoming stack is misaligned by one word, so
  // we need to move the stack by an odd number of words, hence the unnecessary
  // save/restore of %rax.  There's no CFI rule for %rax or %rsi since they are
  // already .cfi_undefined by default.
  push %rax
  .cfi_adjust_cfa_offset 8
  push %rsi
  .cfi_adjust_cfa_offset 8
  push.spill %rdi
  .label LIBC_ASM_LINKAGE(longjmp_sanitizer_call), global
  call __asan_handle_no_return@PLT
  .label LIBC_ASM_LINKAGE(longjmp_sanitizer_return), global
  pop.reload %rdi
  pop %rsi
  .cfi_adjust_cfa_offset -8
  pop %rax
  .cfi_adjust_cfa_offset -8
#endif

  // Calculate return value: %r11d = %esi ?: 1
  mov $1, %r11d
  test %esi, %esi
  cmovnz %esi, %r11d

  // Load the mangled values into temporaries.
  mov 8*JB_PC(%rdi), %rax
  mov 8*JB_SP(%rdi), %rcx
  mov 8*JB_FP(%rdi), %rdx
  mov 8*JB_USP(%rdi), %rsi

  // Keep the low bits of the mangled PC to check against checksum high bits.
  mov %rax, %r8
  shl $32, %r8

  // Demangle each temporary.
  demangle JB_PC, %rax
  demangle JB_SP, %rcx
  demangle JB_FP, %rdx
  demangle JB_USP, %rsi

  // The next instruction clobbers the state of longjmp's caller.
  // So from here on, we'll use CFI that unwinds to setjmp's caller instead.
  // Both callers have the same %rdi value, which we're still using.
  // The vanilla registers are just found in the jmp_buf via %rdi now.
  jmp_buf.cfi 5, 3, JB_RBX
  jmp_buf.cfi 5, 12, JB_R12
  jmp_buf.cfi 5, 13, JB_R13
  jmp_buf.cfi 5, 14, JB_R14
  jmp_buf.cfi 5, 15, JB_R15

  // The CFA is now the setjmp frame's CFA.  The default CFA rule is relative
  // to %rsp, so now point it at where setjmp's %rsp sits now.  The default
  // rule for the caller's %rsp in turn is relative to the CFA, so that's
  // already correct for the setjmp frame now with the new CFA.
  .cfi_def_cfa_register %rcx

  // The setjmp caller's %rip (default return address column) is not in the
  // usual place on that CFA-derived stack anymore.  Instead it's now found
  // among the temporary registers as are the other mangled registers.
  .cfi_register %rip, %rax
  .cfi_register %rbp, %rdx
  //.cfi_register %unsafe_sp, %rsi  -- No DWARF register number for it!

  // The setjmp caller's return value register is now "recoverable".
  // It's not really the caller's value, but it's what will be restored.
  .cfi_register %rax, %r11

  // Restore all the vanilla callee-saves registers.
  restore %rbx, JB_RBX
  restore %r12, JB_R12
  restore %r13, JB_R13
  restore %r14, JB_R14
  restore %r15, JB_R15

  // Accumulate the CRC32 of saved values in the same order setjmp does.  Note
  // this uses the register values after loading them from the jmp_buf, rather
  // than checking the values in memory, to avoid a TOCTTOU error.
  xor %r9, %r9
  crc32 %rax, %r9  // PC
  crc32 %rcx, %r9  // SP
  crc32 %rdx, %r9  // FP
  crc32 %rsi, %r9  // Unsafe SP
  crc32 %rbx, %r9
  crc32 %r12, %r9
  crc32 %r13, %r9
  crc32 %r14, %r9
  crc32 %r15, %r9

  // The stored checksum has the CRC32 in its low bits; its high bits copy the
  // low bits of the mangled PC, which were saved in the high bits of %r8.  If
  // the whole stored checksum word doesn't match, do a tail call to panic.
  or %r8, %r9
  cmp %r9, 8*JB_CHECKSUM(%rdi)
  jne LIBC_ASM_LINKAGE(longjmp_corrupted)

  // Restore the demangled values.
  mov %rdx, %rbp
  .cfi_same_value %rbp
  mov %rsi, %fs:ZX_TLS_UNSAFE_SP_OFFSET
  //.cfi_same_value %unsafe_sp  -- No DWARF register number for it!

  // The setjmp return address goes back onto its original place on setjmp's
  // stack.  So the default CFA-based rule to find it works again.
  mov %rax, (%rcx)
  .cfi_restore %rip

  // Restore SP last, and the default CFA rule with it.
  mov %rcx, %rsp
  .cfi_def_cfa_register %rsp

  // Don't leak the demangled values.
  xor %ecx, %ecx
  xor %edx, %edx
  xor %esi, %esi

  mov %r11d, %eax
  ret

jmp_buf.end_function
