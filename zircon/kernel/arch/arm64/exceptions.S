// Copyright 2016 The Fuchsia Authors
// Copyright (c) 2014 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <arch/arch_thread.h>
#include <arch/arm64.h>
#include <arch/arm64/mp.h>
#include <arch/regs.h>
#include <lib/arch/arm64/exception-asm.h>
#include <lib/arch/arm64/system-asm.h>
#include <lib/arch/asm.h>
#include <lib/syscalls/zx-syscall-numbers.h>
#include <lib/userabi/vdso-arm64.h>
#include <zircon/compiler.h>

#include "smccc.h"


.text

// Spill two registers into the iframe.  If the optional argument is !
// then it's setting up the frame by adjusting the SP by \offset.
.macro iframe.stp reg1, reg2, offset, wb=
  stp \reg1, \reg2, [sp, \offset]\wb
  .cfi.iframe.stp \reg1, \reg2, \offset, \wb
.endm
.macro .cfi.iframe.stp reg1, reg2, offset, wb=
  .if (\offset) % 16
    .error "iframe.stp \offset is not optimally aligned"
  .endif
  .ifc !,\wb
    .cfi_adjust_cfa_offset -(\offset)
  .endif
  // .cfi_rel_offset expresses an offset relative to SP, which is at the base
  // of the iframe.  The assembler will adjust this for offsets relative to the
  // CFA, which is at the end of the iframe.
  .cfi_rel_offset \reg1, \offset + 0
  .cfi_rel_offset \reg2, \offset + 8
.endm

// Reload two registers from the iframe.
.macro iframe.ldp reg1, reg2, offset
  .if (\offset) % 16
    .error "iframe.ldp \offset is not optimally aligned"
  .endif
  ldp \reg1, \reg2, [sp, \offset]
  .ifnc \reg1,xzr
    .cfi_same_value \reg1
  .endif
  .cfi_same_value \reg2
.endm

// Invoke one of those for each x<n> pair, optionally skipping x0, x1 and
// optionally replacing x20 with another register (i.e. xzr), and optionally
// stopping at the (odd number) x<last> rather than all the way up to x29.
.macro iframe.foreach.x op, x0=x0, x20=x20, last=29
  .ifc "x0","\x0"
    \op x0, x1, ARM64_IFRAME_OFFSET_R
  .else
    .ifnc "skip","\x0"
      .error "iframe.foreach.x optional x0= argument must be skip not x0=\x0"
    .endif
  .endif
  .if (\last < 3) || (\last > 29) || (\last % 2 == 0)
    .error "iframe.foreach.x optional last=\last argument must be odd [3,29]"
  .endif
  iframe.x.step 3, \last, \op, x2, x3, ARM64_IFRAME_OFFSET_R + (2 * 8)
  iframe.x.step 5, \last, \op, x4, x5, ARM64_IFRAME_OFFSET_R + (4 * 8)
  iframe.x.step 7, \last, \op, x6, x7, ARM64_IFRAME_OFFSET_R + (6 * 8)
  iframe.x.step 9, \last, \op, x8, x9, ARM64_IFRAME_OFFSET_R + (8 * 8)
  iframe.x.step 11, \last, \op, x10, x11, ARM64_IFRAME_OFFSET_R + (10 * 8)
  iframe.x.step 13, \last, \op, x12, x13, ARM64_IFRAME_OFFSET_R + (12 * 8)
  iframe.x.step 15, \last, \op, x14, x15, ARM64_IFRAME_OFFSET_R + (14 * 8)
  iframe.x.step 17, \last, \op, x16, x17, ARM64_IFRAME_OFFSET_R + (16 * 8)
  iframe.x.step 19, \last, \op, x18, x19, ARM64_IFRAME_OFFSET_R + (18 * 8)
  iframe.x.step 21, \last, \op, \x20, x21, ARM64_IFRAME_OFFSET_R + (20 * 8)
  iframe.x.step 23, \last, \op, x22, x23, ARM64_IFRAME_OFFSET_R + (22 * 8)
  iframe.x.step 25, \last, \op, x24, x25, ARM64_IFRAME_OFFSET_R + (24 * 8)
  iframe.x.step 27, \last, \op, x26, x27, ARM64_IFRAME_OFFSET_R + (26 * 8)
  iframe.x.step 29, \last, \op, x28, x29, ARM64_IFRAME_OFFSET_R + (28 * 8)
.endm

// Helper for iframe.foreach.x, see above.
.macro iframe.x.step n, last, op, reg1, reg2, offset
  .if (\n <= \last)
    \op \reg1, \reg2, \offset
  .endif
.endm

// Push the iframe on the stack and save the x0 and x1 registers.
//
// This expects the .vbar_function.cfi state of CFI when it starts.
// It updates CFI to reflect the SP change and saved register locations.
.macro iframe.exc.start
  // The first push decrements the SP as a side effect. In the case of a stack
  // overflow will result in an immediate fault, instead of 'walking the stack'
  // downwards in an exception loop if the sp were decremented first.
  iframe.stp x0, x1, -ARM64_IFRAME_SIZE, !
