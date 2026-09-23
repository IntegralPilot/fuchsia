// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <bits.h>
#include <lib/page/size.h>
#include <lib/unittest/unittest.h>
#include <lib/zircon-internal/macros.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <arch/aspace.h>
#include <arch/x86/mmu.h>
#include <vm/arch_vm_aspace.h>
#include <vm/physmap.h>
#include <vm/pmm.h>
#include <vm/vm_aspace.h>

static bool check_virtual_address_mapped(uint64_t* pml4, vaddr_t va) {
  constexpr uint kPageTableLevels = 4;

  // Virtual Address is split at [47:39], [38:30], [29:21], [20:12]
  uint64_t offsets[kPageTableLevels] = {BITS_SHIFT(va, 47, 39), BITS_SHIFT(va, 38, 30),
                                        BITS_SHIFT(va, 29, 21), BITS_SHIFT(va, 20, 12)};
  uint64_t* current_level = pml4;
  for (uint i = 0; i < kPageTableLevels; i++) {
    uint64_t index = offsets[i];
    uint64_t pte = current_level[index];
    if ((pte & X86_MMU_PG_P) == 0) {
      return false;
    }

    uint64_t next_level_va = X86_PHYS_TO_VIRT(pte & X86_PG_FRAME);
    current_level = reinterpret_cast<uint64_t*>(next_level_va);
  }
  return true;
}

static bool x86_arch_vmaspace_usermmu_tests() {
  BEGIN_TEST;

  constexpr uint kPtPerPageTable = kPageSize / sizeof(uint64_t);
  constexpr uint kUserPtPerPageTable = kPtPerPageTable / 2;
  {
    constexpr uint64_t kTestAspaceSize = 4ull * 1024 * 1024 * 1024;
    constexpr uintptr_t kTestVirtualAddress = kTestAspaceSize - kPageSize;
    // Basic test - make an aspace, map something, query it, check page tables, unmap
    X86ArchVmAspace aspace(0, kTestAspaceSize, /*mmu_flags=*/0);
    EXPECT_EQ(ZX_OK, aspace.Init());
    uint64_t* const pml4 = reinterpret_cast<uint64_t*>(X86_PHYS_TO_VIRT(aspace.pt_phys()));
    // Expect no user mode mappings in an empty address space.
    for (uint i = 0; i < kUserPtPerPageTable; i++) {
      EXPECT_EQ(pml4[i], 0u);
    }

    paddr_t pa = 0;
    vm_page_t* vm_page;
    pmm_alloc_page(/*alloc_flags=*/0, &vm_page, &pa);
    EXPECT_EQ(ZX_OK, aspace.Map(kTestVirtualAddress, &pa, 1,
                                ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE,
                                X86ArchVmAspace::ExistingEntryAction::Error));
    // Directly examine page tables to ensure there's a mapping.
    EXPECT_EQ(check_virtual_address_mapped(pml4, kTestVirtualAddress), true);

    // Use query() interface to find a mapping.
    paddr_t retrieved_pa;
    arch_mmu_flags_t mmu_flags;
    EXPECT_EQ(ZX_OK, aspace.Query(kTestVirtualAddress, &retrieved_pa, &mmu_flags));
    EXPECT_EQ(retrieved_pa, pa);

    EXPECT_EQ(ZX_OK, aspace.Unmap(kTestVirtualAddress, 1, ArchUnmapOptions::Enlarge));
    EXPECT_EQ(check_virtual_address_mapped(pml4, kTestVirtualAddress), false);
    // Expect no user mode mappings after the user mapping was removed.
    for (uint i = 0; i < kUserPtPerPageTable; i++) {
      EXPECT_EQ(pml4[i], 0u);
    }
    pmm_free_page(vm_page);

    aspace.Destroy();
  }

  END_TEST;
}

// Returns true if there was a valid translation or if the page was not present and the translation
// path had a safe phys_addr.
//
// Returns the terminal page table entry in |ptep| and amount to increment address to get the next
// page table entry in |step|.
static bool check_virtual_address_l1tf_invariant(uint64_t* pml4, vaddr_t va, uint64_t* ptep,
                                                 size_t* step) {
  constexpr uint kPageTableLevels = 4;

  // Virtual Address is split at [47:39], [38:30], [29:21], [20:12]
  uint64_t offsets[kPageTableLevels] = {BITS_SHIFT(va, 47, 39), BITS_SHIFT(va, 38, 30),
                                        BITS_SHIFT(va, 29, 21), BITS_SHIFT(va, 20, 12)};
  uint64_t* current_level = pml4;
  size_t next_step = 512 * GB;
  // Walk the page tables down for address |va| until we hit a leaf PTE or a not-present PTE.
  for (uint i = 0; i < kPageTableLevels; i++) {
    uint64_t index = offsets[i];
    uint64_t pte = current_level[index];

    *ptep = pte;
    *step = next_step;
    // L1TF Invariant: If P == 0, then the address must be zero or some safe address.
    if ((pte & X86_MMU_PG_P) == 0) {
      return (pte & X86_PG_FRAME) == 0;
    }
    if (i == 0) {
      DEBUG_ASSERT((pte & X86_MMU_PG_PS) == 0);  // No 512 GB pages.
    }
    // On finding a large page, terminate the walk early.
    if ((pte & X86_MMU_PG_PS)) {
      return true;
    }

    uint64_t next_level_va = X86_PHYS_TO_VIRT(pte & X86_PG_FRAME);
    current_level = reinterpret_cast<uint64_t*>(next_level_va);
    next_step = next_step >> ADDR_OFFSET;  // next_step steps by the address space under this PTE
  }
  return true;
}

