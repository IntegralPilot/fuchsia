// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/asm.h>
#include <arch/regs.h>
#include <arch/riscv64.h>
#include <arch/riscv64/mp.h>
#include <lib/syscalls/zx-syscall-numbers.h>
#include <zircon/errors.h>

#ifdef KERNEL_NO_USERABI
// If we're not building user abi there are no wrapper functions or
// proper unknown_syscall function for the dispatcher to dispatch to,
// so define a stub routine so the code at least links.
.function unknown_syscall
  lla a0, unknown_syscall_panic_string
  j panic
.end_function

.object unknown_syscall_panic_string, type=rodata
  .string "no syscall hooks to call!"
.end_object
#endif // KERNEL_NO_USERABI

// void riscv64_syscall_dispatcher(iframe_t* iframe);
// Registers in the iframe are parsed using the following convention:
//
//   a0-a7 - contains syscall arguments
//   t0    - contains syscall_num
//   pc    - contains the syscall instruction address
//
.function riscv64_syscall_dispatcher, global
  addi sp, sp, -16
  // Store ra to the +8 slot, leaving 0(sp) free for potential syscall
  // table use below.
  sd ra, 8(sp)

  // Check if we're issuing a syscall from restricted mode.
  // We do this after storing RA to make sure we don't lose the call chain.
  lw t3, PERCPU_IN_RESTRICTED_MODE(s11)
  bnez t3, .Lrestricted_syscall

  ld t1, RISCV64_IFRAME_OFFSET_PC(a0)
  ld t0, RISCV64_IFRAME_OFFSET_T0(a0)
  // Load a0 last since it points to the iframe.
  ld a1, RISCV64_IFRAME_OFFSET_A1(a0)
  ld a2, RISCV64_IFRAME_OFFSET_A2(a0)
  ld a3, RISCV64_IFRAME_OFFSET_A3(a0)
  ld a4, RISCV64_IFRAME_OFFSET_A4(a0)
  ld a5, RISCV64_IFRAME_OFFSET_A5(a0)
  ld a6, RISCV64_IFRAME_OFFSET_A6(a0)
  ld a7, RISCV64_IFRAME_OFFSET_A7(a0)
  ld a0, RISCV64_IFRAME_OFFSET_A0(a0)

  // Verify syscall number and call the unknown handler if bad.
  li t2, ZX_SYS_COUNT
  bgeu t0, t2, .Lunknown_syscall

  // Jump to the right syscall wrapper C++ function via assembly
  // trampolines.  The trampolines expect the user PC in t1 and the user's
  // arguments in the a0..a7 registers; they may write to the word at 0(sp).
  // Each trampoline marshalls some arguments and tail calls the routine
  // which causes the wrapper to return to .Lpost_syscall.  The syscall
  // table is an array of offsets to trampoline entry points defined by the
  // `syscall_dispatcher` macro (see below).
  lla t2, syscall_dispatcher_table
#ifdef __riscv_zba
  sh2add t0, t0, t2  // Index into the table. (t0 = t0 * 4 + t2)
#else
  sll t0, t0, 2   // Scale to 4 bytes per entry.
  add t0, t2, t0  // Index into the table.
#endif
  lw t2, (t0)    // Load 32-bit offset and sign-extend it.
  add t2 , t2, t0  // Materialize the full trampoline PC.
  jalr t2

.Lpost_syscall:
  ld ra, 8(sp)
  addi sp, sp, 16
  ret

.Lunknown_syscall:
  mv a0, t0 // move the syscall number into the 0 arg slot
  mv a1, t1 // pc into arg 1
  call unknown_syscall
  j .Lpost_syscall

.Lrestricted_syscall:
  call syscall_from_restricted // This does not return.
  unimp

.end_function

.object syscall_dispatcher_table, type=rodata, align=4

//
// Syscall args are in a0-a7 already.
// pc is in t1 and needs to go in the next available register,
// or the stack if the regs are full.
//
.macro syscall_dispatcher nargs, syscall
  // Each trampoline goes into its own little .text section that the
  // linker can move around arbitrarily. Relaxation can change the size
  // of the code sequence at link time: `tail`, `call`, and `j` always
  // expand to a multi-instruction sequence that might get relaxed down
  // to a single instruction.
  .pushsection .text.syscall_dispatcher.\syscall, "ax", %progbits
  syscall_dispatcher.\syscall:  // This label will appear in disassembly.
.if \nargs == 8
  // Store the PC in the slot we already left on the stack.
  sd t1, (sp)
.else
  mv a\nargs, t1
.endif
#ifndef KERNEL_NO_USERABI
  tail wrapper_\syscall
#else
  tail unknown_syscall
#endif
  .popsection

  // Each trampoline's address goes into the table so it can be indexed as a
  // flat array of fixed-size pointers.  To keep the table smaller, rather
  // than full pointers, store 32-bit offsets from the location of the
  // pointer itself.  All the macro invocations are within the table's
  // section, so after the .popsection this adds the next entry to the end.
  .int syscall_dispatcher.\syscall - .
.endm

// One of these macros is invoked by kernel.inc for each syscall.
// These are the direct kernel entry points.
#define KERNEL_SYSCALL(name, type, attrs, nargs, arglist, prototype) \
  syscall_dispatcher nargs, name
#define INTERNAL_SYSCALL(...) KERNEL_SYSCALL(__VA_ARGS__)
#define BLOCKING_SYSCALL(...) KERNEL_SYSCALL(__VA_ARGS__)
// These don't have kernel entry points.
#define VDSO_SYSCALL(...)

#include <lib/syscalls/kernel.inc>

#undef KERNEL_SYSCALL
#undef INTERNAL_SYSCALL
#undef BLOCKING_SYSCALL
#undef VDSO_SYSCALL

.end_object  // syscall_dispatcher_table
