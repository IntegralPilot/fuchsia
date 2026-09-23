// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/asm.h>
#include <lib/arch/riscv64/exception-asm.h>
#include <arch/regs.h>
#include <arch/riscv64.h>

// While running in kernel (S) mode outside the exception handler, sscratch is
// always zero.  Certain invariants are always maintained when in kernel mode:
//  * tp
//    - is what __builtin_thread_pointer() is expected to be in kernel code
//  * percpu_ptr (s11)
//    - is the valid kernel (arch) percpu pointer
//  * sp, shadow_call_sp (gp)
//    - have their valid kernel values and can be pushed on
//  * sscratch
//    - zero; or briefly 1 in the middle of the exception prologue, see below
//
// While running in user (U) mode:
//  * All normal registers (including tp, s11, sp, gp) are owned by the user.
//  * sscratch
//    - holds kernel sp virtual address: KernelStack::top() - sizeof(iframe_t)
//    - The final slots of that iframe_t stash the kernel invariant registers:
//      * .t4 = tp
//      * .t5 = percpu_ptr (s11)
//      * .t6 = shadow_call_sp (gp)
//
// Note that a valid kernel virtual address is always negative (high bit set).
// Thus it's an invariant that sstatus.SPP == (sscratch >= 0).

// This covers most of the registers.  Once these are saved away, they can all
// be used as scratch: all *a; t0..t3; all s*.
.macro iframe.on_scratch_regs op
  \op ra, RISCV64_IFRAME_OFFSET_RA
  \op t0, RISCV64_IFRAME_OFFSET_T0
  \op t1, RISCV64_IFRAME_OFFSET_T1
  \op t2, RISCV64_IFRAME_OFFSET_T2
  \op s0, RISCV64_IFRAME_OFFSET_S0
  \op s1, RISCV64_IFRAME_OFFSET_S1
  \op a0, RISCV64_IFRAME_OFFSET_A0
  \op a1, RISCV64_IFRAME_OFFSET_A1
  \op a2, RISCV64_IFRAME_OFFSET_A2
  \op a3, RISCV64_IFRAME_OFFSET_A3
  \op a4, RISCV64_IFRAME_OFFSET_A4
  \op a5, RISCV64_IFRAME_OFFSET_A5
  \op a6, RISCV64_IFRAME_OFFSET_A6
  \op a7, RISCV64_IFRAME_OFFSET_A7
  \op s2, RISCV64_IFRAME_OFFSET_S2
  \op s3, RISCV64_IFRAME_OFFSET_S3
  \op s4, RISCV64_IFRAME_OFFSET_S4
  \op s5, RISCV64_IFRAME_OFFSET_S5
  \op s6, RISCV64_IFRAME_OFFSET_S6
  \op s7, RISCV64_IFRAME_OFFSET_S7
  \op s8, RISCV64_IFRAME_OFFSET_S8
  \op s9, RISCV64_IFRAME_OFFSET_S9
  \op s10, RISCV64_IFRAME_OFFSET_S10
  \op t3, RISCV64_IFRAME_OFFSET_T3
.endm

// This covers the remaining normal registers t4..t6, each paired with a kernel
// invariant register (tp, percpu_ptr, shadow_call_sp) and its index in [0, 2].
.macro iframe.on_stash_regs op
  \op t4, RISCV64_IFRAME_OFFSET_T4, 0, tp, RISCV64_IFRAME_OFFSET_TP
  \op t5, RISCV64_IFRAME_OFFSET_T5, 1, s11, RISCV64_IFRAME_OFFSET_S11
  \op t6, RISCV64_IFRAME_OFFSET_T6, 2, gp, RISCV64_IFRAME_OFFSET_GP
.endm

