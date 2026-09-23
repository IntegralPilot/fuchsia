// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/asm.h>
#include <zircon/errors.h>

lr .req x30
fault_return_ptr .req x3
fault_return_mask .req x4
temp .req x9

// NOTE! We know here that the memcpy code doesn't touch these registers,
// so we can use them to save values.  But they are call-clobbered in the
// C ABI, so we don't need a stack frame.
saved_fault_return_ptr .req x16
saved_lr .req x17

// Arm64UserCopyRet _arm64_user_copy(void *dst, const void *src, size_t len, uint64_t *fault_return, uint64_t fault_return_mask)
.section .text._arm64_user_copy,"ax"
.balign 64 // Align to cache line.  This code fits in one cache line.
.function _arm64_user_copy, global

  adr_global temp, .Luser_copy_return
  and temp, temp, fault_return_mask

  mov saved_fault_return_ptr, fault_return_ptr
  .cfi_register fault_return_ptr, saved_fault_return_ptr
  mov saved_lr, lr
  .cfi_register lr, saved_lr

  // Just call our normal memcpy.  The caller has ensured that the
  // address range is in the user portion of the address space.
  // While fault_return_ptr is set, userspace data faults will be
  // redirected to .Luser_copy_return, with the return values already filled out by the kernel fault
  // handler.
  //
  // NOTE! We make important assumptions here about what the memcpy
  // code does: it never moves the stack pointer, and it never touches
  // the registers we're using for saved_fault_return_ptr and saved_lr.
  str temp, [fault_return_ptr]
  bl __unsanitized_memcpy
  // Store a successful status for the return. In this case since we do not set x1 the value of
  // the fault address in the return struct is undefined.
  mov x0, #ZX_OK

.Luser_copy_return:
  str xzr, [saved_fault_return_ptr]
  mov lr, saved_lr
  .cfi_same_value lr
  ret
  speculation_postfence
.end_function
