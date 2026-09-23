// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/arm64/exception-asm.h>
#include <lib/arch/arm64/feature-asm.h>
#include <lib/arch/cache.h>
#include <lib/arch/asm.h>
#include <lib/arch/ticks.h>
#include <zircon/tls.h>

#include <phys/stack.h>

// This is the entry point from the boot loader or thereabouts.
// It receives one argument, in x0, usually a pointer (physical address).
// The caches and MMU are disabled.
//
// In a ZBI executable, this is where zbi_kernel_t::entry points and
// x0 holds the address of the data ZBI.
//
// In a boot shim, the header code jumps here after normalizing machine state
// and x0 holds what's usually a Device Tree address.
.function _start, global
  // As early as possible collect the time stamp.  x1 and x2 will be the
  // second argument to PhysMain, the arch::EarlyTicks structure passed in
  // two registers.  The first argument is already in x0.
  sample_ticks x1, x2, x9, x10, x11

  // Clear return address and frame pointer: at the root of the call stack.
  mov x29, xzr
  mov x30, xzr

  // Clear any incoming stack pointer so it can't be used accidentally
  // before the proper stack is set up below.
  mov sp, x29

  // Mask all interrupts in case the boot loader left them on.
  msr DAIFSet, #DAIFSETCLR_MASK

  // Reset the exception vector base address registers in case the boot loader
  // left an old table in place (which we might already be clobbering, and
  // almost certainly will be violating the assumptions of).
  msr_vbar_elx x16, xzr  // x16 is clobbered as a scratch register (CurrentEL).

  bootloader_data .req x9
  mov bootloader_data, x0

  // Invalidate any data cache lines above writable segments. The ZBI protocol
  // only guarantees that the memory image is cleaned; however, the presence of
  // any retained cache lines would result in incoherence if we modify the
  // corresponding data before re-enabling the data cache.
  adr_global x0, __data_start
  adr_global x11, _end
  sub x1, x11, x0
  data_cache_range_op ivac

  // Now clear .bss.  Note this assumes it's aligned to 16, which is ensured
  // by BOOT_STACK_ALIGN (and other alignments in .bss) and the linker script.
  // Note that x11 still holds _end.
  adr_global x10, _edata
0:
  stp xzr, xzr, [x10], #16
  cmp x10, x11
  blt 0b

  // Set up the stacks and the thread pointer area.
  tp .req x10
  adr_global tp, boot_thread_pointer
  msr TPIDR_EL1, tp

  // Stack guard canary value.
  boot_stack_guard .req x11

  // If hardware random numbers are available, use them.
  .arch armv8.5-a+rng
  mrs boot_stack_guard, ID_AA64ISAR0_EL1
  tst boot_stack_guard, #ID_AA64ISAR0_EL1_RNDR
  beq .Lno_rndr
  // Reading this register sets the Z flag if no good randomness is delivered.
  mrs boot_stack_guard, RNDRRS
  bne .Lstack_guard_done
  mrs boot_stack_guard, RNDR
  bne .Lstack_guard_done

.Lno_rndr:
  // The only "randomness" readily available is our own load address, so
  // swizzle that in with some arbitrary bits.
  movlit boot_stack_guard, 0xdeadbeef1ee2d00d
  eor boot_stack_guard, tp, boot_stack_guard
.Lstack_guard_done:

  // Before we set sp, ensure that it's 'linked' to the current SP_ELX and not
  // SP_EL0.
  msr SPSel, #1

#if ZX_TLS_STACK_GUARD_OFFSET + 8 != ZX_TLS_UNSAFE_SP_OFFSET
#error "TLS ABI layout???"
#endif
  stp boot_stack_guard, xzr, [tp, #ZX_TLS_STACK_GUARD_OFFSET]

  adr_global x12, boot_stack + BOOT_STACK_SIZE
  mov sp, x12

#if __has_feature(shadow_call_stack)
  // Shadow call stack grows up.
  adr_global shadow_call_sp, boot_shadow_call_stack
#endif

  // Now the full C++ ABI is available.  This could theoretically be a tail
  // call since it's obliged never to return, but it's nice to have the
  // caller in a backtrace.
  mov x0, bootloader_data
  bl PhysMain

  // Trap forever just in case it does return.
0:
  brk #1
  b 0b
.end_function

.object boot_thread_area, bss, local, align=8
  .skip (-ZX_TLS_STACK_GUARD_OFFSET)
#if ZX_TLS_UNSAFE_SP_OFFSET < ZX_TLS_STACK_GUARD_OFFSET
  .error "TLS ABI layout??"
#endif
.label boot_thread_pointer, global
.end_object

// extern "C" void ArmSetVbar(const void* table);
// x0 = table
.function ArmSetVbar, global
  msr_vbar_elx x1, x0
  ret
.end_function
