// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_ASPACE_H_
#define ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_ASPACE_H_

// RISC-V arch-specific declarations for VmAspace implementation.

#include <debug.h>
#include <lib/page/size.h>
#include <lib/zx/result.h>

#include <arch/riscv64/mmu.h>
#include <vm/arch_vm_aspace.h>
#include <vm/mapping_cursor.h>

enum class Riscv64AspaceType {
  kUser,    // Userspace address space.
  kKernel,  // Kernel address space.
  kGuest,   // Second-stage address space.
};

enum class Riscv64AspaceRole : uint8_t {
  kIndependent,
  kRestricted,
  kShared,
  kUnified,
};

class Riscv64ArchVmAspace final : public ArchVmAspaceInterface {
 public:
  Riscv64ArchVmAspace(vaddr_t base, size_t size, Riscv64AspaceType type,
                      page_alloc_fn_t test_paf = nullptr);
  Riscv64ArchVmAspace(vaddr_t base, size_t size, arch_mmu_flags_t mmu_flags,
                      page_alloc_fn_t test_paf = nullptr);
  virtual ~Riscv64ArchVmAspace();

  using ArchVmAspaceInterface::page_alloc_fn_t;

  // See comments on `ArchVmAspaceInterface` where these methods are declared.
  zx_status_t Init() override;
  zx_status_t InitShared() override;
  zx_status_t InitRestricted() override;
  zx_status_t InitUnified(ArchVmAspaceInterface& shared,
                          ArchVmAspaceInterface& restricted) override;

  void DisableUpdates() override;

  zx_status_t Destroy() override;

  // main methods
  zx_status_t Map(vaddr_t vaddr, paddr_t* phys, size_t count, arch_mmu_flags_t mmu_flags,
                  ExistingEntryAction existing_action) override;
  zx_status_t MapContiguous(vaddr_t vaddr, paddr_t paddr, size_t count,
                            arch_mmu_flags_t mmu_flags) override;

  zx_status_t Unmap(vaddr_t vaddr, size_t count, ArchUnmapOptions enlarge) override;
  // riscv does not document any restrictions on manipulating page tables such that duplicate TLB
  // entries could exist, as long as sfence.vma gets called, so unmap is safe to split large pages
  // without enlarging.
  bool UnmapOnlyEnlargeOnOom() const override { return true; }

  zx_status_t Protect(vaddr_t vaddr, size_t count, arch_mmu_flags_t mmu_flags,
                      ArchUnmapOptions enlarge) override;

  zx_status_t Query(vaddr_t vaddr, paddr_t* paddr, arch_mmu_flags_t* mmu_flags) override;

  vaddr_t PickSpot(vaddr_t base, vaddr_t end, vaddr_t align, size_t size,
                   arch_mmu_flags_t mmu_flags) override;

  zx_status_t MarkAccessed(vaddr_t vaddr, size_t count) override;

  zx_status_t HarvestAccessed(vaddr_t vaddr, size_t count, NonTerminalAction non_terminal_action,
                              TerminalAction terminal_action) override;

  bool AccessedSinceLastCheck(bool clear) override;

  paddr_t arch_table_phys() const override { return 0; }
  uint16_t arch_asid() const { return 0; }
  uint16_t asid() const { return asid_; }
  void arch_set_asid(uint16_t asid) {}

  static void ContextSwitch(Riscv64ArchVmAspace* from, Riscv64ArchVmAspace* to);

  static void HandoffPageTablesFromPhysboot(VmPageDoublyLinkedList* mmu_pages);

  static constexpr bool HasNonTerminalAccessedFlag() { return false; }

  static constexpr vaddr_t NextUserPageTableOffset(vaddr_t va) {
    // Work out the virtual address the next page table would start at by first masking the va down
    // to determine its index, then adding 1 and turning it back into a virtual address.
    const uint pt_bits = (kPageShift - 3);
    const uint page_pt_shift = kPageShift + pt_bits;
    return ((va >> page_pt_shift) + 1) << page_pt_shift;
  }

