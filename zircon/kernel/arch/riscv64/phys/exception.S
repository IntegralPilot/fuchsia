// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/asm.h>
#include <lib/arch/riscv64/exception-asm.h>
#include <phys/exception.h>
#include <phys/stack.h>
#include <zircon/tls.h>

#include "riscv64.h"

.function ArchPhysExceptionEntry, global, cfi=custom, align=4
  .cfi.stvec

  // On entry the CFI state thinks the CFA is the SP.  But the SP is the
  // interrupted SP.  Instead we'll move to a dedicated stack used only for
  // exceptions.  (There's just one, so nested any exceptions will just
  // start over reusing it from the top.)  So the SP we're about to switch
  // to is what we'd really like to call the CFA.  That value is not in any
  // register.
#define EXC_ENTRY_CFA (phys_exception_stack + BOOT_STACK_SIZE - PHYS_EXCEPTION_STATE_SIZE)
#if BOOT_STACK_SIZE % BOOT_STACK_ALIGN != 0
#error "BOOT_STACK_SIZE not aligned"
#endif

  // TODO(mcgrathr): DW_CFA_def_cfa_expression with DW_OP_addr could just point
  // to its fixed address.  But the assembler doesn't know how to generate that
  // and .cfi_escape only takes byte values and can't generate a relocation for
  // the address.  The GNU assembler has a .cfi_val_encoded_addr directive that
  // generates a DW_OP_addr expression just like that for you, but that is only
  // for DW_CFA_val_expression (a register rule, not the CFA) and LLVM doesn't
  // implement that anyway.  So the CFA is inaccurate at the first instruction.
//.cfi_def_cfa_encoded_addr EXC_ENTRY_CFA

  // We don't keep anything in sscratch, so we can clobber it freely.
  // This is the only way to free up a register to start saving things.
  csrw sscratch, sp

  // Now we can reset the SP to the reserved exception stack, just below where
  // the registers will be saved.  This is the SP value that's the CFA.
  lla sp, EXC_ENTRY_CFA
  .cfi_def_cfa_register sp

  // Save a register and update CFI to say where it is.
  .macro sd_cfa n
    sd x\n, (\n * 8)(sp)
    .cfi_offset x\n, \n * 8
  .endm

  // x0 is the always-zero register, so slot 0 is used for the PC.
  sd_cfa 1
  // x2 is the SP, which is actually stashed in sscratch, so skip it for now.
  sd_cfa 3
  sd_cfa 4
  sd_cfa 5
  sd_cfa 6
  sd_cfa 7
  sd_cfa 8
  sd_cfa 9
  sd_cfa 10
  sd_cfa 11
  sd_cfa 12
  sd_cfa 13
  sd_cfa 14
  sd_cfa 15
  sd_cfa 16
  sd_cfa 17
  sd_cfa 18
  sd_cfa 19
  sd_cfa 20
  sd_cfa 21
  sd_cfa 22
  sd_cfa 23
  sd_cfa 24
  sd_cfa 25
  sd_cfa 26
  sd_cfa 27
  sd_cfa 28
  sd_cfa 29
  sd_cfa 30
  sd_cfa 31

  // Now that all the normal registers have been saved, we can use
  // some for scratch.

  // Fetch the interrupted PC from the CSR.
  csrr a1, sepc

  // Recover the interrupted SP we stashed, and clear sscratch to be tidy.
  csrrw a0, sscratch, zero

  // Store the PC in slot 0, and tell CFI it's there.
  sd a1, (sp)
  .cfi_offset 64, 0

  // Store the SP in slot 2, and tell CFI it's there.
  sd a0, (2 * 8)(sp)
  .cfi_offset sp, 2 * 8

#if __has_feature(shadow_call_Stack)
  // Reset the shadow call stack to the one reserved for phys exceptions.
  lla shadow_call_sp, phys_exception_shadow_call_stack  // Grows up.
#endif

  // Reset the thread pointer, though nothing should have changed it.
  lla tp, boot_thread_pointer

  // We now have complete CFI representing all the general registers so a
  // debugger can unwind through this frame and back to the interrupted code.
  // A normal call into C++ code will permit unwinding back into this frame.
  // All the ABI stacks have been set up safely so normal C++ code can run.

  // Load the argument for the C++ entry point.
  mv a0, sp

  // Make a first frame pointer record on the exception stack that links to
  // the interrupted state's FP and PC.  This differs from .prologue.fp 0, a1
  // only in that CFI rules for fp and a1 are unchanged.
  add sp, sp, -16
  .cfi_adjust_cfa_offset 16
  sd fp, (sp)
  sd a1, 8(sp)
  add fp, sp, 16

  // Show the interrupted PC as the earliest caller on the shadow call stack.
  // A backtrace via the exception shadow call stack won't go past there like
  // the frame pointer might, but it will at least clearly bottom out at the
  // exception PC.
#if __has_feature(shadow_call_stack)
  sd a1, (shadow_call_sp)
  add shadow_call_sp, shadow_call_sp, 8
#endif

  call ArchPhysException

  // If it returns the magic number, we can resume.
  li a1, PHYS_EXCEPTION_RESUME
  beq a0, a1, .Lresume_from_exception

 // Otherwise it really shouldn't have returned.  Trigger an exception reentry.
 // What else?
.label PhysExceptionHandler_returned_BUG, global
  unimp
  j PhysExceptionHandler_returned_BUG

.Lresume_from_exception:
  // Pop the frame pointer link pair pushed above.
  add sp, sp, 16
  .cfi_adjust_cfa_offset -16

  // The handler code should have restored the SP it got on entry.
  // Since the FP points above its link pair, they should now match again.
  bne sp, fp, PhysExceptionHandler_returned_BUG

  // Reload the interrupted PC (as modified) into sepc.
  ld a0, (sp)
  csrw sepc, a0
#ifndef __clang__  // TODO(https://fxbug.dev/42073127)
  .cfi_register 64, sepc
#endif

  .macro ld_cfa n
    ld x\n, (\n * 8)(sp)
    .cfi_same_value x\n
  .endm

  // x0 is the always-zero register, so slot 0 is used for the PC.
  ld_cfa 1
  // x2 is the SP, which must be done last.
  ld_cfa 3
  ld_cfa 4
  ld_cfa 5
  ld_cfa 6
  ld_cfa 7
  ld_cfa 8
  ld_cfa 9
  ld_cfa 10
  ld_cfa 11
  ld_cfa 12
  ld_cfa 13
  ld_cfa 14
  ld_cfa 15
  ld_cfa 16
  ld_cfa 17
  ld_cfa 18
  ld_cfa 19
  ld_cfa 20
  ld_cfa 21
  ld_cfa 22
  ld_cfa 23
  ld_cfa 24
  ld_cfa 25
  ld_cfa 26
  ld_cfa 27
  ld_cfa 28
  ld_cfa 29
  ld_cfa 30
  ld_cfa 31

  // Finally, we can load the SP from itself, and all the registers are back.
  ld_cfa 2

  // Then we simply resume at sepc, changing modes according to sstatus.
  sret
.end_function
