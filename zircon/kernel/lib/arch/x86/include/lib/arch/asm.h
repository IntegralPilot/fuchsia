// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_ARCH_X86_INCLUDE_LIB_ARCH_ASM_H_
#define ZIRCON_KERNEL_LIB_ARCH_X86_INCLUDE_LIB_ARCH_ASM_H_

// Get the generic file.
#include_next <lib/arch/asm.h>

#ifdef __ASSEMBLER__  // clang-format off

.macro assert.fail
  ud2
.endm

.macro .cfi.all_integer op
  .irp reg,%rax,%rbx,%rcx,%rdx,%rsi,%rdi,%rbp,%rsp,%r8,%r9,%r10,%r11,%r12,%r13,%r14,%r15
    \op \reg
  .endr
.endm

.macro .cfi.all_vectorfp op
  .irp reg,%xmm0,%xmm1,%xmm2,%xmm3,%xmm4,%xmm5,%xmm6,%xmm7,%xmm8,%xmm9,%xmm10,%xmm11,%xmm12,%xmm13,%xmm14,%xmm15,%mm0,%mm1,%mm2,%mm3,%mm4,%mm5,%mm6,%mm7,%st(0),%st(1),%st(2),%st(3),%st(4),%st(5),%st(6),%st(7)
    \op \reg
  .endr
.endm

.macro .cfi.all_call_used op
  .irp reg,%rax,%rcx,%rdx,%rsi,%rdi,%r8,%r9,%r10,%r11
    \op \reg
  .endr
  .cfi.all_vectorfp \op
.endm

// Allow both 32-bit and 64-bit .prologue.fp and .epilogue.fp can share the
// same implementation, since they only differ in register name and size.
#if defined(__i386__)
#  define PROLOGUE_RSP %esp
#  define PROLOGUE_RBP %ebp
#  define PROLOGUE_REGSIZE 4
#else
#  define PROLOGUE_RSP %rsp
#  define PROLOGUE_RBP %rbp
#  define PROLOGUE_REGSIZE 8
#endif

// Standard prologue sequence for FP setup, with CFI.
// Note that this realigns the SP from entry state to be ready for a call.
// The argument doesn't affect the SP adjustment, it just affects the metadata
// for frame stack size (unless `no_metadata=no-metadata` is also given).
.macro .prologue.fp frame_extra_size_for_metadata=0, no_metadata=
  // This counts only the FP push done here, not the call instruction's push.
  .llvm.stack_size (\frame_extra_size_for_metadata + PROLOGUE_REGSIZE), \no_metadata
  push PROLOGUE_RBP
  .cfi_adjust_cfa_offset PROLOGUE_REGSIZE
  .cfi_rel_offset PROLOGUE_RBP, 0
#ifdef _LP64
  mov %rsp, %rbp
#else
  // For x86-64 ILP32, it's more compact to use the 32-bit instruction too.
  // The whole 64-bit registers get pushed, though their high halves are zero.
  // But it's fine to clear the high bits of SP, they must be zero to be valid.
  mov %esp, %ebp
#endif
.endm

// Epilogue sequence to match .prologue.fp.  The argument isn't even used for
// metadata here, but it's accepted for parity with .prologue.fp and other
// machines where both need to take the same parameter.
.macro .epilogue.fp frame_extra_size_for_metadata=0
  pop PROLOGUE_RBP
  .cfi_same_value PROLOGUE_RBP
  .cfi_adjust_cfa_offset -PROLOGUE_REGSIZE
.endm

.macro push.spill reg
  push \reg
  .cfi_adjust_cfa_offset PROLOGUE_REGSIZE
  .cfi_rel_offset \reg, 0
.endm

.macro pop.reload reg
  pop \reg
  .cfi_adjust_cfa_offset -PROLOGUE_REGSIZE
  .cfi_same_value \reg
.endm

#endif  // clang-format on

#endif  // ZIRCON_KERNEL_LIB_ARCH_X86_INCLUDE_LIB_ARCH_ASM_H_
