// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_RISCV64_MMU_H_
#define ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_RISCV64_MMU_H_

#include <lib/page/size.h>
#include <lib/zircon-internal/macros.h>
#include <sys/types.h>

#include <arch/defines.h>
#include <arch/kernel_aspace.h>
#include <arch/riscv64.h>
#include <ktl/tuple.h>

//
// The dimensions of the paging are determined by libpage.
//
// SvXXx4 for hypervisor guest translation
constexpr size_t MMU_GUEST_SIZE_SHIFT = (kVirtualAddressSize + 2);

// page table bits
#define RISCV64_PTE_V (1ul << 0)  // valid
#define RISCV64_PTE_R (1ul << 1)  // read
#define RISCV64_PTE_W (1ul << 2)  // write
#define RISCV64_PTE_X (1ul << 3)  // execute
#define RISCV64_PTE_PERM_MASK (7ul << 1)
#define RISCV64_PTE_U (1ul << 4)         // user
#define RISCV64_PTE_G (1ul << 5)         // global
#define RISCV64_PTE_A (1ul << 6)         // accessed
#define RISCV64_PTE_D (1ul << 7)         // dirty
#define RISCV64_PTE_RSW_MASK (3ul << 8)  // reserved for software
#define RISCV64_PTE_PPN_SHIFT (10)
#define RISCV64_PTE_PPN_BITS (56)
#define RISCV64_PTE_PPN_MASK \
  (((1ul << (RISCV64_PTE_PPN_BITS - kPageTableLevelShift)) - 1) << RISCV64_PTE_PPN_SHIFT)

// Svpbmt (Page based memory types) extension
// Normal memory
#define RISCV64_PTE_PBMT_PMA (0ul << 61)
// Non-cacheable, idempotent, weakly-ordered (RVWMO), main memory
#define RISCV64_PTE_PBMT_NC (1ul << 61)
// Non-cacheable, non-idempotent, strongly-ordered (I/O ordering), I/O
#define RISCV64_PTE_PBMT_IO (2ul << 61)
#define RISCV64_PTE_PBMT_MASK (3ul << 61)

using pte_t = uintptr_t;

// Kernel's use of asids:
//   When using asids, the kernel aspace (active when in kernel threads or idle) will be assigned
// KERNEL_ASID, which is nonzero but otherwise hard assigned to the kernel. Each process is given a
// unique asid for the duration of its lifetime, between FIRST_USER_ASID and MAX_USER_ASID.
//   When not using asids, all aspaces are assigned UNUSED_ASID which is always zero. A full flush
// of the TLB is performed when context switching.
constexpr size_t MMU_RISCV64_ASID_BITS = 16;
constexpr uint16_t MMU_RISCV64_UNUSED_ASID = 0;
constexpr uint16_t MMU_RISCV64_KERNEL_ASID = 1;
constexpr uint16_t MMU_RISCV64_FIRST_USER_ASID = 2;
constexpr uint16_t MMU_RISCV64_MAX_USER_ASID = (1u << MMU_RISCV64_ASID_BITS) - 1;

void riscv64_mmu_early_init();
void riscv64_mmu_early_init_percpu();
void riscv64_mmu_init();
void riscv64_mmu_prevm_init();

// Helper routines for flushing the paging related TLBs on the local cpu
// From RISC-V privileged spec 1.12, section 4.2.1 - Supervisor Memory-Management Fence Instruction

// Flush the TLB completely, including global pages
inline void riscv64_tlb_flush_all() { __asm__("sfence.vma  zero, zero" ::: "memory"); }

// Flush all non global pages from this ASID
inline void riscv64_tlb_flush_asid(uint16_t asid) {
  // Cast the 16bit asid value to a 64bit value before passing into the inline asm to make sure
  // the compiler masks out any potential stray bits.
  __asm__ __volatile__("sfence.vma  zero, %0" ::"r"(static_cast<uint64_t>(asid)) : "memory");
}

// Flush all pages with this address from all ASIDs, including global pages
inline void riscv64_tlb_flush_address_all_asids(vaddr_t va) {
  __asm__ __volatile__("sfence.vma  %0, zero" ::"r"(va) : "memory");
}

// Flush all pages with this address from one ASID, not including global pages
inline void riscv64_tlb_flush_address_one_asid(vaddr_t va, uint16_t asid) {
  // Cast the 16bit asid value to a 64bit value before passing into the inline asm to make sure
  // the compiler masks out any potential stray bits.
  __asm__ __volatile__("sfence.vma  %0, %1" ::"r"(va), "r"(static_cast<uint64_t>(asid)) : "memory");
}

// Extract the asid field out of the SATP register
inline uint16_t riscv64_current_asid() {
  return static_cast<uint16_t>(arch::RiscvSatp::Read().asid());
}

// The physical address of an identity-mapping root page table covering the
// kernel image, stored for loading from secondary CPUs.  The high half is the
// usual full kernel virtual table.
paddr_t riscv64_get_bootstrap_translation_table();

#endif  // ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_RISCV64_MMU_H_