static bool x86_test_l1tf_invariant() {
  BEGIN_TEST;

  // Mitigating L1TF requires that no PTE with the present bit set to false points to a page frame.
  // Check the page tables for the kernel physmap and for the bottom 512 GB of the user address
  // space of the current address space.
  //
  // A complete check would check every valid address of every address space, which could take too
  // long to be included in a kernel unit test; based on examination of code and this unit test,
  // we have some confidence the kernel is not breaking this invariant.
  constexpr size_t kUserMemoryToCheck = 512 * GB;

  // Check all page tables for the physmap, to make sure there are no page table enties with a valid
  // address but the present bit not set.
  uint64_t* const pml4 = reinterpret_cast<uint64_t*>(
      X86_PHYS_TO_VIRT(VmAspace::kernel_aspace()->arch_aspace().pt_phys()));
  for (uintptr_t addr = gPhysmapBase; addr < (gPhysmapBase + gPhysmapSize);) {
    uint64_t pte = 0;
    size_t step = 0;
    bool ok = check_virtual_address_l1tf_invariant(pml4, addr, &pte, &step);
    DEBUG_ASSERT(step > 0);
    char error_address[32];
    if (!ok) {
      snprintf(error_address, sizeof(error_address), "0x%lx pte=%lx ", addr, pte);
    }
    EXPECT_TRUE(ok, error_address);
    addr += step;
  }

  // Check kUserMemoryToCheck addresses, to make sure there are no page tables with a valid address
  // but with the present bit set.
  for (uintptr_t addr = USER_ASPACE_BASE; addr < (USER_ASPACE_BASE + kUserMemoryToCheck);) {
    uint64_t pte = 0;
    size_t step = 0;
    bool ok = check_virtual_address_l1tf_invariant(pml4, addr, &pte, &step);
    char error_address[32];
    snprintf(error_address, sizeof(error_address), "0x%lx pte=%lx ", addr, pte);
    EXPECT_TRUE(ok, error_address);
    addr += step;
  }

  END_TEST;
}

// Ensure the physmap is marked no-execute.
static bool x86_test_physmap_nx() {
  BEGIN_TEST;

  for (uintptr_t addr = gPhysmapBase; addr < (gPhysmapBase + gPhysmapSize); addr += kPageSize) {
    paddr_t paddr;
    arch_mmu_flags_t mmu_flags;
    zx_status_t status = VmAspace::kernel_aspace()->arch_aspace().Query(addr, &paddr, &mmu_flags);
    if (status == ZX_OK) {
      EXPECT_TRUE(!(mmu_flags & ARCH_MMU_FLAG_PERM_EXECUTE));
    } else {
      EXPECT_EQ(status, ZX_ERR_NOT_FOUND);
    }
  }

  END_TEST;
}

// Ensure that Destroy works on unified aspaces without triggering lockdep.
// TODO(https://fxbug.dev/42083004): Move this to the arch-agnostic tests in
// zircon/kernel/tests/mmu_tests.cc once unified aspaces are supported on all architectures.
static bool x86_test_destroy_unified() {
  BEGIN_TEST;

  // Create a shared and restricted aspace that do not share a PML4 entry.
  constexpr uint64_t kTestAspaceSize = 4ull * kPageSize;
  constexpr uint64_t kTestSharedAspaceBase = 1ULL << PML4_SHIFT;
  X86ArchVmAspace restricted(0, kTestAspaceSize, /*mmu_flags=*/0);
  EXPECT_EQ(ZX_OK, restricted.InitRestricted());
  X86ArchVmAspace shared(kTestSharedAspaceBase, kTestAspaceSize, /*mmu_flags=*/0);
  EXPECT_EQ(ZX_OK, shared.InitShared());

  // Create a unified aspace consisting of the above aspaces.
  X86ArchVmAspace unified(0, kTestAspaceSize, /*mmu_flags=*/0);
  EXPECT_EQ(ZX_OK, unified.InitUnified(shared, restricted));

  // Destroy all 3 and make sure it succeeds.
  EXPECT_OK(unified.Destroy());
  EXPECT_OK(restricted.Destroy());
  EXPECT_OK(shared.Destroy());

  END_TEST;
}

UNITTEST_START_TESTCASE(x86_mmu_tests)
UNITTEST("user-aspace page table tests", x86_arch_vmaspace_usermmu_tests)
UNITTEST("l1tf test", x86_test_l1tf_invariant)
UNITTEST("physmap nx", x86_test_physmap_nx)
UNITTEST("destroy unified", x86_test_destroy_unified)
UNITTEST_END_TESTCASE(x86_mmu_tests, "x86_mmu", "x86 mmu tests")