  Riscv64AspaceType type() const { return type_; }
  bool IsKernel() const { return type_ == Riscv64AspaceType::kKernel; }
  bool IsUser() const { return type_ == Riscv64AspaceType::kUser; }

  // Returns 1 for unified aspaces and 0 for all other aspaces. This establishes an ordering that
  // is used when the lock_ is acquired. The restricted aspace page table lock is acquired first,
  // and the unified aspace lock is acquired afterwards.
  uint32_t LockOrder() const { return IsUnified() ? 1 : 0; }

 private:
  class ConsistencyManager;
  inline bool IsValidVaddr(vaddr_t vaddr) const {
    return (vaddr >= base_ && vaddr <= base_ + size_ - 1);
  }

  // Page table management.
  volatile pte_t* GetPageTable(vaddr_t pt_index, volatile pte_t* page_table) TA_REQ(lock_);

  zx::result<paddr_t> AllocPageTable() TA_REQ(lock_);

  void FreePageTable(void* vaddr, paddr_t paddr, ConsistencyManager& cm) TA_REQ(lock_);

  zx_status_t MapPageTable(pte_t attrs, bool ro, uint level, volatile pte_t* page_table,
                           ExistingEntryAction existing_action, MappingCursor& cursor,
                           ConsistencyManager& cm) TA_REQ(lock_);

  zx::result<size_t> UnmapPageTable(vaddr_t vaddr, vaddr_t vaddr_rel, size_t size,
                                    ArchUnmapOptions enlarge, uint level,
                                    volatile pte_t* page_table, ConsistencyManager& cm)
      TA_REQ(lock_);

  zx_status_t ProtectPageTable(vaddr_t vaddr, vaddr_t vaddr_rel, size_t size_in, pte_t attrs,
                               uint level, volatile pte_t* page_table, ConsistencyManager& cm)
      TA_REQ(lock_);

  void HarvestAccessedPageTable(vaddr_t vaddr_in, vaddr_t vaddr_rel_in, size_t size_in, uint level,
                                NonTerminalAction non_terminal_action,
                                TerminalAction terminal_action, volatile pte_t* page_table,
                                ConsistencyManager& cm, bool* unmapped_out) TA_REQ(lock_);

  void MarkAccessedPageTable(vaddr_t vaddr, vaddr_t vaddr_rel, size_t size, uint level,
                             volatile pte_t* page_table, ConsistencyManager& cm) TA_REQ(lock_);

  // Splits a descriptor block into a set of next-level-down page blocks/pages.
  //
  // |vaddr| is the virtual address of the start of the block being split. |level|
  // is the level of the page table entry of the descriptor blocking being split.
  // |page_size_shift| is the page size shift of the current aspace. |page_table| is the
  // page table that contains the descriptor block being split, and |pt_index| is the index
  // into that table.
  zx_status_t SplitLargePage(vaddr_t vaddr, uint level, vaddr_t pt_index,
                             volatile pte_t* page_table, ConsistencyManager& cm) TA_REQ(lock_);

  zx::result<size_t> UnmapPages(vaddr_t vaddr, size_t size, ArchUnmapOptions enlarge,
                                ConsistencyManager& cm) TA_REQ(lock_);

  zx_status_t ProtectPages(vaddr_t vaddr, size_t size, pte_t attrs) TA_REQ(lock_);
  zx_status_t QueryLocked(vaddr_t vaddr, paddr_t* paddr, arch_mmu_flags_t* mmu_flags) TA_REQ(lock_);

  void FlushTLBEntryRun(vaddr_t vaddr, size_t page_count) const TA_REQ(lock_);

  void FlushAsid() const TA_REQ(lock_);

  // Returns true if the given level corresponds to the top level page table.
  static bool IsTopLevel(uint level) { return level == kNumPageTableLevels - 1; }

  bool IsShared() const { return role_ == Riscv64AspaceRole::kShared; }
  bool IsRestricted() const { return role_ == Riscv64AspaceRole::kRestricted; }
  bool IsUnified() const { return role_ == Riscv64AspaceRole::kUnified; }

