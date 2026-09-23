// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/asm.h>
#include <arch/riscv64.h>
#include <zircon/errors.h>

// Riscv64UserCopyRet _riscv64_user_copy(void *dst, const void *src, size_t len, uint64_t *fault_return, uint64_t capture_faults_mask)
// Align to cache line.  This code fits in one cache line.
.function _riscv64_user_copy, global, align=64
  add.sp -8*6
  .llvm.stack_size 8*6

  .macro on_user_copy_regs macro
    \macro s0, 8*0
    \macro s1, 8*1
    \macro gp, 8*2
    \macro s3, 8*3
    \macro ra, 8*4
  .endm

  on_user_copy_regs sd.spill

  // Allow supervisor accesses to user memory by setting the SUM bit
  li a5, RISCV64_CSR_SSTATUS_SUM
  csrs sstatus, a5

  // Set *fault_return to fault_from_user orred with capture_faults_mask
  // to signal to the exception handler when to run the routine
  lla a5, .Lfault_from_user
  or a5, a5, a4
  sd a5, (a3)

  // Save fault_return into a saved reg
  mv s1, a3

  // Save a copy of the stack pointer
  mv s3, sp

  // Just call our normal memcpy.  The caller has ensured that the
  // address range is in the user portion of the address space.
  // While fault_return_ptr is set, userspace data faults will be
  // redirected to .Lfault_from_user, below.
  //
  // Note makes serious assumptions about the implementation of memcpy.
  // Notably the memcpy implementation will not save or modify anything other
  // than s0, gp and sp as per normal frame pointer and shadow call stack
  // usage. We save these registers above as a result in case the fault path is
  // taken and we're effectively pulled out of the guts of the memcpy so that
  // we can restore them below.  TODO-rvbringup: either implement copy inline
  // or acquire/write a memset/memcpy routine in assembly that can guarantee
  // the above tighter.
  call memcpy

  // Store a successful status for the return.
  // Zero out the top word of the structure to be clean about it.
  li a0, ZX_OK
  li a1, 0

.Luser_copy_return:
  // This code may be reached via .Lfault_from_user, when sp may be different.
  .cfi_def_cfa_register s3
  .cfi_remember_state

  // Restore *fault_return and the ra register
  sd zero, (s1)

  // Disable supervisor accesses to user memory
  li a5, RISCV64_CSR_SSTATUS_SUM
  csrc sstatus, a5

  // Recover the old stack pointer
  mv sp, s3
  .cfi_def_cfa_register sp

  on_user_copy_regs ld.reload

  add.sp 8*6
  ret

// If we are capturing faults the exception handler will have placed the
// faulting virtual address for us in a1 and the flags in a2. We do not touch
// a1 and rely on the caller to know if the value is meaningful based on
// whether it specified fault capture or not, we just need to construct a valid
// a0 before jmping to user_copy_return.
.Lfault_from_user:
  .cfi_restore_state  // Use what was remembered at .Luser_copy_return, above.
  li a0, (ZX_ERR_INVALID_ARGS & 0xffffffff)

  // If we are capturing faults the flags will have been placed in a2 and we
  // want them placed in the high bits of a0. If not capturing faults then we
  // will copy some garbage bits which will be ignored by the caller.
  slli a2, a2, 32
  or a0, a0, a2
  j .Luser_copy_return

.end_function
