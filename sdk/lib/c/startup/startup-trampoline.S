// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/asm.h>

#include "../asm-linkage.h"

// See startup-trampoline.h for more details on what this code is doing.

#if defined(__aarch64__)

#define TP_DWARF_REGNO 36 // TPIDR_EL0
#define SCSP_DWARF_REGNO shadow_call_sp

.macro call target
  bl \target
.endm

// Save the 3rd and 4th arguments in call-saved registers.
.macro save_arguments
  mov x19, x2
  .cfi_undefined x19
  mov x20, x3
  .cfi_undefined x20
.endm

.macro prepare_for_jump
  // Until the tail call the original CFA will be found in x16.  This frame's
  // CFI will only ever be used when stepping through startup.  The tail call
  // will clobber x16, but unwinding will never reach here to want its value.
  mov x16, sp
  .cfi_def_cfa_register x16

  // x0 is both first return value register and first argument register.  The
  // argument pointer for start_main is already there.  x1 is the second return
  // value register, where StartCompilerAbi returned the new starting SP.
  //
  // Pretend to be the _start prologue and chain on the empty frame for a zero
  // return address and zero entering FP that _start did on the original stack.
  // In case this is actually the old stack, store zeroes so as not to presume
  // that the _start code did exactly as expected (though now pretending that
  // it did).  (Note this doesn't use .prologue.fp and has no CFI updates
  // because the new stack is still unrelated to the current CFA.)
  stp xzr, xzr, [x1, #-16]
  sub fp, x1, #16
  mov sp, fp

  // Move saved arguments into place.
  mov x1, x19
  mov x2, x20
  // Clear those registers just to keep tidy outermost call-saved state.
  mov x19, xzr
  mov x20, xzr
.endm

.macro tail target
  b \target
.endm

#elif defined(__riscv)

#define TP_DWARF_REGNO tp
#define SCSP_DWARF_REGNO shadow_call_sp

// Save the 3rd and 4th arguments in call-saved registers.
.macro save_arguments
  mv s1, a2
  .cfi_undefined s1
  mv s2, a3
  .cfi_undefined s2
.endm

.macro prepare_for_jump
  // Until the tail call the original CFA will be found in t0.  This frame's
  // CFI will only ever be used when stepping through startup.  The tail call
  // will clobber t0, but unwinding will never reach here to want its value.
  mv t0, sp
  .cfi_def_cfa_register t0

  // a0 is both first return value register and first argument register.  The
  // argument pointer for start_main is already there.  a1 is the second return
  // value register, where StartCompilerAbi returned the new starting SP.
  //
  // Pretend to be the _start prologue and chain on the empty frame for a zero
  // return address and zero entering FP that _start did on the original stack.
  // In case this is actually the old stack, store zeroes so as not to presume
  // that the _start code did exactly as expected (though now pretending that
  // it did).  (Note this doesn't use .prologue.fp and has no CFI updates
  // because the new stack is still unrelated to the current CFA.)
  sd zero, -16(a1)
  sd zero, -8(a1)
  mv fp, a1
  add sp, a1, -16

  // Move saved arguments into place.
  mv a1, s1
  mv a2, s2
  // Clear those registers just to keep tidy outermost call-saved state.
  mv s1, zero
  mv s2, zero
.endm

#elif defined(__x86_64__)

#define TP_DWARF_REGNO %fs.base

// Save the 3rd and 4th arguments in call-saved registers.
.macro save_arguments
  mov %rdx, %r12
  .cfi_undefined %r12
  mov %rcx, %r13
  .cfi_undefined %r13
.endm

.macro prepare_for_jump
  // Install the new stack pointer.  It's already aligned to 16 bytes.  Until
  // the tail call the original CFA will be found in %rcx.  This frame's CFI
  // will only ever be used when stepping through startup.  The tail call will
  // clobber %rcx, but unwinding will never reach here to want its value.
  mov %rsp, %rcx
  mov %rdx, %rsp
  .cfi_def_cfa_register %rcx

  // Move all the arguments into place.
  mov %rax, %rdi
  mov %r12, %rsi
  mov %r13, %rdx

  // After .epilogue.fp (%rsp) points to the original return address,
  // but that %rsp value is now in %rcx instead.
  mov (%rcx), %rax

  // Clear scratch registers just to keep tidy outermost call-saved state.
  xor %r12, %r12
  xor %r12, %r12
  xor %rcx, %rcx

  // First pretend to push the zero return address that _start would have seen
  // on the original stack.  Then pretend to be the _start prologue and chain
  // on the empty frame for a zero return address and zero entering FP.  (Note
  // this doesn't use .prologue.fp and has no CFI updates because the new stack
  // is still unrelated to the current CFA.)  Finally, pretend to be _start's
  // call instruction.  This all simulates exactly what _start is presumed to
  // have done, but on the new stack.
  mov %rcx, -2 * 8(%rsp)
  mov %rcx, -1 * 8(%rsp)
  // At the end of these three instructions, it's as if _start had done these:
  sub $3 * 8, %rsp              // _start: push %rbp
  lea 8(%rsp), %rbp             //         mov %rsp, %rbp
  mov %rax, (%rsp)              //         call __libc_start_main
  xor %rax, %rax
.endm

.macro tail target
  jmp \target
.endm

#endif

.llvm_libc_function __libc_start_main
  .llvm_libc_public __libc_start_main

  .cfi.all_integer .cfi_same_value
  .cfi.all_vectorfp .cfi_same_value
  .prologue.fp

  // This moves the additional arguments (from the dynamic linker) into
  // call-saved registers to be recovered in the final tail call.
  save_arguments

  call LIBC_ASM_LINKAGE(StartCompilerAbi)
  .cfi.all_call_used .cfi_undefined

#ifdef SCSP_DWARF_REGNO
  // The shadow_call_sp register now points to the base of the shadow call
  // stack.  The caller's value is not saved.
  .cfi_undefined SCSP_DWARF_REGNO
#endif

  // The thread pointer register has been reset and the caller's value is gone.
  .cfi_undefined TP_DWARF_REGNO

  .epilogue.fp

  // This moves the SP and argument register into place for the tail call.
  prepare_for_jump

  // Now this frame ceases to exist and is replaced by the C++ function
  // using the full Fuchsia Compiler ABI on its new stacks.
  tail LIBC_ASM_LINKAGE(start_main)

.end_function
