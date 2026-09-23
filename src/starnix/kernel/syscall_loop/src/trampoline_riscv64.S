// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <starnix_asm_utils/asm.h>

FUNCTION(breakpoint_for_module_changes)
  ebreak
  ret
END_FUNCTION(breakpoint_for_module_changes)


//      fn restricted_enter_loop(
//  a0    options: u32,
//  a1    restricted_exit_callback: extern "C" fn(*mut RestrictedEnterContext<'_>, u64) -> bool,
//  a2    restricted_exit_callback_context: *mut RestrictedEnterContext<'_>,
//  a3    restricted_state: *mut zx::sys::zx_restricted_exception_t,
//  a4    extended_pstate_ptr: *mut ExtendedPstatePointer,
//  a0 ) -> zx::sys::zx_status_t;
FUNCTION(restricted_enter_loop)
  // Make space for all 12 callee saved registers, return address, thread pointer, shadow call
  // stack pointer, and 5 parameters on the stack. Since the stack must remain aligned to 16
  // byte boundaries an additional 8 bytes are allocated and not used.
  addi sp, sp, -(20 * 8)

  // Save all of the callee saved registers
  sd  s0,  0*8(sp)
  sd  s1,  1*8(sp)
  sd  s2,  2*8(sp)
  sd  s3,  3*8(sp)
  sd  s4,  4*8(sp)
  sd  s5,  5*8(sp)
  sd  s6,  6*8(sp)
  sd  s7,  7*8(sp)
  sd  s8,  8*8(sp)
  sd  s9,  9*8(sp)
  sd s10, 10*8(sp)
  sd s11, 11*8(sp)

  // Save the return address
  sd  ra, 12*8(sp)

  // Save the thread pointer
  sd  tp, 13*8(sp)

  // Save the shadow call stack pointer
  sd  gp, 14*8(sp)

  // Save original options
  sd  a0, 15*8(sp)

  // Save restricted exit callback
  sd  a1, 16*8(sp)

  // Save restricted exit callback context
  sd  a2, 17*8(sp)

  // Save address of restricted state mappings
  sd  a3, 18*8(sp)

  // Save address of extended processor state mapping
  sd  a4, 19*8(sp)

  // Restore extended processor state
  mv a0, a4
  call restore_extended_pstate

.restricted_enter_loop_top:
  // Restore zx_restricted_enter parameters from the stack
  ld  a0, 15*8(sp)

  // Store address of restricted_return_loop
  la a1, restricted_return_loop

  // Pass the stack pointer as the context argument to the syscall.
  mv a2, sp

//      fn zx_restricted_enter(
//  a0     uint32_t options,
//  a1     uintptr_t vector_table_ptr,
//  a2     uintptr_t context,
//  a0   ) -> zx_status_t
  call zx_restricted_enter

  // If zx_restricted_enter returns to here then we did not enter restricted mode. Unwind the
  // stack and return the error in x0 to the caller.

.restricted_enter_loop_ret:
  // Restore all of the callee saved registers.
  ld  s0,  0*8(sp)
  ld  s1,  1*8(sp)
  ld  s2,  2*8(sp)
  ld  s3,  3*8(sp)
  ld  s4,  4*8(sp)
  ld  s5,  5*8(sp)
  ld  s6,  6*8(sp)
  ld  s7,  7*8(sp)
  ld  s8,  8*8(sp)
  ld  s9,  9*8(sp)
  ld s10, 10*8(sp)
  ld s11, 11*8(sp)

  // Restore the return address
  ld  ra, 12*8(sp)

  // Reset the stack.
  addi sp, sp, (20 * 8)
  ret
END_FUNCTION(restricted_enter_loop)

// The restricted return entry point is not really a function but we treat it like one. It has
// the following parameters:
// fn restricted_return_loop(
//    a0   context: usize,
//    a1   reason_code: u64
// )
FUNCTION(restricted_return_loop)
  // a0 holds the context, which is the stack pointer
  mv sp, a0

  // Save the reason code in a callee-saved register
  mv s1, a1

  // Restore thread pointer
  ld tp, 13*8(sp)

  // Restore shadow call stack
  ld gp, 14*8(sp)

  // Save extended processor state
  ld a0, 19*8(sp)
  call save_extended_pstate

  // TODO: emit CFI directives

  // Invoke callback with context and reason_code:
  //       fn restricted_exit_callback_c(
  // a0      context: usize,
  // a1      reason_code: zx::sys::zx_restricted_reason_t,
  // a2      extended_pstate_ptr: *mut ExtendedPstatePointer,
  // a0    ) -> bool

  // Load callback function and context pointers
  ld t0, 16*8(sp)
  ld a0, 17*8(sp)
  mv a1, s1
  ld a2, 19*8(sp)
  jalr t0

  // TODO: restore CFI directives

  // Did the callback tell us to exit?
  beqz a0, .restricted_enter_loop_ret

  // Restore extended processor state
  ld a0, 19*8(sp)
  call restore_extended_pstate

  // Go back to the top
  j .restricted_enter_loop_top
  // Never reached
END_FUNCTION(restricted_return_loop)