.endm

// Save x2..x29, the interrupted PC, and SPSR in the iframe.  The saved PC is
// also left in x10 for easy use later.  Even after this, the sync from EL0
// case relies on x0..x7 having the incoming syscall argument values preserved.
.macro iframe.exc.save.common start_label:req
  iframe.foreach.x iframe.stp, x0=skip

  .if (. - \start_label) != 15 * 4
    .error "iframe.exc.start + iframe.foreach.x should be exactly 15 isns!"
  .endif

  // This nop fills out the sequence to exactly 16 instructions, which fits
  // precisely into one 64-byte cache line.  This allows the alternate vector
  // code to jump in just after here, into the second cache line of the vector.
  //
  // The intent is to optimize for keeping the number of hot cache lines down
  // when both the main vector table and the alternate table are both in use on
  // different CPUs.  This assumes cache lines are actually 64 bytes, not 128
  // bytes; hence each vector spans two cache lines.
  //
  // It's unavoidable that each vector that's in use must occupy one cache line
  // that will tend to be hot.  For the EL1 vectors, all the code fits into two
  // cache lines, so straight-line code in the main table is optimal (except
  // for the delay of this one nop).  In the alternate table(s), each EL1
  // vector can just occupy the unavoidable first cache line and then branch to
  // the second cache line that's shared with the main table (that is, the
  // instruction immediately after this nop), making for a total of 2+n hot
  // cache lines per vector (in aggregate for all CPUs) for n alternate tables
  // rather than 2+2n hot cache lines per vector if each alternate EL1 vector
  // had its own copy of the full straight-line code.
  //
  // This presupposes that a direct branch instruction at the end of one cache
  // line targetting the beginning of a different cache line will have less net
  // overhead than the {n (number of alternate tables) * number of EL1 (or
  // treated-as-EL1) vectors (14?)}-line increase in i-cache pressure.  This
  // has not been measured.  It does seem especially plausible given that
  // current expectations are for alternate vector tables to be used mostly on
  // CPUs that do more deeply-pipelined and out-of-order execution where fixed
  // branches may be particularly cheap in practice.
  //
  // An additional motivation for sharing the tail of each EL1 vector is that
  // the only code that differs between different individual vectors is in that
  // tail portion.  This means that all the alternate table EL1 vectors can be
  // generated by a single generic macro that branches to the corresponding
  // main vector's tail (second cache line).  That makes it a little easier to
  // maintain things in this file without either more error-prone duplication
  // or yet more layers of macroifying each and every thing.
  //
  // The performance tradeoff between this nop and the associated branching vs
  // i-cache pressure should be measured and if significant should be the
  // ultimate deciding factor.  There's nothing prohibitive about changing the
  // code to duplicate the straight-line code in every vector of every
  // alternate table if that refactoring turns out to produce the best code.
  nop

  mrs x10, elr_el1
  .cfi_register DW_REGNO_PC, x10

  // There is no DWARF number for SPSR.
  mrs x11, spsr_el1

  // Save ELR_ELx, i.e. the interrupted PC, and SPSR.
  // There is no DWARF number for SPSR.
  stp x10, x11, [sp, ARM64_IFRAME_OFFSET_ELR]
  .cfi_rel_offset DW_REGNO_PC, ARM64_IFRAME_OFFSET_ELR
.endm

.macro .cfi.iframe.exc.start.el0
  // The previous SP is in SP_EL0 but there is no DWARF number for that.
  .cfi_undefined sp
.endm

.macro .cfi.iframe.exc.start.el1
  // The previous SP is what it was on entry, which is now the CFA.
#ifndef __clang__ // TODO(https://fxbug.dev/323770291): missing .cfi_* support
  .cfi_val_offset sp, 0
#endif
.endm

// Set up the iframe when coming from EL1, and restore kernel invariants.
.macro iframe.exc.save.el1 restore_scsp:req
  .label .L.iframe.exc.save.el1.\@
  iframe.exc.start
  .cfi.iframe.exc.start.el1

  iframe.exc.save.common .L.iframe.exc.save.el1.\@

  // Despite the name, the USP slot is really expected by higher-level code to
  // uniformly mean "the interrupted SP value" regardless of EL0 vs EL1.  In
  // the EL1 case, the interrupted SP is always just where the iframe ends on
  // the stack.  So recover that value and store it in the USP slot.  (The stp
  // instruction can't take SP as one of the value registers, so a temporary
  // would be needed even if we didn't need to add back the adjustment we made
  // to SP in iframe.exc.start above.)  Note that this doesn't change the CFI
  // for SP to locate it in the iframe's USP slot because restoring the iframe
  // won't actually look at the USP slot, it will just pop the iframe.
  add x8, sp, ARM64_IFRAME_SIZE
  stp lr, x8, [sp, ARM64_IFRAME_OFFSET_LR]
  .cfi_rel_offset lr, ARM64_IFRAME_OFFSET_LR
#if ARM64_IFRAME_OFFSET_USP != ARM64_IFRAME_OFFSET_LR + 8
#error "check iframe_t layout"
#endif

  restore_fixed_regs x9, \restore_scsp
