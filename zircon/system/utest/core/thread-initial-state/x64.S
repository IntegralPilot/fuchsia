// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/asm.h>

.macro check_general_purpose reg
  cmpq $0, \reg
  jne .Lexit
.endm

.macro check_xmm reg
  // toss the register down on the stack in the red zone
  // so we can compare with zero.
  // avoids using the more obvious ptest instruction, which is
  // only present in SSE4.1
  movdqu \reg, -16(%rsp)
  cmpq $0, -16(%rsp)
  jnz .Lexit
  cmpq $0, -8(%rsp)
  jnz .Lexit
.endm

.macro check_ymm reg
  vptest \reg, \reg
  jnz .Lexit
.endm

.macro check_mm reg
  movq \reg, %rax
  cmpq $0, %rax
  jne .Lexit
.endm

.macro check_segment reg
  mov \reg, %ax
  cmpw $0, %ax
  jne .Lexit
.endm

// [[noreturn]] void InitialStateTestThread(uint64_t* ptr, uint64_t value);
// Arguments are in %rdi, %rsi.
.function InitialStateTestThread, global
  jz .Lexit
  jc .Lexit
  js .Lexit
  jp .Lexit

  check_general_purpose %rax
  check_general_purpose %rbx
  check_general_purpose %rcx
  check_general_purpose %rdx
  check_general_purpose %rbp
  check_general_purpose %r8
  check_general_purpose %r9
  check_general_purpose %r10
  check_general_purpose %r11
  check_general_purpose %r12
  check_general_purpose %r13
  check_general_purpose %r14
  check_general_purpose %r15

  // Don't check cs/ss since those are set by the kernel explicitly
  check_segment %ds
  check_segment %es
  check_segment %fs
  check_segment %gs

  check_mm %mm0
  check_mm %mm1
  check_mm %mm2
  check_mm %mm3
  check_mm %mm4
  check_mm %mm5
  check_mm %mm6
  check_mm %mm7

  check_xmm %xmm0
  check_xmm %xmm1
  check_xmm %xmm2
  check_xmm %xmm3
  check_xmm %xmm4
  check_xmm %xmm5
  check_xmm %xmm6
  check_xmm %xmm7
  check_xmm %xmm8
  check_xmm %xmm9
  check_xmm %xmm10
  check_xmm %xmm11
  check_xmm %xmm12
  check_xmm %xmm13
  check_xmm %xmm14
  check_xmm %xmm15

  // Use Intel Vol 1 section 14.3 procedure for checking if AVX enabled

  mov $1, %eax
  cpuid
  // Check for OSXSAVE support (can we use xgetbv)
  test $(1<<27), %ecx
  jz .Lno_avx
  // Check for AVX support
  test $(1<<28), %ecx
  jz .Lno_avx
  // Check for operating system support for AVX
  mov $0, %ecx
  xgetbv
  and $0b110, %eax
  cmp $0b110, %eax
  jz .Lno_avx

  check_ymm %ymm0
  check_ymm %ymm1
  check_ymm %ymm2
  check_ymm %ymm3
  check_ymm %ymm4
  check_ymm %ymm5
  check_ymm %ymm6
  check_ymm %ymm7
  check_ymm %ymm8
  check_ymm %ymm9
  check_ymm %ymm10
  check_ymm %ymm11
  check_ymm %ymm12
  check_ymm %ymm13
  check_ymm %ymm14
  check_ymm %ymm15
.Lno_avx:

  // Store the expected value just below the incoming SP.
  // This leaves the SP correctly aligned to enter the vDSO (without a call).
  movabs $0x1234567890abcdef, %rax
  pushq %rax

  // Store the value from the second argument register to the pointer passed in
  // the first argument register, which tests that each had the expected value.
  mov %rsi, (%rdi)

.Lexit:
  jmp zx_thread_exit@PLT

.end_function
