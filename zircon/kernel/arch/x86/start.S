// Copyright 2016 The Fuchsia Authors
// Copyright (c) 2009 Corey Tabaka
// Copyright (c) 2015 Intel Corporation
// Copyright (c) 2016 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <arch/defines.h>
#include <arch/kernel_aspace.h>
#include <arch/x86/mmu.h>
#include <arch/x86/registers.h>
#include <lib/arch/asm.h>
#include <lib/instrumentation/asan.h>
#include <phys/handoff.h>

// TODO(https://fxbug.dev/379891035): The only thing that happens now in this
// routine, apart from jumping to lk_main(), is kASan setup. That should
// happen holistically during address space setup in physboot, at which point
// this file can be deleted and the kernel entrypoint can be made to be
// lk_main().

// We assume `KERNEL_ASPACE_SIZE` <= 512GB.
.if KERNEL_ASPACE_SIZE > 0x0000008000000000
.err "KERNEL_ASPACE_SIZE must be less than or equal to 512GB"
.endif

.function _start, global
.label PhysbootHandoff, global
  // Save off the handoff pointer in a register that won't get clobbered.
  mov %rdi, %r14

#if __has_feature(address_sanitizer)
  // We modify the currently live address space below.
  mov $X86_CR3_BASE_MASK, %rax
  mov %cr3, %rcx
  andq %rax, %rcx  // %rcx now holds the root page table address.

  // Pick out the table pointed to by the last entry, covering [-512GiB, 0);
  // save it for reference later both outside of this and below to link in
  // the following table.
  add $(NO_OF_PT_ENTRIES - 1) * 8, %rcx
  mov (%rcx), %rcx
  mov $X86_PT_BASE_ADDRESS_MASK, %rax
  and %rax, %rcx
  mov %rcx, upper_512gib_page_table_phys(%rip)

  // After these instructions, virtual addresses may be translated to physical
  // ones by subtracting %rbx.
  lea __executable_start(%rip), %rbx
  sub kKernelPhysicalLoadAddress(%rip), %rbx

  // kASAN tracks memory validity with a 'shadow map' starting at a fixed offset. The shadow map
  // tracks the validity of accesses within an eight-byte region with one byte - zero means that
  // all bytes are valid, non-zero tracks either fine-grained validity or various invalid states.
  //
  // At boot, start with a shadow map of all zeros, allowing every access. Efficiently encode the
  // zeroed shadow map by using a single page of zeros and pointing all kASAN page tables at it.
  //
  // The shadow map covers 512 GB of kernel address space which is the current virtual address
  // space of the kernel. This requires 64 GB of kernel virtual address space, which requires
  // 64 PDP entries.
  // TODO(https://fxbug.dev/42104852): Unmap the shadow's shadow, the region of shadow memory covering the
  // shadow map. This should never be accessed.
  // Make the kasan Page Tables point to the zero page
  movl $NO_OF_PT_ENTRIES, %ecx
  lea kasan_shadow_pt(%rip), %rdi
  lea kasan_zero_page(%rip), %rax
  sub %rbx, %rax  // Translate to physical address
  or $X86_KERNEL_KASAN_INITIAL_PT_FLAGS, %rax
  rep stosq

  // Make the Page Directory point to the Page Table
  movl $NO_OF_PT_ENTRIES, %ecx
  lea kasan_shadow_pd(%rip), %rdi
  lea kasan_shadow_pt(%rip), %rax
  sub %rbx, %rax  // Translate to physical address
  or $X86_KERNEL_KASAN_INITIAL_PD_FLAGS, %rax
  rep stosq

  // Put the page directory entry into the upper 512 GiB table. It's 64 entries starting from
  // the index corresponding to the KASAN_SHADOW_OFFSET virtual address.
  // 64 pdp entries span 64GB of shadow map, covering 512 GB of kernel address space
#define PDP_HIGH_SHADOW_OFFSET (((KASAN_SHADOW_OFFSET) >> 30) & 0x1ff)
  mov upper_512gib_page_table_phys(%rip), %rdi
  add $PDP_HIGH_SHADOW_OFFSET * 8, %rdi
  movl $X86_KERNEL_KASAN_PDP_ENTRIES, %ecx
  lea kasan_shadow_pd(%rip), %rax
  sub %rbx, %rax  // Translate to physical address
  or $X86_KERNEL_KASAN_INITIAL_PD_FLAGS, %rax
  rep stosq
#endif  // __has_feature(address_sanitizer)

  // Call the main module
  mov %r14, %rdi
  call lk_main

0:                          /* just sit around waiting for interrupts */
  hlt                     /* interrupts will unhalt the processor */
  pause
  jmp 0b                  /* so jump back to halt to conserve power */
.end_function