.endm

// Set up the iframe when coming from EL0, and restore kernel invariants.
.macro iframe.exc.save.el0
  .label .L.iframe.exc.save.el0.\@
  iframe.exc.start
  .cfi.iframe.exc.start.el0

  // This leaves the interrupted PC in x10 where the syscall path checks it.
  iframe.exc.save.common .L.iframe.exc.save.el0.\@

  // Fetch the interrupted SP from SP_EL0 and save it in the USP slot.
  mrs x9, sp_el0
  .cfi_register sp, x9
  stp lr, x9, [sp, ARM64_IFRAME_OFFSET_LR]
  .cfi_rel_offset lr, ARM64_IFRAME_OFFSET_LR
  .cfi_rel_offset sp, ARM64_IFRAME_OFFSET_USP

  // Put TPIDR_EL1 into x11 while restoring percpu_ptr and shadow_call_sp.
  restore_fixed_regs x11, 1

  // Save the user's mdscr value in the arch_thread struct. Because this
  // value is saved in the struct instead of on the stack, we must ensure
  // that we don't overwrite a previously saved value when we re-enter
  // the kernel. Only save/restore on user/kernel transitions.
  mrs x12, mdscr_el1
  str x12, [x11, CURRENT_MDSCR_OFFSET]
.endm

// Like `.function name`: here to `.end_function name` is the body of a
// function that's jumped to after iframe.exc.save.common sets up the iframe on
// the stack.  Recreate the right CFI conditions.
.macro .iframe.function.common name
  .function \name, local, cfi=custom, nosection=nosection
  .cfi_def_cfa sp, ARM64_IFRAME_SIZE
  iframe.foreach.x .cfi.iframe.stp
  .cfi_rel_offset DW_REGNO_PC, ARM64_IFRAME_OFFSET_ELR
.endm

// Recreate the CFI state at the end of iframe.exc.save.el1.
.macro .iframe.function.el1 name
  .iframe.function.common \name
  .cfi.iframe.exc.start.el1
.endm

// Recreate the CFI state at the end of iframe.exc.save.el0.
.macro .iframe.function.el0 name
  .iframe.function.common \name
  .cfi_rel_offset sp, ARM64_IFRAME_OFFSET_USP
.endm

// This is the common tail of iframe.exc.restore.{el0,el1}.  The PC, SP, and LR
// (x30) are in place already.  This restores x2..x29 (omitting x20 if
// x20=xzr), pops the iframe and does ERET to resume at ELR_EL1 and SPSR_EL1
// (and SP_EL0 if resuming in EL0).
.macro iframe.exc.restore.common x20:req
  // Use x10 and x11 as temporaries for the interrupted PC and SPSR.
  ldp x10, x11, [sp, ARM64_IFRAME_OFFSET_ELR]
  .cfi_register DW_REGNO_PC, x10
  // There is no DWARF number for SPSR.

  // The interrupted PC will be restored from ELR_EL1 by eret.
  msr elr_el1, x10
  .cfi_register DW_REGNO_PC, DW_REGNO_ELR_ELx

  // The interrupted SPSR will be restored from SPSR_EL1 by eret.
  // There is no DWARF number for SPSR.
  msr spsr_el1, x11

  // Reload all the normal registers.
  iframe.foreach.x iframe.ldp, x20=\x20

  // Finally, pop the iframe and ERET.
  .add.sp ARM64_IFRAME_SIZE
  eret

  // Prevent speculation through ERET.
  speculation_postfence
.endm

#if __has_feature(shadow_call_stack)
#define RESTORE_SCSP 1
#else
#define RESTORE_SCSP 0
#endif

// All normal C code in the kernel expects the invariants that the fixed
// registers assigned to the percpu_ptr and the shadow-call-stack pointer have
// the correct values for the current CPU and kernel thread.  When an exception
// happens in the kernel, only percpu_ptr needs to be reloaded.  (In fact, it
// would be disastrous to reload the shadow-call-stack pointer because the
// correct value to reflect the interrupted thread's kernel call stack exists
// only in the register!) But when an exception happens in a lower EL
// (i.e. user mode), these registers must be reloaded from the struct
// arch_thread accessible via TPIDR_EL1 before reaching any C functions.
.macro restore_fixed_regs tp_tmp:req, restore_scsp:req
  mrs \tp_tmp, tpidr_el1
  .if (\restore_scsp & RESTORE_SCSP)
