// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/x86/boot-cpuid.h>
#include <zircon/compiler.h>

#include <arch/x86/gdt.h>
#include <arch/x86/idt.h>
#include <arch/x86/mp.h>
#include <phys/handoff.h>
#include <vm/handoff-end.h>

__NO_SAFESTACK void ArchPostHandoffBootstrap(const ArchPhysHandoff* arch_handoff) {
  // Best to do this early. See docstring for more details.
  load_startup_gdt();

  // Before setting %gs.base to &bp_percpu, copy over the unsafe stack pointer
  // and stack guard set by physboot.
  //
  // What physboot handed off was a temporary region of memory covering the
  // subset of `x86_percpu` dealing in the thread ABI.
  bp_percpu.stack_guard = READ_PERCPU_FIELD(stack_guard);
  bp_percpu.kernel_unsafe_sp = READ_PERCPU_FIELD(kernel_unsafe_sp);
  write_msr(X86_MSR_IA32_GS_BASE, reinterpret_cast<uintptr_t>(&bp_percpu));

  // Set up the idt
  idt_setup(&_idt_startup);
  load_startup_idt();

  // Initialize CPUID value cache - and do so before functions (like
  // x86_init_percpu) begin to access CPUID data.
  arch::InitializeBootCpuid();

  // Assign this core CPU# 0 and initialize its per cpu state
  x86_init_percpu(0);
}
