// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/asm.h>
#include <zircon/tls.h>

#include "asm-linkage.h"
#include "../jmp_buf.h"


.macro mangle index, reg
  xor LIBC_ASM_LINKAGE(gJmpBufManglers)+8*\index(%rip), \reg
.endm

jmp_buf.llvm_libc_function setjmp
  // The caller's %rdi value is actually recoverable throughout, though
  // its default state is undefined (call-clobbered).
  .cfi_same_value %rdi

  // Copy the values that need mangling into temporaries.
  mov (%rsp), %r11                            // PC
  mov %rsp, %rcx                              // SP
  mov %rbp, %rdx                              // FP
  mov %fs:ZX_TLS_UNSAFE_SP_OFFSET, %rsi       // Unsafe SP

  // Store all the vanilla callee-saves registers.
  mov %rbx, 8*JB_RBX(%rdi)
  mov %r12, 8*JB_R12(%rdi)
  mov %r13, 8*JB_R13(%rdi)
  mov %r14, 8*JB_R14(%rdi)
  mov %r15, 8*JB_R15(%rdi)

  // Accumulate the CRC32 of saved values in %rax.
  xor %eax, %eax
  crc32 %r11, %rax  // PC
  crc32 %rcx, %rax  // SP
  crc32 %rdx, %rax  // FP
  crc32 %rsi, %rax  // Unsafe SP
  crc32 %rbx, %rax
  crc32 %r12, %rax
  crc32 %r13, %rax
  crc32 %r14, %rax
  crc32 %r15, %rax

  // Mangle each temporary.
  mangle JB_PC, %r11
  mangle JB_SP, %rcx
  mangle JB_FP, %rdx
  mangle JB_USP, %rsi

  // Store the mangled values.
  mov %r11, 8*JB_PC(%rdi)
  mov %rcx, 8*JB_SP(%rdi)
  mov %rdx, 8*JB_FP(%rdi)
  mov %rsi, 8*JB_USP(%rdi)

  // Store the checksum.  The CRC32 leaves the 32 high bits unused.  So swizzle
  // in part of the mangled PC for good measure.
  mov %eax, 8*JB_CHECKSUM(%rdi)
  mov %r11d, 8*JB_CHECKSUM+4(%rdi)

  // %rax = 0
  xor %eax, %eax

  ret

jmp_buf.end_function
