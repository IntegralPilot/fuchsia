// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/asm.h>
#include <lib/arch/riscv64/system-asm.h>
#include <zircon/tls.h>

#include <phys/stack.h>

#include "arch-phys-info-asm.h"

// This is the entry point from the boot loader or thereabouts.
// It receives two arguments, in a0, a1 (x10, x11): a0 is the HART ID and
// a1 is usually a pointer (physical address).  The MMU is disabled.
//
// In a ZBI executable, this is where zbi_kernel_t::entry points and
// a1 (x11) holds the address of the data ZBI.
//
// In a boot shim, the header code jumps here after normalizing machine state
// and a1 (x11) holds what's usually a Device Tree address.
.function _start, global
  // As early as possible collect the time stamp.  It will be the second
  // argument to PhysMain.  We'll leave a2 (time), a0 (hart ID), and a1 (ZBI /
  // DTB) alone while we clear the bss.
  rdtime a2

  // Clear return address and frame pointer: at the root of the call stack.
  mv ra, zero
  mv s0, zero

  // Clear any incoming stack pointer so it can't be used accidentally
  // before the proper stack is set up below.
  mv sp, zero

  // Clear the gp register in case anything tries to use it.
  // When shadow-call-stack is enabled, this is shadow_call_sp and set below.
  mv gp, zero

  // Mask all interrupts in case the boot loader left them on.
  csrc sstatus, SSTATUS_SIE
  csrw sie, zero

  // Reset the trap vector base address register in case the boot loader
  // left an old vector in place (which we might already be clobbering, and
  // almost certainly will be violating the assumptions of).
  csrw stvec, zero

  // Disable the MMU just in case it was left on (it should not have been).
  csrw satp, zero

  // Clear .bss.  The linker script ensures these are aligned to 16 bytes.
  lla a3, _edata
  lla a4, _end
0:
  sd zero, (a3)
  sd zero, 8(a3)
  add a3, a3, 16
  blt a3, a4, 0b

  // Now that bss has been cleared, store the hart ID in a bss variable.
  // And set up the arguments for PhysMain.
  lla a3, gArchPhysInfoStorage
  sd a0, ARCH_PHYS_INFO_BOOT_HART_ID(a3)
  mv a0, a1 // ZBI / DTB pointer
  mv a1, a2 // arch::EarlyTicks

  // Set up the stacks and the thread pointer area.
  lla tp, boot_thread_pointer

  // Stack guard canary value.
#define boot_stack_guard a2

  // TODO: csr read seed, check bits, loop?

.Lno_seed:
  // The only "randomness" readily available is our own load address, so
  // swizzle that in with some arbitrary bits.
  li boot_stack_guard, 0xdeadbeef1ee2d00d
  or boot_stack_guard, tp, boot_stack_guard
.Lstack_guard_done:
  sd boot_stack_guard, ZX_TLS_STACK_GUARD_OFFSET(tp)

  lla sp, boot_stack + BOOT_STACK_SIZE

#if __has_feature(shadow_call_stack)
  lla shadow_call_sp, boot_shadow_call_stack
#else
  mv shadow_call_sp, zero
#endif

  // Now the full C++ ABI is available.  This could theoretically be a tail
  // call since it's obliged never to return, but it's nice to have the
  // caller in a backtrace.
  call PhysMain

  // Trap forever just in case it does return.
0:
  unimp
  j 0b
.end_function

.object boot_thread_area, bss, local, align=8
  .skip (-ZX_TLS_STACK_GUARD_OFFSET)
#if ZX_TLS_UNSAFE_SP_OFFSET < ZX_TLS_STACK_GUARD_OFFSET
  .error "TLS ABI layout??"
#endif
.label boot_thread_pointer, global
.end_object
