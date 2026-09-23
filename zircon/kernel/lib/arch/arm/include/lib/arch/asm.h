// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_ARCH_ARM_INCLUDE_LIB_ARCH_ASM_H_
#define ZIRCON_KERNEL_LIB_ARCH_ARM_INCLUDE_LIB_ARCH_ASM_H_

// Get the generic file.
#include_next <lib/arch/asm.h>

#ifdef __ASSEMBLER__  // clang-format off

// If the compiler uses -mthumb, then tell the assembler to use Thumb too.
#ifdef __thumb__
.thumb
pcrel.bias = 4
#else
pcrel.bias = 8
#endif

.macro assert.fail
  udf #0xfe
.endm

// Read $tp into \reg (as __builtin_thread_pointer() does: TPIDRURO).
.macro read_tp reg, cond=
  mrc\cond p15, #0, \reg, c13, c0, #3 // load_tp_hard in GCC
.endm

.macro .cfi.all_integer op
  .irp reg,r0,r1,r2,r3,r4,r5,r6,r7,r8,r9,r10,r11,r12,r14
    \op \reg
  .endr
.endm

.macro .cfi.all_vectorfp op
  .irp reg,q0,q1,q2,q3,q4,q5,q6,q7,q8,q9,q10,q11,q12,q13,q14,q15,q16,q17,q18,q19,q20,q21,q22,q23,q24,q25,q26,q27,q28,q29,q30,q31
    \op \reg
  .endr
.endm

.macro .cfi.all_call_used op
  .irp reg,r0,r1,r2,r3,r9,r12,q0,q1,q2,q3,q8,q9,q10,q11,q12,q13,q14,q15
    \op \reg
  .endr
.endm

// Add an immediate value to SP, adjusting CFI assuming the CFA rule
// uses an offset from SP.
.macro .add.sp imm
  add sp, sp, #\imm
  .cfi_adjust_cfa_offset -(\imm)
.endm

// Does push, but with CFI updates assuming SP is still the CFA register.
// Arguments are comma-separated registers without braces or `-` syntax.
.macro push.spill regs:vararg
  _.push.spill .L.push.spill.\@.adjust, \regs
.endm

// The FP prologue is mostly rolled into a single push spilling all registers.
// The same adjustment (register count) used for CFI goes the other way to
// recover the CFA: the FP points two words below the CFA.  This emits
// .stack_sizes metadata unconditionally; there's no provision for a frame size
// more than just the FP linkage setup and other spilled registers.
.macro .prologue.fp regs:vararg
  _.push.spill .L.prologue.fp.\@.adjust, \regs, fp, lr
  add fp, sp, #.L.prologue.fp.\@.adjust - 8
  .llvm.stack_size .L.prologue.fp.\@.adjust
.endm

// Does pop, but with CFI updates assuming SP is still the CFA register.
.macro pop.reload regs:vararg
  pop {\regs}
  _.pushpop.adjust _.pop.reload.cfi, .L.pop.reload.\@.adjust, \regs
  .cfi_adjust_cfa_offset -.L.pop.reload.\@.adjust
.endm
.macro _.pop.reload.cfi reg, adjust
  .cfi_same_value \reg
.endm

// The epilogue is entirely accomplished with a single multi-register reload.
.macro .epilogue.fp regs:vararg
  pop.reload \regs, fp, lr
.endm

// This is a subroutine used by the push.spill and .prologue.fp macros.  The
// invoker passes a local "label" name, i.e. a symbol name that starts with
// `.L` and will only be used by this macro invocation, via `\@` uniqueness.
//
// (Local symbols starting with `.L` get discarded by the assembler and/or
// linker so they don't appear among the symbols displayed by debugging tools.)
// Each invoker uses a name specific to that macro invocation via the \@
// substitution.  This is a special assembly macro token substituted just like
// a macro argument, but it's a count of macro expansions such that using it
// once in a given macro means it will always be unique within the whole
// assembly file after macro expansion.
//
// The \adjust temporary symbol ("label") is only used within this macro
// invocation.  The macro iterates through the comma-separated \regs list just
// after emitting `push {\regs}`, accumulating a count in \adjust so that it
// can emit a single .cfi_adjust_cfa_offset instead of many separate ones.  It
// then needs to emit a `.cfi_rel_offset` for each of \regs; these need to be
// after the .cfi_adjust_cfa_offset so that they are relative to the SP as
// adjusted.  The subroutine _.pushpop.adjust (below) does the \adjust-tracking
// iteration over \regs.  That's passed the tiny subroutine _.push.spill.cfi to
// track a derived unique symbol based on the \adjust name, but separate for
// each register so its place in the contiguous sequence of spill locations
// can then be set with .cfi_rel_offset in a final iteration across \regs.
.macro _.push.spill adjust, regs:vararg
  push {\regs}
  .save {\regs}
  // The macro stashes all the .cfi_rel_offset values for each register
  // so they can be applied after the CFA adjustment makes them right.
  _.pushpop.adjust _.push.spill.cfi, \adjust, \regs
  .cfi_adjust_cfa_offset \adjust
  .irp reg, \regs
    .cfi_rel_offset \reg, \adjust\().\reg
  .endr
.endm
.macro _.push.spill.cfi reg, adjust
  \adjust\().\reg = \adjust
.endm

.macro _.pushpop.adjust on_reg, adjust, regs:vararg
  \adjust = 0
  .irp reg,\regs
    .ifnb \reg
      \adjust = \adjust + 4
      \on_reg \reg, \adjust
    .endif
  .endr
.endm

#endif  // clang-format on

#endif  // ZIRCON_KERNEL_LIB_ARCH_ARM_INCLUDE_LIB_ARCH_ASM_H_
