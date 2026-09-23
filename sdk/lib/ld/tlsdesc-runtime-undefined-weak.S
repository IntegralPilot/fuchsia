// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/asm.h>
#include <lib/ld/tlsdesc.h>

#ifndef TLSDESC_RETAIN
#error "build system must define TLSDESC_RETAIN"
#endif

#if defined(__aarch64__)

// Return -$tp so that adding $tp yields zero, i.e. nullptr.
.tlsdesc.function _ld_tlsdesc_runtime_undefined_weak, retain=TLSDESC_RETAIN, align=8
  .tlsdesc.lsda kUndefinedWeak

  mrs x0, TPIDR_EL0
  neg x0, x0

  ret

.end_function

// Return addend-$tp so that adding $tp looks like nullptr+addend.
.tlsdesc.function _ld_tlsdesc_runtime_undefined_weak_addend, retain=TLSDESC_RETAIN
  .tlsdesc.lsda kUndefinedWeakAddend

  // Only x0 and x30 can be clobbered, so another register must be saved.
  stp x1, xzr, [sp, #-16]!
  .cfi_adjust_cfa_offset 16
  .cfi_offset x1, -16

  // On entry x0 contains the argument: the address of the GOT slot pair.
  // Fetch the addend.
  ldr tlsdesc_r0, [x0, #tlsdesc.value_offset]

  // On exit x0 contains the return value: offset from $tp (TPIDR_EL0).
  .cfi_undefined x0

  // Fetch the thread pointer.
  mrs x1, TPIDR_EL0

  // Subtract the thread pointer from the addend.
  sub tlsdesc_r0, tlsdesc_r0, tlsdesc_r1

  ldr x1, [sp], #16
  .cfi_same_value x1
  .cfi_adjust_cfa_offset -16
  ret

.end_function

#elif defined(__arm__)

// Return -$tp so that adding $tp yields zero, i.e. nullptr.
.tlsdesc.function _ld_tlsdesc_runtime_undefined_weak, retain=TLSDESC_RETAIN, align=4
  .tlsdesc.lsda kUndefinedWeak

  // On exit r0 contains the return value: offset from $tp (TPIDR_EL0).
  .cfi_undefined r0

  read_tp r0
  neg r0, r0

  bx lr

.end_function

// Return addend-$tp so that adding $tp looks like nullptr+addend.
.tlsdesc.function _ld_tlsdesc_runtime_undefined_weak_addend, retain=TLSDESC_RETAIN
  .tlsdesc.lsda kUndefinedWeakAddend

  // Only r0 and lr can be clobbered, so another register must be saved.
  push.spill r1

  // On entry r0 contains the argument: the address of the GOT slot pair.
  // Fetch the addend.
  ldr r0, [r0, #4]

  // On exit r0 contains the return value: offset from $tp (TPIDR_EL0).
  .cfi_undefined r0

  // Fetch the thread pointer.
  read_tp r0

  // Subtract the thread pointer from the addend.
  sub r0, r0, r1

  pop.reload r1
  bx lr

.end_function

#elif defined(__riscv)

// Return -$tp so that adding $tp yields zero, i.e. nullptr.
.tlsdesc.function _ld_tlsdesc_runtime_undefined_weak, retain=TLSDESC_RETAIN, align=4
  .tlsdesc.lsda kUndefinedWeak

  tlsdesc.sub a0, zero, tp

  // On exit a0 contains the return value: offset from tp.
  .cfi_undefined a0

  // The caller's return address is in t0, with ra preserved.
  jr t0

.end_function

// Return addend-$tp so that adding $tp looks like nullptr+addend.
.tlsdesc.function _ld_tlsdesc_runtime_undefined_weak_addend, retain=TLSDESC_RETAIN
  .tlsdesc.lsda kUndefinedWeakAddend

  // On entry a0 contains the argument: the address of the GOT slot range.
  tlsdesc.load a0, tlsdesc.value_offset(a0)

  // On exit a0 contains the return value: offset from tp.
  .cfi_undefined a0

  tlsdesc.sub a0, a0, tp

  // The caller's return address is in t0, with ra preserved.
  jr t0

.end_function

#elif defined(__x86_64__)

// Return -$tp so that adding $tp yields zero, i.e. nullptr.
.tlsdesc.function _ld_tlsdesc_runtime_undefined_weak, retain=TLSDESC_RETAIN
  .tlsdesc.lsda kUndefinedWeak

  // On entry %rax contains the argument: the address of the GOT slot pair.
  // On exit %rax contains the return value: offset from $tp (%fs.base).
  xor %eax, %eax
  .cfi_undefined %rax
  sub %fs:0, %tlsdesc_ax

  ret

.end_function

// Return addend-$tp so that adding $tp looks like nullptr+addend.
.tlsdesc.function _ld_tlsdesc_runtime_undefined_weak_addend, retain=TLSDESC_RETAIN
  .tlsdesc.lsda kUndefinedWeakAddend

  // On entry %rax contains the argument: the address of the GOT slot pair.
  // On exit %rax contains the return value: offset from $tp (%fs.base).
  //
  // Note that on x86-64 ILP32, GOT entries are still 8 bytes, to facilitate
  // use of the indirect addressing modes.
  mov 8(%rax), %tlsdesc_ax
  .cfi_undefined %rax
  sub %fs:0, %tlsdesc_ax

  ret

.end_function

#else

// Not all machines have TLSDESC support specified in the psABI.

#endif