.function Riscv64ExceptionEntry, global, cfi=custom, align=4
  .cfi.stvec
  .cfi_remember_state  // Push state to pop at .Luser, below.

  // Before it's determined whether this exception is coming from U or S mode,
  // the CFA is calculated according to what the code is about to do to set sp:
  //  * cfa = sscratch == 0 ? /*kernel*/ sp : /*user*/ sscratch;
  // The CFA expression goes through these DWARF expression stack states:
  //  * <start>        => []
  //  * breg(sp)       => [sp]
  //  * breg(sscratch) => [sscratch, sp]
  //  * dup            => [sscratch, sscratch, sp]
  //  * bra(swap?)     => [sscratch, sp] if sscratch == 0 else [sp, sscratch]
  //  * drop           => [sp] if sscratch == 0 else [sscratch]
  .cfi_escape DW_CFA_def_cfa_expression, 13, \
              DW_OP_breg(2), 0,  \
              DW_OP_bregx, ULEB128_2BYTE(4096 + RISCV64_CSR_SSCRATCH), 0, \
              DW_OP_dup, DW_OP_bra, 1, 0, DW_OP_swap, DW_OP_drop

  // Swap sp and sscratch.
  csrrw sp, sscratch, sp

  // Almost the same DWARF expression, but now for (regs swapped vs above):
  //  * cfa = sp == 0 ? sscratch : sp;
  .cfi_escape DW_CFA_def_cfa_expression, 13, \
              DW_OP_bregx, ULEB128_2BYTE(4096 + RISCV64_CSR_SSCRATCH), 0, \
              DW_OP_breg(2), 0,  \
              DW_OP_dup, DW_OP_bra, 1, 0, DW_OP_swap, DW_OP_drop

  // A proper kernel virtual address always has the high bits set, i.e.
  // appears as a negative integer.  A proper kernel SP is in sscratch
  // while running in user mode.
  bltz sp, .Luser

#ifdef __clang__  // TODO(https://fxbug.dev/42073127)
#define CSR_CFI(...)
#else
#define CSR_CFI(...) __VA_ARGS__
#endif

  // Still in the kernel, so sscratch is now the incoming sp.
  CSR_CFI(.cfi_def_cfa sscratch, 0)
  .cfi_val_offset sp, 0

  // If sscratch was not zero, then something is fishy...
  .cfi_remember_state  // Push state to pop at Riscv64ExceptionStackFault.
  bnez sp, Riscv64ExceptionStackFault

  // Swap back, so sscratch is again zero and the incoming sp is restored.
  csrrw sp, sscratch, zero
  .cfi_def_cfa_register sp

  // Make things fishy!  Set the low bit of sscratch, so it's positive.  It's
  // not zero as it should be in kernel mode.  It's not a valid kernel sp value
  // (which would be a negative integer).  That only happens right here.
  csrs sscratch, 1

  // Probe to trigger a fault for stack overflow before moving sp again.  If
  // this gets back to this path, it should wind up below because sscratch is
  // nonzero but has its high bit clear (i.e. looks positive).
  sd zero, -RISCV64_IFRAME_SIZE(sp)

#if __has_feature(shadow_call_stack)
  // Likewise for the shadow call SP.  The code here won't use it for real, but
  // compiled code will and if it overflows that should be caught as early as
  // possible to take the same emergency panic path as machine stack overflow.
  sd zero, (shadow_call_sp)