  // The `DestroyIndividual` and `DestroyUnified` functions are called by the public `Destroy`
  // function depending on whether this address space is unified or not. Note that unified aspaces
  // must be destroyed prior to destroying their constituent aspaces. In other words, the shared
  // and restricted aspaces must have a longer lifetime than the unified aspaces they are part of.
  zx_status_t DestroyIndividual();
  zx_status_t DestroyUnified();

  // Frees the top level page table.
  void FreeTopLevelPage() TA_REQ(lock_);

  // Accessors for the shared and restricted aspaces associated with a unified aspace.
  // We can turn off thread safety analysis as these accessors should only be used on unified
  // aspaces, for which both the shared and restricted aspace pointers are notionally const.
  Riscv64ArchVmAspace* get_shared_aspace() TA_NO_THREAD_SAFETY_ANALYSIS {
    DEBUG_ASSERT(IsUnified());
    return shared_aspace_;
  }
  Riscv64ArchVmAspace* get_restricted_aspace() TA_NO_THREAD_SAFETY_ANALYSIS {
    DEBUG_ASSERT(IsUnified());
    return referenced_aspace_;
  }

  // Accessor for the unified aspace associated with a specific restricted aspace.
  // Thread safety analysis is required for this because the referenced_aspace_ pointer is set
  // after the restricted aspace is created during unified aspace creation.
  Riscv64ArchVmAspace* get_unified_aspace() const TA_REQ(lock_) {
    DEBUG_ASSERT(IsRestricted());
    return referenced_aspace_;
  }
  // data fields
  fbl::Canary<fbl::magic("VAAS")> canary_;

  DECLARE_MUTEX(Riscv64ArchVmAspace, lockdep::LockFlagsNestable) lock_;

  // Page allocate function, if set will be used instead of the default allocator
  const page_alloc_fn_t test_page_alloc_func_ = nullptr;

  uint16_t asid_ = MMU_RISCV64_UNUSED_ASID;

  // Pointer to the translation table.
  paddr_t tt_phys_ = 0;
  volatile pte_t* tt_virt_ = nullptr;

  // Upper bound of the number of pages allocated to back the translation
  // table.
  size_t pt_pages_ = 0;

  // Type of address space.
  const Riscv64AspaceType type_;

  // The role this aspace plays in unified aspaces, if any. This should only be set by the `Init*`
  // functions and should not be modified anywhere else.
  Riscv64AspaceRole role_ = Riscv64AspaceRole::kIndependent;

  // The number of times entries in the top level page are referenced by other page tables.
  // Unified aspaces increment and decrement this value on their associated shared and restricted
  // aspaces, so we must hold the lock_ when doing so.
  uint32_t num_references_ TA_GUARDED(lock_) = 0;

  // A pointer to another aspace that shares entries with this one.
  // If this is a restricted aspace, this references the associated unified aspace.
  // If this is a unified aspace, this references the associated restricted aspace.
  // If neither is true, this is set to null.
  Riscv64ArchVmAspace* referenced_aspace_ TA_GUARDED(lock_) = nullptr;

  // A pointer to the shared aspace whose contents are shared with this unified aspace.
  // Set to null if this is not a unified aspace.
  Riscv64ArchVmAspace* shared_aspace_ TA_GUARDED(lock_) = nullptr;

  // Range of address space.
  const vaddr_t base_ = 0;
  const size_t size_ = 0;

  // Number of CPUs this aspace is currently active on.
  ktl::atomic<uint32_t> num_active_cpus_ = 0;

  // Whether not this has been accessed since |AccessedSinceLastCheck| was called.
  ktl::atomic<bool> accessed_since_last_check_ = false;
};

class Riscv64VmICacheConsistencyManager final : public ArchVmICacheConsistencyManagerInterface {
 public:
  Riscv64VmICacheConsistencyManager() = default;
  ~Riscv64VmICacheConsistencyManager() override { Finish(); }

  void SyncAddr(vaddr_t start, size_t len) override;
  void Finish() override;

 private:
  bool need_invalidate_ = false;
};

using ArchVmAspace = Riscv64ArchVmAspace;
using ArchVmICacheConsistencyManager = Riscv64VmICacheConsistencyManager;
#endif  // ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_ASPACE_H_
