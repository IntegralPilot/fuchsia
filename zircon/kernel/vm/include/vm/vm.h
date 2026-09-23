// Copyright 2016 The Fuchsia Authors
// Copyright (c) 2014 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_VM_INCLUDE_VM_VM_H_
#define ZIRCON_KERNEL_VM_INCLUDE_VM_VM_H_

#include <align.h>
#include <arch.h>
#include <lib/ktrace.h>
#include <stdint.h>
#include <stdlib.h>
#include <sys/types.h>
#include <zircon/compiler.h>

#include <arch/kernel_aspace.h>
#include <arch/vm.h>
#include <ktl/span.h>
#include <vm/arch_vm_aspace.h>

#ifndef VM_TRACING_LEVEL
#define VM_TRACING_LEVEL 0
#endif

// Evaluates to true if tracing is enabled for the given level.
#define VM_KTRACE_LEVEL_ENABLED(level) ((VM_TRACING_LEVEL) >= (level))

#define VM_MAKE_UNIQUE_TOKEN3(a, b) a##b
#define VM_MAKE_UNIQUE_TOKEN2(a, b) VM_MAKE_UNIQUE_TOKEN3(a, b)
#define VM_MAKE_UNIQUE_TOKEN(a) VM_MAKE_UNIQUE_TOKEN2(a, __LINE__)

#define VM_KTRACE_DURATION(level, string, args...) \
  ktrace::Scope VM_MAKE_UNIQUE_TOKEN(_duration_) = \
      KTRACE_BEGIN_SCOPE_ENABLE(VM_KTRACE_LEVEL_ENABLED(level), "kernel:vm", string, ##args)

// In builds with verbose VM tracing enabled (VM_TRACING_LEVEL >= 1), we emit these events under the
// "kernel:vm" category at the specified VM tracing level. In standard/production builds
// (VM_TRACING_LEVEL = 0), we fallback to emitting under the lightweight "kernel:oom" category
// instead.
//
// The general expectation is that users will enable "kernel:oom" in cases where "kernel:vm" is
// too expensive. Using a compile-time selection here prevents duplicate overlapping slices in
// the trace visualizer when both categories are enabled at runtime, while still providing a
// single category for developers opting for more verbose VM tracing.
#if VM_TRACING_LEVEL >= 1
#define OOM_KTRACE_DURATION(level, string, args...) VM_KTRACE_DURATION(level, string, ##args)
#else
#define OOM_KTRACE_DURATION(level, string, args...) \
  ktrace::Scope VM_MAKE_UNIQUE_TOKEN(_duration_) = KTRACE_BEGIN_SCOPE("kernel:oom", string, ##args)
#endif

#define VM_KTRACE_DURATION_BEGIN(level, string, args...) \
  KTRACE_DURATION_BEGIN_ENABLE(VM_KTRACE_LEVEL_ENABLED(level), "kernel:vm", string, ##args)

#define VM_KTRACE_DURATION_END(level, string, args...) \
  KTRACE_DURATION_END_ENABLE(VM_KTRACE_LEVEL_ENABLED(level), "kernel:vm", string, ##args)

#define VM_KTRACE_FLOW_BEGIN(level, string, flow_id, args...) \
  KTRACE_FLOW_BEGIN_ENABLE(VM_KTRACE_LEVEL_ENABLED(level), "kernel:vm", string, flow_id, ##args)

#define VM_KTRACE_FLOW_END(level, string, flow_id, args...) \
  KTRACE_FLOW_END_ENABLE(VM_KTRACE_LEVEL_ENABLED(level), "kernel:vm", string, flow_id, ##args)

#define VM_KTRACE_INSTANT(level, string, args...) \
  KTRACE_INSTANT_ENABLE(VM_KTRACE_LEVEL_ENABLED(level), "kernel:vm", string, ##args)

class VmAspace;

// kernel address space
static_assert(KERNEL_ASPACE_BASE + (KERNEL_ASPACE_SIZE - 1) > KERNEL_ASPACE_BASE, "");

// user address space, defaults to below kernel space with a 16MB guard gap on either side
static_assert(USER_ASPACE_BASE + (USER_ASPACE_SIZE - 1) > USER_ASPACE_BASE, "");

// linker script provided variables for various virtual kernel addresses
extern const char __executable_start[];
extern const char __code_start[];
extern const char __code_end[];
extern const char __rodata_start[];
extern const char __rodata_end[];
extern const char __relro_start[];
extern const char __relro_end[];
extern char __data_start[];
extern char __data_end[];
extern char __bss_start[];
extern char _end[];

extern paddr_t zero_page_paddr;
extern vm_page_t* zero_page;

// Ends the VM's role within the context of phys handoff: it destroys the VMAR
// containing the mappings backing temporary hand-off data.
void vm_end_handoff();

// return a pointer to the zero page
static inline vm_page_t* vm_get_zero_page(void) { return zero_page; }

// return the physical address of the zero page
static inline paddr_t vm_get_zero_page_paddr(void) { return zero_page_paddr; }

// Whether the virtual heap is enabled.
bool vm_using_virtual_heap();

// Request the heap dimensions.
vaddr_t vm_get_kernel_heap_base();
size_t vm_get_kernel_heap_size();

// internal kernel routines below, do not call directly

// internal routine by the scheduler to swap mmu contexts
void vmm_context_switch(VmAspace* oldspace, VmAspace* newaspace);

// set the current user aspace as active on the current thread.
// NULL is a valid argument, which unmaps the current user address space
void vmm_set_active_aspace(VmAspace* aspace);

#endif  // ZIRCON_KERNEL_VM_INCLUDE_VM_VM_H_