#endif

  // Reset to expected invariant state: sscratch is zero in kernel mode.
  csrw sscratch, zero

  // Set up the iframe and spill all the boring registers to it.
  add.sp -RISCV64_IFRAME_SIZE
  iframe.on_scratch_regs sd.spill

  // Now the a* registers are available for scratch use.  Spill the remaining
  // t* registers just like the others.  (Their slots aren't in use as stash
  // slots for the kernel invariant registers.)  Also spill all those kernel
  // invariant registers just to have them in the iframe, though they should
  // not be changing.
  .macro kernel_exc.spill reg, offset, index, kreg, koffset
    sd.spill \reg, \offset
    sd.spill \kreg, \koffset
  .endm
  iframe.on_stash_regs kernel_exc.spill

  // Clear the SUM bit in sstatus early (while reading sstatus) in case the
  // interrupted kernel state had it set.  (Too large for the 5-bit immediate.)
  li a4, RISCV64_CSR_SSTATUS_SUM

  // The remaining iframe slots aren't simple register spills.
  // The SP is the incoming SP, which has already been adjusted down.
  // The PC and STATUS slots just come from CSRs.
  add a0, sp, RISCV64_IFRAME_SIZE
  csrr a1, sepc
  csrrc a2, sstatus, a4  // Clears SUM in sstatus, reads old value into a2.
  csrr a3, scause

  sd a0, RISCV64_IFRAME_OFFSET_SP(sp)
  .cfi_rel_offset sp, RISCV64_IFRAME_OFFSET_SP

  sd a1, RISCV64_IFRAME_OFFSET_PC(sp)
  // 64 is the canonical "alternate return address" register number,
  // used as .cfi_return_column by .cfi.stvec.
  .cfi_rel_offset 64, RISCV64_IFRAME_OFFSET_PC

  sd a2, RISCV64_IFRAME_OFFSET_STATUS(sp)
  CSR_CFI(.cfi_rel_offset sstatus, RISCV64_IFRAME_OFFSET_STATUS)

  mv a0, sp
  call Riscv64KernelException

  // Reload the CSRs that sret will put back into action.
  ld a0, RISCV64_IFRAME_OFFSET_STATUS(sp)
  ld a1, RISCV64_IFRAME_OFFSET_PC(sp)
  csrw sstatus, a0
  csrw sepc, a1

  // Reload all the normal registers from the iframe, but not the kernel
  // invariant registers or the sp itself, which will not have changed.
  // (Except the percpu_ptr may have changed with rescheduling.)
  .macro kernel_exc.reload reg, offset, index, kreg, koffset
    ld.reload \reg, \offset
  .endm
  iframe.on_stash_regs kernel_exc.reload
  iframe.on_scratch_regs ld.reload
  add.sp RISCV64_IFRAME_SIZE  // Same value in RISCV64_IFRAME_OFFSET_SP(sp).

  // Resume at sepc with sstatus in force.
  sret

.label Riscv64ExceptionStackFault, global  // visible in debugger / disasm
  // Here things are just as at entry, but sscratch and sp have been swapped.
  // Since the exception happened with sscratch nonzero in kernel mode (that
  // value is now in sp), it must have been while sscratch was 1 during the
  // stack probe store.
  .cfi_restore_state

  // Save the incoming sp back into sscratch momentarily, so we don't lose it
  // while recovering for panic.  The actual sscratch value here is lost.
  // Only 1 should ever be possible.
  csrw sscratch, sp
  CSR_CFI(.cfi_register sp, sscratch)

  // Reset sp to the emergency panic stack.  There is only one for all CPUs.
  // Its top two words are not used as stack, but instead hold an atomic flag
  // indicating if a CPU has taken the panic path here.
  lla sp, riscv64_emergency_stack_top - 16
  .cfi_def_cfa sp, 0

  // The address of the flag is the only nonzero value we have handy in a
  // register, so that's what's stored as the atomic flag.  If it's already
  // nonzero because a another CPU got here first, just spin in place.  Its
  // value already matches sp, so the atomic swap will succeed with no change.
 amoswap.d sp, sp, (sp)
.label Riscv64EmergencyStackBusy, global
  bnez sp, Riscv64EmergencyStackBusy

  // Recover the sp and save all the registers for the panic exception dump.
  lla sp, riscv64_emergency_stack_top - (16 + RISCV64_IFRAME_SIZE)
  .cfi_def_cfa_offset RISCV64_IFRAME_SIZE

  iframe.on_scratch_regs sd.spill
  iframe.on_stash_regs kernel_exc.spill

  // Recover the (presumably overflowing) sp to record it, and rezero sscratch.
  csrrw a0, sscratch, zero

  csrr a1, sepc
  csrr a2, sstatus
  csrr a3, scause

#if __has_feature(shadow_call_stack)
  // Along with the machine stack, use a special emergency shadow call stack.
  lla shadow_call_sp, riscv64_emergency_shadow_call_stack_base
#endif

  sd a0, RISCV64_IFRAME_OFFSET_SP(sp)
  .cfi_rel_offset sp, RISCV64_IFRAME_OFFSET_SP
  sd a1, RISCV64_IFRAME_OFFSET_PC(sp)
  .cfi_rel_offset 64, RISCV64_IFRAME_OFFSET_PC
  sd a2, RISCV64_IFRAME_OFFSET_STATUS(sp)
  CSR_CFI(.cfi_rel_offset sstatus, RISCV64_IFRAME_OFFSET_STATUS)

  // Call the panic path to dump the exception details.  It must not return.
  mv a0, sp
  call Riscv64EmergencyException
  unimp