#if CURRENT_SCSP_OFFSET != CURRENT_PERCPU_PTR_OFFSET + 8
# error "shadow_call_sp must follow current_percpu_ptr in struct arch_thread"
#endif
    ldp percpu_ptr, shadow_call_sp, [\tp_tmp, #CURRENT_PERCPU_PTR_OFFSET]
  .else
    ldr percpu_ptr, [\tp_tmp, #CURRENT_PERCPU_PTR_OFFSET]
  .endif
.endm

// Unhandled exception or irq. Save the full state and pass the which value
// through to the inner routine.
.macro iframe.invalid_exception, name, which
  // Save state in the iframe and restore kernel invariants.
  // This resets the shadow_call_sp just in case it was clobbered somehow,
  // or the invalid exception is coming from EL0.
  iframe.exc.save.el1 1

  // Call into the C++ code with the iframe and vector number as arguments.
  mov x0, sp
  mov x1, #\which
  bl arm64_invalid_exception

  // Spin rather than cascade exceptions if that ever returns.
  // Definitely don't restore state from the iframe after this!
  // The C++ code should have done a panic and not returned at all.
  b .
.endm

// Asynchronous exceptions (IRQ, SError).  Call into C++ with the iframe
// and a flag indicating user (EL0) vs kernel (EL1), and then restore from
// the iframe when C++ returns.
.macro iframe.async_exception.el1 name, call
  .vbar_function \name

    // Save state in the iframe and restore kernel invariants.
    // Don't touch shadow_call_sp, keep using the interrupted kernel value.
    iframe.exc.save.el1 0

    // Call into the C++ code with the iframe and flags as arguments.
    mov x0, sp
    mov x1, xzr
    bl \call

    // Use the common return path to restore state and eret.
    b arm64_exc_shared_restore

  .end_vbar_function
.endm

// Asynchronous exceptions from EL0.
.macro iframe.async_exception.el0 name, call
  .vbar_function \name

    // Save state in the iframe and restore kernel invariants.
    iframe.exc.save.el0

    // Call into the C++ code with the iframe and flags as arguments.
    mov x0, sp
    mov x1, #ARM64_EXCEPTION_FLAG_LOWER_EL
    bl \call

    // Use the common return path to restore state and eret.
    b arm64_exc_shared_restore_lower_el

  .end_vbar_function
.endm

// Asynchronous exceptions from arm32 EL0.
.macro iframe.async_exception32.el0 name, call
  .vbar_function \name

    // Save state in the iframe and restore kernel invariants.
    iframe.exc.save.el0

    // If the exception came from an unrestricted task, panic.
    mov x1, #ARM64_DISALLOWED_ARM32_ASYNC_EXCEPTION
    ldr w12, [percpu_ptr, #PERCPU_IN_RESTRICTED_MODE]
    cbz w12, arm64_disallowed_arm32_exception

    // Call into the C++ code with the iframe and flags as arguments.
    mov x0, sp
    mov x1, #ARM64_EXCEPTION_FLAG_LOWER_EL
    bl \call

    // Use the common return path to restore state and eret.
    b arm64_exc_shared_restore_lower_el

  .end_vbar_function
.endm

// TODO: find the appropriate place to reenable FIQs here when they're needed.

// Synchronous exceptions, i.e. everything expected that's not external.
.macro iframe.sync_exception.el1 name
  .vbar_function \name

    // Save state in the iframe and restore kernel invariants.
    // Don't touch shadow_call_sp, keep using the interrupted kernel value.
    iframe.exc.save.el1 0

    // Load x29 with the value of the "exception frame".  This is not a real
    // stack frame.  Instead, we have carefully arranged our iframe structure so
    // that our layout looks like:
    //
    // ```
    // x29  // the frame pointer (at exception time)
    // elr  // the exception link register
    // spsr // saved process stack register
    // x30  // the link register (at exception time)
    // ```
    //
    // instead of following x29 by x30, which might feel more natural.  By doing
    // this, we can now load x29 with the pointer to x29 in our iframe, making
    // it appear (to backtrace code) like we have another frame in our
    // backtrace, but one where the instruction pointer of the frame is the
    // address of the fault itself, instead of being where a `bl` would return
    // to.  See comments in arch/arm64/include/arch/regs.h for more details.
    //
    // TODO(johngro): Currently this code assumes that backtraces will always be
    // computed using frame pointers on the main stack.  Someday, if we want the
    // option to use the SCS as well, we will need to push the value of the ELR
    // onto the SCS and properly adjust (and later restore) the SCS pointer
    // (x18) as well.
    add x29, sp, #(ARM64_IFRAME_OFFSET_ELR - 8)

    // Call into the C++ code with iframe, flags, and ESR as arguments.
    mov x0, sp
    mov x1, xzr
    mrs x2, esr_el1
    bl arm64_sync_exception

    // Use the common return path to restore state and eret.
    b arm64_exc_shared_restore

  .end_vbar_function
.endm

.macro iframe.sync_exception.el0 name
  .vbar_function \name

    // Save state in the iframe and restore kernel invariants.
    iframe.exc.save.el0

    // Collect the Exception Syndrome Register that explains what this is.
    // If this is a syscall, x0-x7 contain args and x16 contains syscall num.
    // x10 contains ELR_EL1.
    // TODO(mcgrathr): make syscall wrappers get args from iframe instead
    mrs x9, esr_el1
    ubfx x11, x9, #ESR_EL1_EC_SHIFT, #ESR_EL1_EC_WIDTH
    cmp x11, #ESR_EC_SVC64          // Check for 64-bit syscall...
    beq arm64_syscall_dispatcher    // ...and jump to syscall handler.

    // We've just run out of space to fit in the 0x80 bytes of the sync
    // exception vector.  Branch to another block of code later in the file
    // that will finish getting ready and call arm64_sync_exception.
    b arm64_sync_exception_el0_nonsyscall

  .end_vbar_function
.endm

// Mirror the arm64 handler but with dedicated arm32 paths.
.macro iframe.sync_exception32.el0 name
  .vbar_function \name
    // Save state in the iframe and restore kernel invariants.
    iframe.exc.save.el0

    // Collect the Exception Syndrome Register that explains what this is.
    mrs x9, esr_el1
    ubfx x11, x9, #ESR_EL1_EC_SHIFT, #ESR_EL1_EC_WIDTH
    cmp x11, #ESR_EC_SVC32          // Check for 32-bit syscall...
    beq arm32_syscall_dispatcher    // ...and jump to syscall handler.

    // We've just run out of space to fit in the 0x80 bytes of the sync
    // exception vector.  Branch to another block of code later in the file
    // that will finish getting ready and call arm64_sync_exception.
    b arm32_sync_exception_el0_nonsyscall

  .end_vbar_function
.endm

// Define the vector table.  See <lib/arch/arm64/exception-asm.h> for a
// more complete explanation.  Inside `.vbar_table` ... `.end_vbar_table`,
// each use of `.vbar_function` uses the exact name that's the name given
// to `.vbar_table` plus the exact suffix that describes the particular
// vector entry point in the terms they appear in the ARM manual under
// "Exception Vectors".  The `.vbar_function` entries must appear in the
// correct order, and an assembly-time error will be diagnosed if any is
// out of order (or too long).  Here it's the `iframe.*_exception` macros
// defined just above that use `.vbar_function`, but the full name of the
// `.vbar_function` entry point they'll define is spelled out in the macro
// invocations below and those symbol names will be visible in disassembly.
//
// iframe.invalid_exception provides the body for entry points with no
// individual `.vbar_function` below.  These are automatically filled in as
// needed when a vector that must be ordered later than an omitted one is
// defined, and at the end of the table.
.vbar_table arm64_el1_exception, global, iframe.invalid_exception

iframe.sync_exception.el1 arm64_el1_exception_sync_current_sp_elx

iframe.async_exception.el1 arm64_el1_exception_irq_current_sp_elx, arm64_irq

iframe.async_exception.el1 arm64_el1_exception_serror_current_sp_elx, \
                           arm64_serror_exception

iframe.sync_exception.el0 arm64_el1_exception_sync_lower_a64

iframe.async_exception.el0 arm64_el1_exception_irq_lower_a64, arm64_irq

iframe.async_exception.el0 arm64_el1_exception_serror_lower_a64, \
                           arm64_serror_exception

iframe.sync_exception32.el0 arm64_el1_exception_sync_lower_a32

iframe.async_exception32.el0 arm64_el1_exception_irq_lower_a32, arm64_irq

iframe.async_exception32.el0 arm64_el1_exception_serror_lower_a32, \
                             arm64_serror_exception

.end_vbar_table


// In the alternate tables, each default vector mirrors the first 15
// instructions of the corresponding main vector and then has a branch into
// that code.  Each main vector has a nop as its 16th instruction, ending
// the first 64-byte cache line.  The alternate vector jumps to just after
// that nop, to share the second cache line of the vector's code.
.macro iframe.alternate.stub name, offset
  .label .L.iframe.alternate.stub.\@
  iframe.exc.start
  .if ((\offset) & ARCH_ARM64_VBAR_CONTEXT_MASK) == ARCH_ARM64_VBAR_LOWER_A64
    .cfi.iframe.exc.start.el0
  .else
    .cfi.iframe.exc.start.el1
  .endif
  iframe.foreach.x iframe.stp, x0=skip

  .if (. - .L.iframe.alternate.stub.\@) != 15 * 4
    .error "iframe.exc.start + iframe.foreach.x should be exactly 15 isns!"
  .endif

  // Now jump into the second half of the corresponding vector.
  b arm64_el1_exception + (\offset) + \
    (.L.iframe.alternate.stub.\@.end - .L.iframe.alternate.stub.\@)
  .label .L.iframe.alternate.stub.\@.end
.endm


// The smccc11_workaround table has non-default vectors for EL0 entry vectors.
.macro iframe.smccc11_workaround.el0 name:req, reload_args:req
  .vbar_function arm64_el1_exception_smccc11_workaround_\name
    .label .L.arm64_el1_exception_smccc11_workaround_\name\().start
    iframe.exc.start
    .cfi.iframe.exc.start.el0
    iframe.foreach.x iframe.stp, x0=skip, last=3

    .label .L.arm64_el1_exception_smccc11_workaround_\name\().workaround
    smccc_workaround_function_w0
    smccc_conduit

    .if \reload_args
      // Reload the registers that SMCCC may have clobbered, x0..x3.
      // These registers are needed by the syscall path.
      // TODO(mcgrathr): make syscall wrappers get args from iframe instead
      iframe.foreach.x iframe.ldp, last=3
    .endif

    // Now pick up in the corresponding main vector code just after the
    // prefix that was repeated above.
    b arm64_el1_exception_\name + \
      (.L.arm64_el1_exception_smccc11_workaround_\name\().workaround - \
       .L.arm64_el1_exception_smccc11_workaround_\name\().start)
  .end_vbar_function
.endm

.vbar_table arm64_el1_exception_smccc11_workaround, global, \
            iframe.alternate.stub
iframe.smccc11_workaround.el0 sync_lower_a64, 1
iframe.smccc11_workaround.el0 irq_lower_a64, 0
iframe.smccc11_workaround.el0 serror_lower_a64, 0
iframe.smccc11_workaround.el0 sync_lower_a32, 1
iframe.smccc11_workaround.el0 irq_lower_a32, 0
iframe.smccc11_workaround.el0 serror_lower_a32, 0
.end_vbar_table


// The smccc10_workaround table has non-default vectors for EL0 entry vectors.
// It differs from the smccc11_workaround table in needing to save registers up
// through x17 before the SMCCC (1.0) call, and reload all argument registers
// (up through x7 rather than only up through x3) afterwards.
.macro iframe.smccc10_workaround.el0 name:req, reload_args:req
  .vbar_function arm64_el1_exception_smccc10_workaround_\name
    .label .L.arm64_el1_exception_smccc10_workaround_\name\().start
    iframe.exc.start
    .cfi.iframe.exc.start.el0
    iframe.foreach.x iframe.stp, x0=skip, last=17

    .label .L.arm64_el1_exception_smccc10_workaround_\name\().workaround
    smccc_workaround_function_w0
    smccc_conduit

    .if \reload_args
      // Reload the argument registers that SMCCC may have clobbered, x0..x7.
      // These registers are needed by the syscall path.
      // TODO(mcgrathr): make syscall wrappers get args from iframe instead
      iframe.foreach.x iframe.ldp, last=7
    .endif

    // Now pick up in the corresponding main vector code just after the
    // prefix that was repeated above.
    b arm64_el1_exception_\name + \
      (.L.arm64_el1_exception_smccc10_workaround_\name\().workaround - \
       .L.arm64_el1_exception_smccc10_workaround_\name\().start)
  .end_vbar_function
.endm

.vbar_table arm64_el1_exception_smccc10_workaround, global, \
            iframe.alternate.stub
iframe.smccc10_workaround.el0 sync_lower_a64, 1
iframe.smccc10_workaround.el0 irq_lower_a64, 0
iframe.smccc10_workaround.el0 serror_lower_a64, 0
iframe.smccc10_workaround.el0 sync_lower_a32, 1
iframe.smccc10_workaround.el0 irq_lower_a32, 0
iframe.smccc10_workaround.el0 serror_lower_a32, 0
.end_vbar_table


// Start the rest of the code on its own cache line.
.balign 64

// This function calls arm64_unrestricted_arm32_exception()
// which expects sp to point to the saved iframe and x1
// to contain the exception identifier.
.iframe.function.el0 arm64_disallowed_arm32_exception
  .label arm64_invalid_arm32_exception
  mov x0, sp
  mrs x2, esr_el1
  bl arm64_unrestricted_arm32_exception
  // Spin rather than cascade exceptions if that ever returns.
  // Definitely don't restore state from the iframe after this!
  // The C++ code should have done a panic and not returned at all.
  b .
.end_function

// This is a wrapper for arm64_sync_exception_el0_nonsyscall
// to ensure that the function is never entered from an arm32
// exception unless the caller is in restricted mode.
//
// Expected state prior to branch:
// - sp - points to the saved iframe
// - x9 - contains the original value of x1
// - x1 - contains the arm32 "exception id"
//
.iframe.function.el0 arm32_sync_exception_el0_nonsyscall
  // If the exception came from an unrestricted task, panic.
  ldr w12, [percpu_ptr, #PERCPU_IN_RESTRICTED_MODE]
  cbnz w12, .Larm32_restricted_nonsyscall

  // Panic if we're not supposed to be here.
  mov x1, #ARM64_DISALLOWED_ARM32_SYNC_EXCEPTION
  bl arm64_disallowed_arm32_exception

.Larm32_restricted_nonsyscall:
  b arm64_sync_exception_el0_nonsyscall
.end_function

// This is the tail of iframe.sync_exception.el0 that doesn't fit into
// the vector.  x9 contains ESR_EL1.
.iframe.function.el0 arm64_sync_exception_el0_nonsyscall

  // Call into C++ with the iframe, flags, and ESR as arguments.
  mov x0, sp
  mov x1, #ARM64_EXCEPTION_FLAG_LOWER_EL
  mov x2, x9
  bl arm64_sync_exception

  // This is the common path for returns to EL0.  The syscall path jumps here.
  //
  // The kernel C++ code is done and ready to return to user mode.  The iframe
  // is still on the stack as set up in iframe.exc.save.el0, modified in place
  // by C++.  Other registers have been clobbered following the normal C
  // calling conventions.
  //
  // Reverse iframe.exc.save.el0 above, restoring state from the iframe.
  .label arm64_exc_shared_restore_lower_el, type=function

  // Fetch the arch_thread pointer, where shadow_call_sp and MDSCR are saved.
  mrs x9, tpidr_el1

  // Load LR (x30) and a temporary (x10) for the user SP.  These don't use
  // iframe.ldp because the CFI for SP shouldn't be .cfi_same_value.
  ldp lr, x10, [sp, #ARM64_IFRAME_OFFSET_LR]
  .cfi_same_value lr

  // Fetch the MDSCR_EL1 value iframe.exc.save.el0 saved in arch_thread.
  ldr x11, [x9, CURRENT_MDSCR_OFFSET]

  // The shadow-call-stack pointer (x18) is saved/restored in struct
  // arch_thread on context switch.  On entry from EL0, it gets reloaded from
  // there via the restore_fixed_regs macro above.  So when returning to EL0,
  // we must make sure to write back the current value (which should always be
  // the base, since returning to EL0 should be the base of the call stack) so
  // that the next kernel entry reloads that instead of whatever was current
  // last time this thread switched out.
#if __has_feature(shadow_call_stack)
  str shadow_call_sp, [x9, #CURRENT_SCSP_OFFSET]
#endif

  // Put the iframe's USP value into SP_EL0, where it will become the SP after
  // ERET resumes in EL0.  Don't update CFI for SP because there's no DWARF
  // number for SP_EL0 so the debugger couldn't find it there.  In this
  // one-instruction window it can be found in x9, but that's about to be
  // clobbered below so it's more useful to report that it's still in the
  // iframe, even though that cannot be modified any more to affect the SP to
  // be restored.  It will invalidate the entire branch predictor via a secure
  // monitor call implemented in firmware.  Mitigation for CVE-2017-5715 and
  // CVE-2022-23960 for Cortex-A73 and Cortex-A75.
  msr sp_el0, x10

  // Restore user MDSCR.
  msr mdscr_el1, x11

  // Restore the remaining registers and resume in EL0.
  iframe.exc.restore.common x20=x20

.end_function


// The kernel C++ code is done and ready to return to interrupted kernel code.
// The iframe is still on the stack as set up in iframe.exc.save, modified in
// place by C++.  Other registers have been clobbered following the normal C
// conventions.
.iframe.function.el1 arm64_exc_shared_restore

  // First, reload LR (x30).  The USP slot is not used.  The interrupted SP
  // is just where SP will be adjusted to when the iframe is popped off.
  ldr lr, [sp, #ARM64_IFRAME_OFFSET_LR]
  .cfi_same_value lr

  // Restoring from EL1 -> EL1, leave x20 alone since it's holding the
  // current per cpu pointer. It may have changed since we originally
  // took the exception if we had been rescheduled to another cpu.
  iframe.exc.restore.common x20=xzr

.end_function


// Emit a small trampoline to branch to the wrapper routine for the syscall.
// Syscall args are in x0-x7 already.  User PC is in x10 and needs to go in the
// next available argument register, or the stack if out of argument registers.
// The trampolines are a tightly-packed sequence for each syscall in order of
// its number.  Each is exactly 16 bytes long (up to 4 instructions).
.macro syscall_dispatcher nargs, syscall
  .balign 16
  .Lsyscall_dispatcher.\syscall\():
  .if \nargs == 8
    stp x10, xzr, [sp, #-16]!  // Store an extra word to keep SP aligned.
    .cfi_adjust_cfa_offset 16
    bl wrapper_\syscall
    .add.sp 16
  .else
    mov x\nargs, x10
    bl wrapper_\syscall
  .endif
  b .Lpost_syscall
  .if (. - .Lsyscall_dispatcher.\syscall\()) > 16
    .error "syscall_dispatcher \nargs, \syscall code too long!"
  .endif
.endm

// Clear unused GPRs to constrain speculative execution with user-controlled
// values.  While this is not a mitigation for any single vulnerability, this
// does make constructing attacks more difficult - speculatively executed
// gadgets will execute with most register state clear.
// TODO(https://fxbug.dev/42076199): Add to synchronous exception path as well.
.macro speculation_clear_gprs
  // x0 - x7 hold syscall arguments from user.
  // x9 holds ESR_EL1, which should be constant describing the SVC exception.
  // x10 holds ELR_EL1 (user PC).
  // x16 holds syscall number from user.
  // x18 holds kernel shadow_call_sp.
  // x20 holds kernel percpu_ptr
  .irp reg,x8,x11,x12,x13,x14,x15,x17,x19,x21,x22,x23,x24,x25,x26,x27,x28,x29,x30
    mov \reg, xzr
  .endr
.endm

// As per speculation_clear_gprs, but adjusted for arm32.
.macro arm32_speculation_clear_gprs
  // x18 holds kernel shadow_call_sp.
  // x20 holds kernel percpu_ptr
  //
  // While x13-15 hold arm32 mapping of SP, LR, and PC respectively, the values
  // have already been saved off.
  .irp reg,x0,x1,x2,x3,x4,x5,x6,x7,x8,x9,x10,x11,x12,x13,x14,x15,x16,x17,x19,x21,x22,x23,x24,x25,x26,x27,x28,x29,x30
    mov \reg, xzr
  .endr
.endm

// Wrapper for restricted mode only arm32 syscalls
//
// Expected state prior to arm32_syscall_dispatcher branch:
//
// percpu and shadow call stack registers have been restored
//
// sp    - points to base of iframe
//
.iframe.function.el0 arm32_syscall_dispatcher

  arm32_speculation_clear_gprs

  // If the exception came from an unrestricted task, panic.
  ldr w12, [percpu_ptr, #PERCPU_IN_RESTRICTED_MODE]
  cbnz w12, .Larm32_restricted_syscall

  mov x1, #ARM64_DISALLOWED_ARM32_SYSCALL
  bl arm64_disallowed_arm32_exception

.Larm32_restricted_syscall:
  // Move the pointer to the iframe into the first argument.
  mov x0, sp
  bl syscall_from_restricted
  // This does not return.
.end_function

//
// Expected state prior to arm64_syscall_dispatcher branch:
//
// percpu and shadow call stack registers have been restored
//
// x0-x7 - contains syscall arguments
// x9    - contains esr_el1 (not used)
// x10   - contains elr_el1
// x16   - contains syscall_num
// sp    - points to base of iframe
//
// Expected state prior to branching to syscall_dispatcher macro:
//
// x0-x7  - contains syscall arguments
// x10    - contains userspace pc
//
.iframe.function.el0 arm64_syscall_dispatcher

  speculation_clear_gprs

  // Check if we're issuing a syscall from restricted mode.
  ldr w12, [percpu_ptr, #PERCPU_IN_RESTRICTED_MODE]
  cbnz w12, .Lrestricted_syscall

  // Verify syscall number and call the unknown handler if bad.
  cmp x16, #ZX_SYS_COUNT
  bhs .Lunknown_syscall

  // Spectre V1: If syscall number >= ZX_SYS_COUNT, replace it with zero. The
  // branch/test above means this can only occur in wrong-path speculative
  // executions.
  csel x16, xzr, x16, hs
  csdb

  // Jump to the right syscall wrapper. The syscall table is an
  // array of 16 byte aligned routines for each syscall. Each routine
  // marshalls some arguments, bls to the routine, and then branches
  // back to .Lpost_syscall (see syscall_dispatcher macro above).
  adr x12, .Lsyscall_table
  add x12, x12, x16, lsl #4
  br x12

  // Prevent speculation through BR.
  speculation_postfence

.Lrestricted_syscall:
  // Move the pointer to the iframe into the first argument.
  mov x0, sp
  bl syscall_from_restricted
  // This does not return.

.Lunknown_syscall:
  mov x0, x16  // Syscall number in the first argument register.
  mov x1, x10  // User PC in the second argument register.
  bl unknown_syscall
  // Fall through.

.Lpost_syscall:
  // Upon return from syscall, x0 = status, x1 = thread signalled
  // Move the status to frame->r[0] for return to userspace.
  str x0, [sp, ARM64_IFRAME_OFFSET_R]

  // Spectre: ARM64 CPUs may speculatively execute instructions after an SVC
  // instruction.  The userspace entry code has a speculation barrier;
  // advance ELR_EL1 past it on the return since it has already done its job.
  .ifne ARM64_SYSCALL_SPECULATION_BARRIER_SIZE - 12
    .error "Syscall speculation barrier must be 12 bytes"
  .endif
  ldr  x10, [sp, ARM64_IFRAME_OFFSET_ELR]
  add  x10, x10, ARM64_SYSCALL_SPECULATION_BARRIER_SIZE
  str  x10, [sp, ARM64_IFRAME_OFFSET_ELR]

  // Check for pending signals. If none, just return.
  cbz x1, arm64_exc_shared_restore_lower_el

  // Call into C++ with the iframe as argument.
  mov x0, sp
  bl arch_iframe_process_pending_signals

  // Now that it's done, actually return to user mode.
  b arm64_exc_shared_restore_lower_el

  // The end of the function is the jump table for calling the wrapper
  // functions.  Each entry is 16 bytes long and naturally aligned so
  // that the arithmetic above uses the right offset from .Lsyscall_table
  // for the syscall number.
  .balign 16
  .Lsyscall_table:

// One of these macros is invoked by kernel.inc for each syscall.

// These don't have kernel entry points.
#define VDSO_SYSCALL(...)

// These are the direct kernel entry points.
#define KERNEL_SYSCALL(name, type, attrs, nargs, arglist, prototype) \
  syscall_dispatcher nargs, name
#define INTERNAL_SYSCALL(...) KERNEL_SYSCALL(__VA_ARGS__)
#define BLOCKING_SYSCALL(...) KERNEL_SYSCALL(__VA_ARGS__)

#include <lib/syscalls/kernel.inc>

#undef VDSO_SYSCALL
#undef KERNEL_SYSCALL
#undef INTERNAL_SYSCALL
#undef BLOCKING_SYSCALL

.end_function
