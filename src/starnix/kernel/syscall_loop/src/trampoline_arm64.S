// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <starnix_asm_utils/asm.h>

FUNCTION(breakpoint_for_module_changes)
  brk #0
  ret
END_FUNCTION(breakpoint_for_module_changes)

//       fn restricted_enter_loop(
//  x0    options: u32,
//  x1    restricted_exit_callback: extern "C" fn(*mut RestrictedEnterContext<'_>, u64) -> bool,
//  x2    restricted_exit_callback_context: *mut RestrictedEnterContext<'_>,
//  x3    restricted_state: *mut zx::sys::zx_restricted_exception_t,
//  x4    extended_pstate_ptr: *mut ExtendedPstatePointer,
//  x0 ) -> zx::sys::zx_status_t;
FUNCTION(restricted_enter_loop)
  // This function:
  //   - Saves the callee saved registers (including the link register) on the stack.
  //   - Passes the stack pointer as the third ("context") parameter (in r2) to zx_restricted_enter().
  //   - Calls zx_restricted_enter().
  //   - Returns any failures (this will not return at all on success).

  // Save the callee saved registers (including the link register).
  stp x20, x19, [sp, #-16]!
  stp x22, x21, [sp, #-16]!
  stp x24, x23, [sp, #-16]!
  stp x26, x25, [sp, #-16]!
  stp x28, x27, [sp, #-16]!
  stp x30, x29, [sp, #-16]!

  // Save shadow call stack pointer and original options
  stp x0, x18, [sp, #-16]!   // Will be at [sp], #32

  // Save restricted exit callback function and context
  stp x1,  x2, [sp, #-16]!   // Will be at [sp], #16

  // Save address of restricted state and extended processor state mappings
  stp x3,  x4, [sp, #-16]!   // Will be at [sp], #0

  // Restore extended processor state
  mov x0, x4
  bl restore_extended_pstate

.restricted_enter_loop_top:
  // Restore zx_restricted_enter options from stack
  ldr x0, [sp, #32]

  // Load address of restricted exit vector callback.
  // The restricted state CPSR register's bit 0x10 selects between aarch32 and aarch64 mode
  // If it's asserted, we are entering 32 bit mode and want to use our aarch32 exit callback.
  adr  x8, restricted_return_loop
  adr  x9, restricted_return_loop_aarch32
  // Restore restricted state mapping
  ldr  x10, [sp, #0]
  // Load CPSR word - offset is after 32 registers, PC, and TIPDR_EL0
  ldrb w11, [x10, #34 * 8]
  tst  w11, #0x10
  csel x1, x8, x9, eq

  // Save stack pointer as context parameter for syscall
  mov x2, sp

//      fn zx_restricted_enter(
//  x0     uint32_t options,
//  x1     uintptr_t vector_table_ptr,
//  x2     uintptr_t context,
//  x0   ) -> zx_status_t
  bl zx_restricted_enter

  // If zx_restricted_enter returns to here then we did not enter restricted mode. Unwind the
  // stack and return the error in x0 to the caller.

.restricted_enter_loop_ret:
  // Pop temporaries
  add sp, sp, #(6 * 8)

  // Restore callee saved registers
  ldp x30, x29, [sp], #16
  ldp x28, x27, [sp], #16
  ldp x26, x25, [sp], #16
  ldp x24, x23, [sp], #16
  ldp x22, x21, [sp], #16
  ldp x20, x19, [sp], #16

  ret
END_FUNCTION(restricted_enter_loop)

// The restricted return entry point is not really a function but we treat it like one. It has the following
// parameters:
// fn restricted_return_loop(
//    x0   context: usize,
//    x1   reason_code: u64
// )
FUNCTION(restricted_return_loop)
  // The "caller" is actually interrupted restricted mode state.  This means the
  // "return address" will be treated as the precise PC of the "caller", rather
  // than as its return address that's one instruction after the call site.
  .cfi_signal_frame

  // x0 holds the context, which is the stack pointer.
  mov sp, x0

  // Save the reason code in a callee-saved register
  mov x19, x1

  // Restore shadow call stack
  ldr x18, [sp, #40]

  // Save extended processor state
  ldr x0, [sp, #8]
  bl save_extended_pstate

  // Load the address of the mapped restricted mode register state to x27
  ldr x27, [sp]

  // Load frame pointer from restricted state to connect Starnix stack to Linux's.
  // This offset matches the offset of x29 in the `zx_restricted_state_t` struct.
  ldr x29, [x27, 0xE8]

  // Emit CFI directives referring to the restricted mode register state
  .cfi_remember_state
  .cfi_def_cfa x27, 0
  .cfi_offset x0, 0
  .cfi_offset x1, 0x08
  .cfi_offset x2, 0x10
  .cfi_offset x3, 0x18
  .cfi_offset x4, 0x20
  .cfi_offset x5, 0x28
  .cfi_offset x6, 0x30
  .cfi_offset x7, 0x38
  .cfi_offset x8, 0x40
  .cfi_offset x9, 0x48
  .cfi_offset x10, 0x50
  .cfi_offset x11, 0x58
  .cfi_offset x12, 0x60
  .cfi_offset x13, 0x68
  .cfi_offset x14, 0x70
  .cfi_offset x15, 0x78
  .cfi_offset x16, 0x80
  .cfi_offset x17, 0x88
  .cfi_offset x18, 0x90
  .cfi_offset x19, 0x98
  .cfi_offset x20, 0xA0
  .cfi_offset x21, 0xA8
  .cfi_offset x22, 0xB0
  .cfi_offset x23, 0xB8
  .cfi_offset x24, 0xC0
  .cfi_offset x25, 0xC8
  .cfi_offset x26, 0xD0
  .cfi_offset x27, 0xD8
  .cfi_offset x28, 0xE0
  .cfi_offset x29, 0xE8
  .cfi_offset lr, 0xF0   // x30
  .cfi_offset sp, 0xF8   // x31

  // We cannot refer directly to the PC register as per the AArch64 manual:
  // https://developer.arm.com/documentation/dui0801/l/Overview-of-AArch64-state/Predeclared-core-register-names-in-AArch64-state?lang=en,
  // attempting to do so will silently not produce a CFI directive to recover that register.
  // Instead, we are handcrafting our own DW_CFA_offset_extended_sf CFI expression, pointing at
  // register "32" (which is not specified in the DWARF standard, but the unwinder understands to be
  // "PC") and a value corresponding to the next stack slot at offset 0x100 in SLEB128 format per
  // the format specification for DW_CFA_offset_extended_sf. This way, the unwinder can recover both
  // PC and LR, which enables finding the rest of the restricted mode stack frames.
  .cfi_escape 0x11, 32, 0x40

  // Invoke callback with context and reason_code:
  //       fn restricted_exit_callback_c(
  // x0      context: usize,
  // x1      reason_code: zx::sys::zx_restricted_reason_t,
  // x2      extended_pstate_ptr: *mut ExtendedPstatePointer,
  // x0    ) -> bool

  // Load callback context and function pointers stack
  ldp x3, x0, [sp, #16]

  // Restore reason code that we saved in x19 earlier.
  mov x1, x19

  // Restore the extended pstate pointer
  ldr x2, [sp, #8]

  // Call the callback
  blr x3

  // Restore CFI state after callback
  .cfi_restore_state

  // Did the callback tell us to exit?
  cbz x0, .restricted_enter_loop_ret

  // Restore extended processor state
  ldr x0, [sp, #8]
  bl restore_extended_pstate

  // Go back to the loop
  b .restricted_enter_loop_top
  // Never reached.
END_FUNCTION(restricted_return_loop)

// The restricted return entry point is not really a function but we treat it like one. It has the following
// parameters:
// fn restricted_return_loop_aarch32(
//    x0   context: usize,
//    x1   reason_code: u64
// )
FUNCTION(restricted_return_loop_aarch32)
  // The "caller" is actually interrupted restricted mode state.  This means the
  // "return address" will be treated as the precise PC of the "caller", rather
  // than as its return address that's one instruction after the call site.
  .cfi_signal_frame

  // x0 holds the context, which is the stack pointer.
  mov sp, x0

  // Save the reason code in a callee-saved register
  mov x19, x1

  // Restore shadow call stack
  ldr x18, [sp, #40]

  // Save extended processor state for aarch32
  ldr x0, [sp, #8]
  bl save_extended_aarch32_pstate

  // Load the address of the mapped restricted mode register state to x27
  ldr x27, [sp]

  // Load frame pointer from restricted state to connect Starnix stack to Linux's.
  // This offset matches the offset of x29 in the `zx_restricted_state_t` struct.
  ldr x29, [x27, 0xE8]

  // Emit CFI directives referring to the restricted mode register state
  .cfi_remember_state
  .cfi_def_cfa x27, 0
  .cfi_offset x0, 0
  .cfi_offset x1, 0x08
  .cfi_offset x2, 0x10
  .cfi_offset x3, 0x18
  .cfi_offset x4, 0x20
  .cfi_offset x5, 0x28
  .cfi_offset x6, 0x30
  .cfi_offset x7, 0x38
  .cfi_offset x8, 0x40
  .cfi_offset x9, 0x48
  .cfi_offset x10, 0x50
  .cfi_offset x11, 0x58
  .cfi_offset x12, 0x60
  .cfi_offset x13, 0x68
  .cfi_offset x14, 0x70
  .cfi_offset x15, 0x78
  .cfi_offset x16, 0x80
  .cfi_offset x17, 0x88
  .cfi_offset x18, 0x90
  .cfi_offset x19, 0x98
  .cfi_offset x20, 0xA0
  .cfi_offset x21, 0xA8
  .cfi_offset x22, 0xB0
  .cfi_offset x23, 0xB8
  .cfi_offset x24, 0xC0
  .cfi_offset x25, 0xC8
  .cfi_offset x26, 0xD0
  .cfi_offset x27, 0xD8
  .cfi_offset x28, 0xE0
  .cfi_offset x29, 0xE8
  .cfi_offset lr, 0xF0   // x30
  .cfi_offset sp, 0xF8   // x31

  // We cannot refer directly to the PC register as per the AArch64 manual:
  // https://developer.arm.com/documentation/dui0801/l/Overview-of-AArch64-state/Predeclared-core-register-names-in-AArch64-state?lang=en,
  // attempting to do so will silently not produce a CFI directive to recover that register.
  // Instead, we are handcrafting our own DW_CFA_offset_extended_sf CFI expression, pointing at
  // register "32" (which is not specified in the DWARF standard, but the unwinder understands to be
  // "PC") and a value corresponding to the next stack slot at offset 0x100 in SLEB128 format per
  // the format specification for DW_CFA_offset_extended_sf. This way, the unwinder can recover both
  // PC and LR, which enables finding the rest of the restricted mode stack frames.
  .cfi_escape 0x11, 32, 0x40

  // Invoke callback with context and reason_code:
  //       fn restricted_exit_callback_c(
  // x0      context: usize,
  // x1      reason_code: zx::sys::zx_restricted_reason_t,
  // x2      extended_pstate_ptr: *mut ExtendedPstatePointer,
  // x0    ) -> bool

  // Load callback context and function pointers stack
  ldp x3, x0, [sp, #16]

  // Restore reason code that we saved in x19 earlier.
  mov x1, x19

  // Load the extended pstate pointer
  ldr x2, [sp, #8]

  // Call the callback.
  blr x3

  // Restore CFI state after callback
  .cfi_restore_state

  // Did the callback tell us to exit?
  cbz x0, .restricted_enter_loop_ret

  // Restore extended processor state for aarch32
  ldr x0, [sp, #8]
  bl restore_extended_aarch32_pstate

  // Go back to the loop
  b .restricted_enter_loop_top
  // Never reached.
END_FUNCTION(restricted_return_loop_aarch32)