.Luser:
  // Here things are just as at entry, but sscratch and sp have been swapped.
  // The kernel sp (stashed in sscratch while in user mode) already points at
  // the base of the iframe, so the CFA is above the iframe.
  .cfi_restore_state
  .cfi_def_cfa sp, RISCV64_IFRAME_SIZE

  // Spill all the user values for the kernel invariant registers first.
  .macro user_exc.kspill reg, offset, index, kreg, koffset
    sd.spill \kreg, \koffset
  .endm
  iframe.on_stash_regs user_exc.kspill

  // Now restore the stashed kernel invariant registers.
  .macro user_exc.restore reg, offset, index, kreg, koffset
    ld \kreg, (RISCV64_IFRAME_OFFSET_T4 + (8*\index))(sp)
  .endm
  iframe.on_stash_regs user_exc.restore

  // Spill all the boring registers, freeing them up for scratch use.
  iframe.on_scratch_regs sd.spill

  // Recover the incoming user SP from sscratch to store in the iframe, and
  // reset sscratch to zero as the invariant for normal kernel mode execution.
  csrrw a0, sscratch, zero

  // Fetch the other special values and store them in the iframe.
  // The scause value doesn't go in the iframe, but is handy as an argument.
  csrr a1, sepc
  csrr a2, sstatus
  csrr a3, scause
  sd a0, RISCV64_IFRAME_OFFSET_SP(sp)
  .cfi_rel_offset sp, RISCV64_IFRAME_OFFSET_SP
  sd a1, RISCV64_IFRAME_OFFSET_PC(sp)
  .cfi_rel_offset 64, RISCV64_IFRAME_OFFSET_PC
  sd a2, RISCV64_IFRAME_OFFSET_STATUS(sp)
  CSR_CFI(.cfi_rel_offset sstatus, RISCV64_IFRAME_OFFSET_STATUS)

  // Finally, spill the remaining user registers, whose slots double as the
  // stash slots for kernel invariant registers.
  .macro user_exc.tspill reg, offset, index, kreg, koffset
    sd.spill \reg, \offset
  .endm
  iframe.on_stash_regs user_exc.tspill

  // Now call into higher-level code with the iframe_t* as argument.
  mv a0, sp
  call Riscv64UserException

  // arch_uspace_entry jumps here with sp and shadow_call_sp set appropriately.
  // Everything else is the same as if the iframe had been filled in this path.
.label Riscv64ReturnToUser, global

  // First reload all the user registers whose slots double as the stash slots
  // for kernel invariant registers.
  .macro user_exc.treload reg, offset, index, kreg, koffset
    ld.reload \reg, \offset
  .endm
  iframe.on_stash_regs user_exc.treload

  // Stash the kernel invariant registers in those slots.
  .macro user_exc.stash reg, offset, index, kreg, koffset
    sd \kreg, (RISCV64_IFRAME_OFFSET_T4 + (8*\index))(sp)
  .endm
  iframe.on_stash_regs user_exc.stash

  // Fetch the user values that sret will restore.
  ld a0, RISCV64_IFRAME_OFFSET_STATUS(sp)
  ld a1, RISCV64_IFRAME_OFFSET_PC(sp)

  // Save the kernel sp in sscratch as expected while in user mode.
  csrw sscratch, sp

  // Reload the user values for the kernel invariant registers.
  .macro user_exc.kreload reg, offset, index, kreg, koffset
    ld.reload \kreg, \koffset
  .endm
  iframe.on_stash_regs user_exc.kreload

  // Reload the CSRs for set while scratch registers are still usable.
  csrw sstatus, a0
  csrw sepc, a1

  // Reload all the remaining user registers, and finally the user SP.
  iframe.on_scratch_regs ld.reload
  ld.reload sp, RISCV64_IFRAME_OFFSET_SP

  // Resume at sepc with sstatus in force.
  sret
.end_function
