// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// This file provides real-mode entry points for secondary CPU initialization.

#include <asm.h>
#include <lib/arch/asm.h>
#include <arch/x86/mmu.h>
#include <arch/x86/bootstrap16.h>
#include <arch/x86/descriptor.h>
#include <arch/x86/registers.h>

// This code's only non-PIC instructions are movabs, which can be fixed up
// safely (see gen-kaslr-fixups.sh).  This section name is specially known
// by kernel.ld and gen-kaslr-fixups.sh.
.section .text.bootstrap16,"ax",%progbits
.balign X86_PAGE_SIZE

.label x86_bootstrap16_start, global

.code16
.label x86_bootstrap16_entry, global
    // Enter no-fill cache mode (allegedly this is the initial state
    // according to Intel 3A, but on at least one Broadwell the APs can
    // come up with caching enabled)
    mov %cr0, %ebx
    or $X86_CR0_CD, %ebx
    and $~X86_CR0_NW, %ebx
    mov %ebx, %cr0
0:

    // We cheat a little and don't switch off of our real mode segments in
    // protected mode.  In real mode and protected mode, all of our code
    // and data accesses are relative to %cs and %ss, using the real mode
    // segment calculations.

    // setup %ds/%ss to refer to the data region
    mov %cs, %si
    add $0x100, %si
    mov %si, %ds
    mov %si, %ss

    lgdtl BCD_PHYS_GDTR_OFFSET

    // enter protected mode (but without paging)
    mov %cr0, %ebx
    or $X86_CR0_PE, %ebx
    mov %ebx, %cr0

    // clear instruction prefetch queue
    jmp 0f
0:
    // enable PAE / PGE
    mov %cr4, %ecx
    or $(X86_CR4_PAE|X86_CR4_PGE), %ecx
    mov %ecx, %cr4

    // load CR3 with the bootstrap PML4
    mov BCD_PHYS_BOOTSTRAP_PML4_OFFSET, %ecx
    mov %ecx, %cr3

    // enable IA-32e mode and indicate support for NX pages.
    // need the latter for once we switch to the real kernel
    // address space.
    mov $X86_MSR_IA32_EFER, %ecx
    rdmsr
    or $X86_EFER_LME, %eax
    or $X86_EFER_NXE, %eax
    wrmsr

    // enable paging
    mov %cr0, %ebx
    or $X86_CR0_PG, %ebx
    mov %ebx, %cr0

    // Translate data page segment into full address
    mov %ds, %esi
    shl $4, %esi

    // Jump to 64-bit CS
    mov $BCD_PHYS_LM_ENTRY_OFFSET, %esp
    lretl

// Get the secondary cpu into 64-bit mode with interrupts disabled and no TSS
.code64
.label x86_secondary_cpu_long_mode_entry, global
    // When we get here, %rsi should contain the absolute address of our data
    // page.
    mov $1, %rdi
    LOCK xadd %edi, BCD_CPU_COUNTER_OFFSET(%esi)
    // %rdi is now the index this CPU should use to grab resources

    // Shift index by 2, since the per_cpu member contains two 64-bit values which
    // will be at offsets 8*(2n) and 8*(2n+1) relative to PER_CPU_BASE_OFFSET
    shl $1, %rdi
    // Retrieve the top of this CPUs initial kernel stack
    // Note: the stack is unusable until we switch cr3 below
    mov BCD_PER_CPU_BASE_OFFSET(%rsi, %rdi, 8), %rsp

    // Retrieve this CPUs initial thread
    // Note: the stack is unusable until we switch cr3 below
    add $1, %rdi
    mov BCD_PER_CPU_BASE_OFFSET(%rsi, %rdi, 8), %rdx

    // Retrieve the new PML4 address before our data page becomes unreachable
    mov BCD_PHYS_KERNEL_PML4_OFFSET(%esi), %ecx
    // Similarly for the CPU waiting mask
    mov BCD_CPU_WAITING_OFFSET(%esi), %rdi

    // Switch out of the copied code page and into the kernel's
    // version of it
    jmp *BCD_VIRT_LM_HIGH_ENTRY_OFFSET(%rsi)

.function x86_secondary_cpu_long_mode_high_entry, global
    // Switch to the kernel's PML4
    mov %rcx, %cr3
    // As of this point, %esi is invalid

    // Reload the GDT with one based off of non-identity mapping
    lgdt _temp_gdtr(%rip)

    // Zero our data segments
    xor %eax, %eax
    mov %eax, %ds
    mov %eax, %es
    mov %eax, %fs
    mov %eax, %gs
    mov %eax, %ss

    // Load the IDT. Note that this preserves both %rdx and %rdi (see below).
    call load_startup_idt

    // %rdi (`aps_still_booting`) and %rdx (`thread`) were set above and have
    // been preserved since then.
    mov %rdx, %rsi
    call x86_secondary_entry

// If x86_secondary_entry returns, hang.
0:
    hlt
    jmp 0b
.end_function

.label x86_bootstrap16_end, global
    nop
