// Copyright 2016 The Fuchsia Authors
// Copyright (c) 2014 Google Inc. All rights reserved
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT
#include "arch/arm64/mmu.h"

#include <align.h>
#include <assert.h>
#include <bits.h>
#include <debug.h>
#include <inttypes.h>
#include <lib/arch/arm64/system.h>
#include <lib/arch/cache.h>
#include <lib/arch/intrin.h>
#include <lib/boot-options/boot-options.h>
#include <lib/counters.h>
#include <lib/fit/defer.h>
#include <lib/heap.h>
#include <lib/instrumentation/asan.h>
#include <lib/ktrace.h>
#include <lib/lazy_init/lazy_init.h>
#include <lib/page/size.h>
#include <lib/page_cache.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>
#include <trace.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <arch/arm64/hypervisor/el2_state.h>
#include <arch/aspace.h>
#include <kernel/auto_preempt_disabler.h>
#include <kernel/mutex.h>
#include <ktl/algorithm.h>
#include <ktl/atomic.h>
#include <ktl/span.h>
#include <lk/init.h>
#include <phys/handoff.h>
#include <platform/timer.h>
#include <vm/arch_vm_aspace.h>
#include <vm/physmap.h>
#include <vm/pmm.h>
#include <vm/vm.h>

#include "asid_allocator.h"

#include <ktl/enforce.h>

#define LOCAL_TRACE 0
#define TRACE_CONTEXT_SWITCH 0

/* ktraces just local to this file */
#define LOCAL_KTRACE_ENABLE 0

#define LOCAL_KTRACE(label, args...) \
  KTRACE_CPU_INSTANT_ENABLE(LOCAL_KTRACE_ENABLE, "kernel:probe", label, ##args)

// Use one of the ignored bits for a software simulated accessed flag for non-terminal entries.
// TODO: Once the hardware setting of the terminal AF is supported usage of this for non-terminal AF
// will have to become optional as we rely on the software terminal fault to set the non-terminal
// bits.
#define MMU_PTE_ATTR_RES_SOFTWARE_AF BM(55, 1, 1)
// Ensure we picked a bit that is actually part of the software controlled bits.
static_assert((MMU_PTE_ATTR_RES_SOFTWARE & MMU_PTE_ATTR_RES_SOFTWARE_AF) ==
              MMU_PTE_ATTR_RES_SOFTWARE_AF);

// Use one of the ignored bits to do store an odd parity flag. This is checked whenever a pte is
// read and its purpose is to help detect any stray memory corruption that impacts the page tables.
#define MMU_PTE_RES_PARITY BM(56, 1, 1)
static_assert((MMU_PTE_ATTR_RES_SOFTWARE & MMU_PTE_RES_PARITY) == MMU_PTE_RES_PARITY);

static_assert(((long)kHandoffVirtualAddress >> MMU_KERNEL_SIZE_SHIFT) == -1, "");
static_assert(((long)KERNEL_ASPACE_BASE >> MMU_KERNEL_SIZE_SHIFT) == -1, "");
static_assert(MMU_KERNEL_SIZE_SHIFT <= 48, "");
static_assert(MMU_KERNEL_SIZE_SHIFT >= 25, "");

// Physical addresses of the kernel(/upper) and lower root page tables,
// saved in start.S.
paddr_t root_kernel_page_table_phys;
paddr_t root_lower_page_table_phys;

namespace {

// If lock is contended, limit harvesting to 32 entries per iteration with the
// arch aspace lock held to avoid delays in accessed faults in the same aspace
// running in parallel.
//
// This limit is derived from the following observations:
// 1. Worst case runtime to harvest a terminal PTE on a low-end A53 is ~780ns.
// 2. Real workloads can result in harvesting thousands of terminal PTEs in a
//    single aspace.
// 3. An access fault handler will spin up to 150us on the aspace adaptive
//    mutex before blocking.
// 4. Unnecessarily blocking is costly when the system is heavily loaded,
//    especially during accessed faults, which tend to occur multiple times in
//    quick succession within and across threads in the same process.
//
// To achieve optimal contention between access harvesting and access faults,
// it is important to avoid exhausting the 150us mutex spin phase by holding
// the aspace mutex for too long. The selected entry limit results in a worst
// case harvest time of about 1/6 of the mutex spin phase.
//
//   Ti = worst case runtime per top-level harvest iteration.
//   Te = worst case runtime per terminal entry harvest.
//   L  = max entries per top-level harvest iteration.
//
//   Ti = Te * L = 780ns * 32 = 24.96us
//
constexpr size_t kHarvestEntriesBetweenUnlocks = 32;

// Whether ASID use is enabled.
bool feat_asid_enabled;

// Whether or not we allow break-before-make. Used in very early boot.
bool allow_bbm = false;

KCOUNTER(cm_flush_all, "mmu.consistency_manager.flush_all")
KCOUNTER(cm_flush_all_replacing, "mmu.consistency_manager.flush_all_replacing")
KCOUNTER(cm_single_tlb_invalidates, "mmu.consistency_manager.single_tlb_invalidate")
KCOUNTER(cm_flush, "mmu.consistency_manager.flush")

lazy_init::LazyInit<AsidAllocator> asid;

KCOUNTER(vm_mmu_protect_make_execute_calls, "vm.mmu.protect.make_execute_calls")
KCOUNTER(vm_mmu_protect_make_execute_pages, "vm.mmu.protect.make_execute_pages")
KCOUNTER(vm_mmu_page_table_alloc, "vm.mmu.pt.alloc")
KCOUNTER(vm_mmu_page_table_free, "vm.mmu.pt.free")
KCOUNTER(vm_mmu_page_table_reclaim, "vm.mmu.pt.reclaim")
KCOUNTER(vm_mmu_set_af_fail, "vm.mmu.set_af_fail")

page_cache::PageCache page_cache;

zx::result<vm_page_t*> CacheAllocPage() {
  if (!page_cache) {
    return Pmm::Node().AllocPage(PMM_ALLOC_FLAG_ANY);
  }

  zx::result result = page_cache.Allocate(1);
  if (result.is_error()) {
    return result.take_error();
  }

  vm_page_t* page = result->page_list.pop_front();
  DEBUG_ASSERT(page != nullptr);
  DEBUG_ASSERT(result->page_list.is_empty());
  return zx::ok(page);
}

void CacheFreePages(VmPageDoublyLinkedList* list) {
  if (!page_cache) {
    pmm_free(list);
  } else {
    page_cache.Free(ktl::move(*list));
  }
}

void CacheFreePage(vm_page_t* p) {
  if (!page_cache) {
    pmm_free_page(p);
  } else {
    page_cache::PageCache::PageList list;
    list.push_back(p);

    page_cache.Free(ktl::move(list));
  }
}

void InitializePageCache(uint32_t level) {
  ASSERT(level < LK_INIT_LEVEL_THREADING);

  const size_t reserve_pages = 8;
  zx::result<page_cache::PageCache> result = page_cache::PageCache::Create(reserve_pages);

  ASSERT(result.is_ok());
  page_cache = ktl::move(result.value());
}

// Initialize the cache after the percpu data structures are initialized.
LK_INIT_HOOK(arm64_mmu_page_cache_init, InitializePageCache, LK_INIT_LEVEL_KERNEL)

void enable_bbm(uint32_t level) {
  dprintf(INFO, "ARM: enabling break-before-make\n");
  allow_bbm = true;
}

// Enable break-before-make when splitting large pages after the VM has been initialized
// which is where a bunch of pieces of the physmap and kernel are unmapped or permissions
// lowered.
LK_INIT_HOOK(arm64_mmu_enable_bbm, enable_bbm, LK_INIT_LEVEL_VM)

// Calculates the parity of the given pte. Return value of true indicates it has odd parity, which
// is the goal.
constexpr bool compute_parity(pte_t pte) { return (ktl::popcount(pte) % 2) == 0; }

constexpr pte_t add_parity(pte_t pte) {
  if (!compute_parity(pte)) {
    pte ^= MMU_PTE_RES_PARITY;
  }
  return pte;
}

inline void check_parity(pte_t pte) {
  ASSERT_MSG(compute_parity(pte), "PTE parity error: %#" PRIx64, pte);
}

inline pte_t load_pte(pte_t* pte, ktl::memory_order order) {
  pte_t val = ktl::atomic_ref(*pte).load(order);
  check_parity(val);
  return val;
}

inline bool compare_exchange_pte(pte_t* pte, pte_t& expected, pte_t desired,
                                 ktl::memory_order order) {
  expected = add_parity(expected);
  desired = add_parity(desired);
  bool success = ktl::atomic_ref(*pte).compare_exchange_strong(expected, desired, order);
  // If the exchange failed, expected will have been updated and so we should check its parity.
  if (!success) {
    check_parity(expected);
  }
  return success;
}

// Convert user level mmu flags to flags that go in L1 descriptors.
// Hypervisor flag modifies behavior to work for single translation regimes
// such as the mapping of kernel pages with ArmAspaceType::kHypervisor in EL2.
pte_t mmu_flags_to_s1_pte_attr(arch_mmu_flags_t flags, bool hypervisor = false) {
  pte_t attr = MMU_PTE_ATTR_AF;

  switch (flags & ARCH_MMU_FLAG_CACHE_MASK) {
    case ARCH_MMU_FLAG_CACHED:
      attr |= MMU_PTE_ATTR_NORMAL_MEMORY | MMU_PTE_ATTR_SH_INNER_SHAREABLE;
      break;
    case ARCH_MMU_FLAG_WRITE_COMBINING:
      attr |= MMU_PTE_ATTR_NORMAL_UNCACHED | MMU_PTE_ATTR_SH_INNER_SHAREABLE;
      break;
    case ARCH_MMU_FLAG_UNCACHED:
      attr |= MMU_PTE_ATTR_STRONGLY_ORDERED;
      break;
    case ARCH_MMU_FLAG_UNCACHED_DEVICE:
      attr |= MMU_PTE_ATTR_DEVICE;
      break;
    default:
      panic("unexpected flags value 0x%x", flags);
  }

  switch (flags & (ARCH_MMU_FLAG_PERM_USER | ARCH_MMU_FLAG_PERM_WRITE)) {
    case 0:
      attr |= MMU_PTE_ATTR_AP_P_RO_U_NA;
      break;
    case ARCH_MMU_FLAG_PERM_WRITE:
      attr |= MMU_PTE_ATTR_AP_P_RW_U_NA;
      break;
    case ARCH_MMU_FLAG_PERM_USER:
      attr |= MMU_PTE_ATTR_AP_P_RO_U_RO;
      break;
    case ARCH_MMU_FLAG_PERM_USER | ARCH_MMU_FLAG_PERM_WRITE:
      attr |= MMU_PTE_ATTR_AP_P_RW_U_RW;
      break;
  }

  if (hypervisor) {
    // For single translation regimes such as the hypervisor pages, only
    // the XN bit applies.
    if ((flags & ARCH_MMU_FLAG_PERM_EXECUTE) == 0) {
      attr |= MMU_PTE_ATTR_XN;
    }
  } else {
    if (flags & ARCH_MMU_FLAG_PERM_EXECUTE) {
      if (flags & ARCH_MMU_FLAG_PERM_USER) {
        // User executable page, marked privileged execute never.
        attr |= MMU_PTE_ATTR_PXN;
      } else {
        // Privileged executable page, marked user execute never.
        attr |= MMU_PTE_ATTR_UXN;
      }
    } else {
      // All non executable pages are marked both privileged and user execute never.
      attr |= MMU_PTE_ATTR_UXN | MMU_PTE_ATTR_PXN;
    }
  }

  if (flags & ARCH_MMU_FLAG_NS) {
    attr |= MMU_PTE_ATTR_NON_SECURE;
  }

  return attr;
}

arch_mmu_flags_t s1_pte_attr_to_mmu_flags(pte_t pte, bool hypervisor = false) {
  arch_mmu_flags_t mmu_flags = 0;
  switch (pte & MMU_PTE_ATTR_ATTR_INDEX_MASK) {
    case MMU_PTE_ATTR_STRONGLY_ORDERED:
      mmu_flags |= ARCH_MMU_FLAG_UNCACHED;
      break;
    case MMU_PTE_ATTR_DEVICE:
      mmu_flags |= ARCH_MMU_FLAG_UNCACHED_DEVICE;
      break;
    case MMU_PTE_ATTR_NORMAL_UNCACHED:
      mmu_flags |= ARCH_MMU_FLAG_WRITE_COMBINING;
      break;
    case MMU_PTE_ATTR_NORMAL_MEMORY:
      mmu_flags |= ARCH_MMU_FLAG_CACHED;
      break;
    default:
      panic("unexpected pte value %" PRIx64, pte);
  }

  mmu_flags |= ARCH_MMU_FLAG_PERM_READ;
  switch (pte & MMU_PTE_ATTR_AP_MASK) {
    case MMU_PTE_ATTR_AP_P_RW_U_NA:
      mmu_flags |= ARCH_MMU_FLAG_PERM_WRITE;
      break;
    case MMU_PTE_ATTR_AP_P_RW_U_RW:
      mmu_flags |= ARCH_MMU_FLAG_PERM_USER | ARCH_MMU_FLAG_PERM_WRITE;
      break;
    case MMU_PTE_ATTR_AP_P_RO_U_NA:
      break;
    case MMU_PTE_ATTR_AP_P_RO_U_RO:
      mmu_flags |= ARCH_MMU_FLAG_PERM_USER;
      break;
  }

  if (hypervisor) {
    // Single translation regimes such as the hypervisor only support the XN bit.
    if ((pte & MMU_PTE_ATTR_XN) == 0) {
      mmu_flags |= ARCH_MMU_FLAG_PERM_EXECUTE;
    }
  } else {
    // Based on whether or not this is a user page, check UXN or PXN bit to determine
    // if it's an executable page.
    if (mmu_flags & ARCH_MMU_FLAG_PERM_USER) {
      if ((pte & MMU_PTE_ATTR_UXN) == 0) {
        mmu_flags |= ARCH_MMU_FLAG_PERM_EXECUTE;
      }
    } else if ((pte & MMU_PTE_ATTR_PXN) == 0) {
      // Privileged page, check the PXN bit.
      mmu_flags |= ARCH_MMU_FLAG_PERM_EXECUTE;
    }

    // TODO: https://fxbug.dev/42169684
    // Add additional asserts here that the translation table entries are correctly formed
    // with regards to UXN and PXN bits and possibly other unhandled and/or ambiguous bits.
  }

  if (pte & MMU_PTE_ATTR_NON_SECURE) {
    mmu_flags |= ARCH_MMU_FLAG_NS;
  }

  return mmu_flags;
}

pte_t mmu_flags_to_s2_pte_attr(arch_mmu_flags_t flags) {
  pte_t attr = MMU_PTE_ATTR_AF;

  switch (flags & ARCH_MMU_FLAG_CACHE_MASK) {
    case ARCH_MMU_FLAG_CACHED:
      attr |= MMU_S2_PTE_ATTR_NORMAL_MEMORY | MMU_PTE_ATTR_SH_INNER_SHAREABLE;
      break;
    case ARCH_MMU_FLAG_WRITE_COMBINING:
      attr |= MMU_S2_PTE_ATTR_NORMAL_UNCACHED | MMU_PTE_ATTR_SH_INNER_SHAREABLE;
      break;
    case ARCH_MMU_FLAG_UNCACHED:
      attr |= MMU_S2_PTE_ATTR_STRONGLY_ORDERED;
      break;
    case ARCH_MMU_FLAG_UNCACHED_DEVICE:
      attr |= MMU_S2_PTE_ATTR_DEVICE;
      break;
    default:
      panic("unexpected flags value 0x%x", flags);
  }

  if (flags & ARCH_MMU_FLAG_PERM_WRITE) {
    attr |= MMU_S2_PTE_ATTR_S2AP_RW;
  } else {
    attr |= MMU_S2_PTE_ATTR_S2AP_RO;
  }
  if (!(flags & ARCH_MMU_FLAG_PERM_EXECUTE)) {
    attr |= MMU_S2_PTE_ATTR_XN;
  }

  return attr;
}

arch_mmu_flags_t s2_pte_attr_to_mmu_flags(pte_t pte) {
  arch_mmu_flags_t mmu_flags = 0;

  switch (pte & MMU_S2_PTE_ATTR_ATTR_INDEX_MASK) {
    case MMU_S2_PTE_ATTR_STRONGLY_ORDERED:
      mmu_flags |= ARCH_MMU_FLAG_UNCACHED;
      break;
    case MMU_S2_PTE_ATTR_DEVICE:
      mmu_flags |= ARCH_MMU_FLAG_UNCACHED_DEVICE;
      break;
    case MMU_S2_PTE_ATTR_NORMAL_UNCACHED:
      mmu_flags |= ARCH_MMU_FLAG_WRITE_COMBINING;
      break;
    case MMU_S2_PTE_ATTR_NORMAL_MEMORY:
      mmu_flags |= ARCH_MMU_FLAG_CACHED;
      break;
    default:
      panic("unexpected pte value %" PRIx64, pte);
  }

  mmu_flags |= ARCH_MMU_FLAG_PERM_READ;
  switch (pte & MMU_PTE_ATTR_AP_MASK) {
    case MMU_S2_PTE_ATTR_S2AP_RO:
      break;
    case MMU_S2_PTE_ATTR_S2AP_RW:
      mmu_flags |= ARCH_MMU_FLAG_PERM_WRITE;
      break;
    default:
      panic("unexpected pte value %" PRIx64, pte);
  }

  if (!(pte & MMU_S2_PTE_ATTR_XN)) {
    mmu_flags |= ARCH_MMU_FLAG_PERM_EXECUTE;
  }

  return mmu_flags;
}

bool is_pte_valid(pte_t pte) {
  return (pte & MMU_PTE_DESCRIPTOR_MASK) != MMU_PTE_DESCRIPTOR_INVALID;
}

void update_pte(pte_t* pte, pte_t newval, ktl::memory_order order = ktl::memory_order_relaxed) {
  ktl::atomic_ref(*pte).store(add_parity(newval), order);
}

int first_used_page_table_entry(pte_t* page_table, uint page_size_shift) {
  const unsigned int count = 1U << (page_size_shift - 3);

  for (unsigned int i = 0; i < count; i++) {
    pte_t pte = load_pte(&page_table[i], ktl::memory_order_relaxed);
    if (pte != MMU_PTE_DESCRIPTOR_INVALID) {
      // Although the descriptor isn't exactly the INVALID value, it might have been corrupted and
      // also not a valid entry. Some forms of corruption are indistinguishable from valid entries,
      // so this is really just checking for scenarios where the low type bits got set to INVALID,
      // but the rest of the entry did not.
      //
      // TODO(https://fxbug.dev/42159319): Once https://fxbug.dev/42159319 is resolved this can be
      // removed.
      ASSERT_MSG(is_pte_valid(pte),
                 "page_table at %p has malformed invalid entry %#" PRIx64 " at %u\n", page_table,
                 pte, i);
      return i;
    }
  }
  return -1;
}

ArmAspaceType AspaceTypeFromFlags(arch_mmu_flags_t mmu_flags) {
  // Kernel/Guest flags are mutually exclusive. Ensure at most 1 is set.
  DEBUG_ASSERT(((mmu_flags & ARCH_ASPACE_FLAG_KERNEL) != 0) +
                   ((mmu_flags & ARCH_ASPACE_FLAG_GUEST) != 0) <=
               1);
  if (mmu_flags & ARCH_ASPACE_FLAG_KERNEL) {
    return ArmAspaceType::kKernel;
  }
  if (mmu_flags & ARCH_ASPACE_FLAG_GUEST) {
    return ArmAspaceType::kGuest;
  }
  return ArmAspaceType::kUser;
}

ktl::string_view ArmAspaceTypeName(ArmAspaceType type) {
  switch (type) {
    case ArmAspaceType::kKernel:
      return "kernel";
    case ArmAspaceType::kUser:
      return "user";
    case ArmAspaceType::kGuest:
      return "guest";
    case ArmAspaceType::kHypervisor:
      return "hypervisor";
  }
  __UNREACHABLE;
}

}  // namespace

// A consistency manager that tracks TLB updates, walker syncs and free pages in an effort to
// minimize DSBs (by delaying and coalescing TLB invalidations) and switching to full ASID
// invalidations if too many TLB invalidations are requested.
// The aspace lock *must* be held over the full operation of the ConsistencyManager, from
// construction to deletion. The lock must be held continuously to deletion, and specifically till
// the actual TLB invalidations occur, due to strategy employed here of only invalidating actual
// vaddrs with changing entries, and not all vaddrs an operation applies to. Otherwise the following
// scenario is possible
//  1. Thread 1 performs an Unmap and removes PTE entries, but drops the lock prior to invalidation.
//  2. Thread 2 performs an Unmap, no PTE entries are removed, no invalidations occur
//  3. Thread 2 now believes the resources (pages) for the region are no longer accessible, and
//     returns them to the pmm.
//  4. Thread 3 attempts to access this region and is now able to read/write to returned pages as
//     invalidations have not occurred.
// This scenario is possible as the mappings here are not the source of truth of resource
// management, but a cache of information from other parts of the system. If thread 2 wanted to
// guarantee that the pages were free it could issue it's own TLB invalidations for the vaddr range,
// even though it found no entries. However this is not the strategy employed here at the moment.
class ArmArchVmAspace::ConsistencyManager {
 public:
  ConsistencyManager(ArmArchVmAspace& aspace) TA_REQ(aspace.lock_) : aspace_(aspace) {}
  ~ConsistencyManager() {
    Flush();

    if (!to_free_.is_empty()) {
      // Any page tables in the to_free_ list have been disconnected from tt_virt_ and are no longer
      // discoverable. Synchronize to ensure any previous readers have finished before freeing the
      // pages.

      // TODO(https://fxbug.dev/502004224): We should remove/revisit this logging when we have a
      // better understanding of the referenced issue.
      const zx_instant_mono_t start = current_mono_time();

      aspace_.rcu_.Synchronize();

      const zx_duration_mono_t duration = current_mono_time() - start;
      if (duration > ZX_SEC(1)) {
        dprintf(ALWAYS,
                "[MMU_STALL_WARN] Aspace %p: rcu_.Synchronize() took %ld ms (spinning too long!)\n",
                &aspace_, duration / ZX_MSEC(1));
      }

      CacheFreePages(&to_free_);
    }
  }

  void MapEntry(vaddr_t va, bool terminal) {
    // We do not need to sync the walker, despite writing a new entry, as this is a
    // non-terminal entry and so is irrelevant to the walker anyway.
    if (!terminal) {
      return;
    }

    // If we're mapping in the kernel aspace we may access the page shortly. DSB to make sure the
    // page table walker sees it and ISB to keep the cpu from prefetching through this point.
    // We do not need to do this for user pages since there will be a synchronization event before
    // returning back to user space, or in the case of performing a user_copy after this mapping
    // to the newly mapped page at worst there will be an extraneous page fault.
    if (aspace_.type_ == ArmAspaceType::kKernel) {
      __dsb(ARM_MB_ISHST);
      isb_pending_ = true;
    }
  }

  // Queue a TLB entry for flushing. This may get turned into a complete ASID flush, or even a
  // complete TLB (all ASID) flush if the associated aspace is a shared one.
  void FlushEntry(vaddr_t va, bool terminal) {
    // Check we have queued too many entries already.
    if (num_pending_tlbs_ >= kMaxPendingTlbs) {
      // Most of the time we will now prefer to invalidate the entire ASID, the exception is if
      // this aspace is using the global ASID, since we cannot perform a global TLB invalidation
      // for all ASIDs. Note that there is an instruction to invalidate the entire TLB, but it is
      // only available in EL2, and we are in EL1.
      if (aspace_.asid_ != MMU_ARM64_GLOBAL_ASID) {
        // Keep counting entries so that we can track how many TLB invalidates we saved by grouping.
        num_pending_tlbs_++;
        return;
      }
      // Flush what pages we've cached up until now and reset counter to zero.
      Flush();
    }

    // va must be page aligned so we can safely throw away the bottom bit.
    DEBUG_ASSERT(IsPageRounded(va));
    DEBUG_ASSERT(aspace_.IsValidVaddr(va));

    pending_tlbs_[num_pending_tlbs_++] = {va, terminal};
  }

  // Performs any pending synchronization of TLBs and page table walkers. Includes the DSB to ensure
  // TLB flushes have completed prior to returning to user.
  void Flush() {
    cm_flush.Add(1);

    // Flush any pending ISBs.
    if (isb_pending_) {
      __isb(ARM_MB_SY);
      isb_pending_ = false;
    }

    if (num_pending_tlbs_ == 0) {
      return;
    }
    // Need a DSB to synchronize any page table updates prior to flushing the TLBs.
    __dsb(ARM_MB_ISHST);

    AssertHeld(aspace_.lock_);
    // Check if we should just be performing a full ASID invalidation.
    // If the associate aspace is shared, this will be upgraded to a full TLB invalidation across
    // all ASIDs.
    if (num_pending_tlbs_ > kMaxPendingTlbs || aspace_.type_ == ArmAspaceType::kHypervisor) {
      cm_flush_all.Add(1);
      cm_flush_all_replacing.Add(num_pending_tlbs_);
      // If we're a shared aspace, we should be invalidating across all ASIDs.
      if (aspace_.IsShared()) {
        aspace_.FlushAllAsids();
      } else {
        aspace_.FlushAsid();
      }
    } else {
      for (size_t i = 0; i < num_pending_tlbs_; i++) {
        const vaddr_t va = pending_tlbs_[i].va();
        DEBUG_ASSERT(aspace_.IsValidVaddr(va));
        aspace_.FlushTLBEntry(va, pending_tlbs_[i].terminal());
      }
      cm_single_tlb_invalidates.Add(num_pending_tlbs_);
    }

    // DSB to ensure TLB flushes happen prior to returning to user.
    __dsb(ARM_MB_ISH);

    // Local flushes that the kernel may observe prior to Context Synchronization Event
    // should go ahead and get an ISB to force it.
    if (aspace_.type_ == ArmAspaceType::kKernel) {
      __isb(ARM_MB_SY);
    }

    num_pending_tlbs_ = 0;
  }

  // Queue a page for freeing that is dependent on TLB flushing. This is for pages that were
  // previously installed as page tables and they should not be reused until the non-terminal TLB
  // flush has occurred.
  void FreePage(vm_page_t* page) { to_free_.push_back(page); }

  Lock<CriticalMutex>* lock() const TA_RET_CAP(aspace_.lock_) { return &aspace_.lock_; }
  Lock<CriticalMutex>& lock_ref() const TA_RET_CAP(aspace_.lock_) { return aspace_.lock_; }

 private:
  // Maximum number of TLB entries we will queue before switching to ASID invalidation.
  static constexpr size_t kMaxPendingTlbs = 16;

  // Pending TLBs to flush are stored as 63 bits, with the bottom bit stolen to store the terminal
  // flag. 63 bits is more than enough as these entries are page aligned at the minimum.
  struct PendingTlbs {
    PendingTlbs() = default;
    PendingTlbs(uint64_t va, bool terminal) : va_terminal_(va | terminal) {}

    bool terminal() const { return va_terminal_ & 1; }
    uint64_t va() const { return va_terminal_ & ~1UL; }

   private:
    // address[63:1], terminal[0]
    uint64_t va_terminal_;
  };

  static_assert(sizeof(PendingTlbs) == 8);

  // The aspace we are invalidating TLBs for.
  ArmArchVmAspace& aspace_;

  // vm_page_t's to release to the PMM after the TLB invalidation occurs.
  VmPageDoublyLinkedList to_free_;

  // Pending ISB
  bool isb_pending_ = false;

  // The main list of pending TLBs.
  uint32_t num_pending_tlbs_ = 0;
  PendingTlbs pending_tlbs_[kMaxPendingTlbs];
};

uint64_t ArmArchVmAspace::Tcr() const {
  if (IsRestricted()) {
    return MMU_TCR_FLAGS_USER_RESTRICTED;
  }
  return MMU_TCR_FLAGS_USER;
}

arch_mmu_flags_t ArmArchVmAspace::MmuFlagsFromPte(pte_t pte) {
  switch (type_) {
    case ArmAspaceType::kUser:
    case ArmAspaceType::kKernel:
      return s1_pte_attr_to_mmu_flags(pte);
    case ArmAspaceType::kHypervisor:
      return s1_pte_attr_to_mmu_flags(pte, true);
    case ArmAspaceType::kGuest:
      return s2_pte_attr_to_mmu_flags(pte);
  }
  __UNREACHABLE;
}

zx_status_t ArmArchVmAspace::Query(vaddr_t vaddr, paddr_t* paddr, arch_mmu_flags_t* mmu_flags) {
  Guard<CriticalMutex> al{&lock_};
  return QueryLocked(vaddr, paddr, mmu_flags);
}

zx_status_t ArmArchVmAspace::QueryLocked(vaddr_t vaddr, paddr_t* paddr,
                                         arch_mmu_flags_t* mmu_flags) {
  vaddr_t vaddr_rem;

  canary_.Assert();
  LTRACEF("aspace %p, vaddr 0x%lx\n", this, vaddr);

  DEBUG_ASSERT(tt_virt_);

  DEBUG_ASSERT(IsValidVaddr(vaddr));
  if (!IsValidVaddr(vaddr)) {
    return ZX_ERR_OUT_OF_RANGE;
  }

  pte_t* page_table = tt_virt_;
  uint32_t index_shift = top_index_shift_;
  vaddr_rem = vaddr - vaddr_base_;
  while (true) {
    const ulong index = vaddr_rem >> index_shift;
    vaddr_rem -= (vaddr_t)index << index_shift;
    const pte_t pte = load_pte(&page_table[index], ktl::memory_order_relaxed);
    const uint descriptor_type = pte & MMU_PTE_DESCRIPTOR_MASK;
    const paddr_t pte_addr = pte & MMU_PTE_OUTPUT_ADDR_MASK;

    LTRACEF("va %#" PRIxPTR ", index %lu, index_shift %u, rem %#" PRIxPTR ", pte %#" PRIx64 "\n",
            vaddr, index, index_shift, vaddr_rem, pte);

    if ((pte & MMU_PTE_VALID) == 0) {
      ASSERT_MSG(pte == 0, "invalid pte should be zero %#" PRIx64 "\n", pte);
      return ZX_ERR_NOT_FOUND;
    }

    if (descriptor_type == ((index_shift > page_size_shift_) ? MMU_PTE_L012_DESCRIPTOR_BLOCK
                                                             : MMU_PTE_L3_DESCRIPTOR_PAGE)) {
      if (paddr) {
        *paddr = pte_addr + vaddr_rem;
      }
      if (mmu_flags) {
        *mmu_flags = MmuFlagsFromPte(pte);
      }
      LTRACEF("va 0x%lx, paddr 0x%lx, flags 0x%x\n", vaddr, paddr ? *paddr : ~0UL,
              mmu_flags ? *mmu_flags : ~0U);
      return ZX_OK;
    }

    ASSERT_MSG(index_shift > page_size_shift_ && descriptor_type == MMU_PTE_L012_DESCRIPTOR_TABLE,
               "index_shift %u, page_size_shift %u, descriptor_type %#x", index_shift,
               page_size_shift_, descriptor_type);

    page_table = static_cast<pte_t*>(paddr_to_physmap(pte_addr));
    index_shift -= page_size_shift_ - 3;
  }
}

zx::result<vm_page_t*> ArmArchVmAspace::AllocPageTable() {
  LTRACEF("page_size_shift %u\n", page_size_shift_);

  // currently we only support allocating a single page
  DEBUG_ASSERT(page_size_shift_ == kPageShift);

  auto test_alloc = [&]() -> zx::result<vm_page_t*> {
    vm_page_t* page;
    paddr_t paddr;
    zx_status_t status = test_page_alloc_func_(0, &page, &paddr);
    if (status == ZX_OK) {
      return zx::ok(page);
    }
    return zx::error(status);
  };

  // Allocate a page from the pmm via function pointer passed to us in Init().
  // The default is CacheAllocPage so test and explicitly call it to avoid any unnecessary
  // virtual functions.
  auto result = likely(!test_page_alloc_func_) ? CacheAllocPage() : test_alloc();
  if (likely(result.is_ok())) {
    (*result)->set_state(vm_page_state::MMU);
    pt_pages_++;
    kcounter_add(vm_mmu_page_table_alloc, 1);
    (*result)->mmu.num_mappings = 0;

    LOCAL_KTRACE("page table alloc");

    LTRACEF("allocated %#lx\n", (*result)->paddr());
  }
  return result;
}

void ArmArchVmAspace::FreePageTable(void* vaddr, vm_page_t* page, ConsistencyManager& cm,
                                    Reclaim reclaim) {
  ASSERT(page);
  LTRACEF("vaddr %p paddr %#lx page_size_shift %u\n", vaddr, page->paddr(), page_size_shift_);

  // currently we only support freeing a single page
  DEBUG_ASSERT(page_size_shift_ == kPageShift);

  LOCAL_KTRACE("page table free");

  DEBUG_ASSERT(page->state() == vm_page_state::MMU);
  DEBUG_ASSERT(page->mmu.num_mappings == 0);
  cm.FreePage(page);

  pt_pages_--;
  kcounter_add(vm_mmu_page_table_free, 1);
  if (reclaim == Reclaim::Yes) {
    kcounter_add(vm_mmu_page_table_reclaim, 1);
  }
}

zx_status_t ArmArchVmAspace::SplitLargePage(vaddr_t vaddr, const uint index_shift, vaddr_t pt_index,
                                            pte_t* page_table, ConsistencyManager& cm) {
  DEBUG_ASSERT(index_shift > page_size_shift_);

  const pte_t pte = load_pte(&page_table[pt_index], ktl::memory_order_relaxed);
  DEBUG_ASSERT((pte & MMU_PTE_DESCRIPTOR_MASK) == MMU_PTE_L012_DESCRIPTOR_BLOCK);

  auto result = AllocPageTable();
  if (result.is_error()) {
    TRACEF("failed to allocate page table\n");
    return result.status_value();
  }
  vm_page_t* page = *result;

  const uint next_shift = (index_shift - (page_size_shift_ - 3));

  const auto new_page_table = static_cast<pte_t*>(paddr_to_physmap(page->paddr()));
  const auto new_desc_type =
      (next_shift == page_size_shift_) ? MMU_PTE_L3_DESCRIPTOR_PAGE : MMU_PTE_L012_DESCRIPTOR_BLOCK;
  const auto attrs = (pte & ~(MMU_PTE_OUTPUT_ADDR_MASK | MMU_PTE_DESCRIPTOR_MASK)) | new_desc_type;

  const uint next_size = 1U << next_shift;
  for (uint64_t i = 0, mapped_paddr = pte & MMU_PTE_OUTPUT_ADDR_MASK;
       i < MMU_KERNEL_PAGE_TABLE_ENTRIES; i++, mapped_paddr += next_size) {
    // directly write to the pte, no need to update since this is
    // a completely new table
    new_page_table[i] = add_parity(mapped_paddr | attrs);
  }
  page->mmu.num_mappings = MMU_KERNEL_PAGE_TABLE_ENTRIES;

  if (allow_bbm) {
    // As we are changing the block size of a translation we must do a break-before-make in
    // accordance with ARM requirements to avoid TLB and other inconsistency.
    update_pte(&page_table[pt_index], MMU_PTE_DESCRIPTOR_INVALID);
    cm.FlushEntry(vaddr, true);
    AssertHeld(cm.lock_ref());
    // Must force the flush to happen now before installing the new entry. This will also ensure the
    // page table entries we wrote will be visible before we install it.
    cm.Flush();
  }

  update_pte(&page_table[pt_index], page->paddr() | MMU_PTE_L012_DESCRIPTOR_TABLE);
  LTRACEF("pte %p[%#" PRIxPTR "] = %#" PRIx64 "\n", page_table, pt_index,
          load_pte(&page_table[pt_index], ktl::memory_order_relaxed));

  // no need to update the page table count here since we're replacing a block entry with a table
  // entry.

  cm.FlushEntry(vaddr, false);

  return ZX_OK;
}

void ArmArchVmAspace::FlushTLBEntryForAllAsids(vaddr_t vaddr, bool terminal) const {
  if (terminal) {
    ARM64_TLBI(vaale1is, (vaddr >> 12) & TLBI_VADDR_MASK);
  } else {
    ARM64_TLBI(vaae1is, (vaddr >> 12) & TLBI_VADDR_MASK);
  }
}

// use the appropriate TLB flush instruction to globally flush the modified entry
// terminal is set when flushing at the final level of the page table.
void ArmArchVmAspace::FlushTLBEntry(vaddr_t vaddr, bool terminal) const {
  switch (type_) {
    case ArmAspaceType::kUser: {
      if (IsShared()) {
        // If this is a shared aspace, we need to flush this address for all ASIDs.
        FlushTLBEntryForAllAsids(vaddr, terminal);
      } else {
        // Otherwise, flush this address for the specific ASID.
        if (terminal) {
          ARM64_TLBI(vale1is, ((vaddr >> 12) & TLBI_VADDR_MASK) | (vaddr_t)asid_ << 48);
        } else {
          ARM64_TLBI(vae1is, ((vaddr >> 12) & TLBI_VADDR_MASK) | (vaddr_t)asid_ << 48);
        }
      }
      return;
    }
    case ArmAspaceType::kKernel: {
      DEBUG_ASSERT(asid_ == MMU_ARM64_GLOBAL_ASID);
      FlushTLBEntryForAllAsids(vaddr, terminal);
      return;
    }
    case ArmAspaceType::kGuest: {
      uint64_t vttbr = arm64_vttbr(asid_, tt_phys_);
      [[maybe_unused]] zx_status_t status = arm64_el2_tlbi_ipa(vttbr, vaddr, terminal);
      DEBUG_ASSERT(status == ZX_OK);
      return;
    }
    case ArmAspaceType::kHypervisor:
      PANIC("Unsupported.");
      return;
  }
  __UNREACHABLE;
}

void ArmArchVmAspace::FlushAllAsids() const {
  DEBUG_ASSERT(type_ == ArmAspaceType::kUser);
  DEBUG_ASSERT(IsShared());
  ARM64_TLBI_NOADDR(vmalle1is);
}

void ArmArchVmAspace::FlushAsid() const {
  switch (type_) {
    case ArmAspaceType::kUser: {
      DEBUG_ASSERT(asid_ != MMU_ARM64_GLOBAL_ASID);
      ARM64_TLBI_ASID(aside1is, asid_);
      return;
    }
    case ArmAspaceType::kKernel: {
      // The alle1is instruction that invalidates the TLBs for all ASIDs is only available in EL2,
      // and not EL1.
      panic("FlushAsid not available for kernel address space");
      return;
    }
    case ArmAspaceType::kGuest: {
      uint64_t vttbr = arm64_vttbr(asid_, tt_phys_);
      zx_status_t status = arm64_el2_tlbi_vmid(vttbr);
      DEBUG_ASSERT(status == ZX_OK);
      return;
    }
    case ArmAspaceType::kHypervisor: {
      // Flush all TLB entries in EL2.
      zx_status_t status = arm64_el2_tlbi_el2();
      DEBUG_ASSERT(status == ZX_OK);
      return;
    }
  }
  __UNREACHABLE;
}

using ArchUnmapOptions = ArchVmAspaceInterface::ArchUnmapOptions;

ktl::pair<zx_status_t, uint> ArmArchVmAspace::UnmapPageTable(
    VirtualAddressCursor& cursor, ArchUnmapOptions unmap_options, CheckForEmptyPt pt_check,
    const uint index_shift, pte_t* page_table, ConsistencyManager& cm, Reclaim reclaim) {
  const vaddr_t block_size = 1UL << index_shift;
  const uint num_entries = (1u << (page_size_shift_ - 3));
  const uint64_t index_mask = num_entries - 1;
  uint index = static_cast<uint>((cursor.vaddr_rel() >> index_shift) & index_mask);
  uint unmapped = 0;

  for (; index != num_entries && cursor.size() != 0; ++index) {
    pte_t pte = load_pte(&page_table[index], ktl::memory_order_relaxed);

    if ((pte & MMU_PTE_DESCRIPTOR_MASK) == MMU_PTE_DESCRIPTOR_INVALID) {
      cursor.SkipEntry(block_size);
      continue;
    }

    // Check if this is a large page and we need to split it.
    if (index_shift > page_size_shift_ &&
        (pte & MMU_PTE_DESCRIPTOR_MASK) == MMU_PTE_L012_DESCRIPTOR_BLOCK &&
        (!IS_ROUNDED(cursor.vaddr_rel(), block_size) || cursor.size() < block_size)) {
      // Splitting a large page may perform break-before-make, and during that window we will have
      // temporarily unmapped beyond our range, so make sure we are permitted to do that.
      if (!allow_bbm && !(unmap_options & ArchUnmapOptions::Enlarge)) {
        return {ZX_ERR_NOT_SUPPORTED, unmapped};
      }
      zx_status_t s = SplitLargePage(cursor.vaddr(), index_shift, index, page_table, cm);
      if (unlikely(s != ZX_OK)) {
        // If split fails, just unmap the whole thing, and let a
        // subsequent page fault clean it up.
        if (unmap_options == ArchUnmapOptions::None) {
          return {s, unmapped};
        }
        // We must unmap here, and not in the normal block below, so that we can use SkipEntry
        // instead of ConsumeVAddr on the cursor. This is necessary since the range we are having to
        // unmap is, by definition, larger than our actual target cursor, and it would be an error
        // to call ConsumeVAddr.
        update_pte(&page_table[index], MMU_PTE_DESCRIPTOR_INVALID);
        unmapped++;

        cm.FlushEntry(cursor.vaddr(), true);
        cursor.SkipEntry(block_size);
        continue;
      }
      pte = load_pte(&page_table[index], ktl::memory_order_relaxed);
    }

    if (index_shift > page_size_shift_ &&
        (pte & MMU_PTE_DESCRIPTOR_MASK) == MMU_PTE_L012_DESCRIPTOR_TABLE) {
      const paddr_t page_table_paddr = pte & MMU_PTE_OUTPUT_ADDR_MASK;
      pte_t* next_page_table = static_cast<pte_t*>(paddr_to_physmap(page_table_paddr));

      // Recurse a level but remember where we are unmapping from in case we need to do a second
      // pass to remove a PT.
      const vaddr_t unmap_vaddr = cursor.vaddr();
      auto [status, lower_unmapped] =
          UnmapPageTable(cursor, unmap_options, pt_check, index_shift - (page_size_shift_ - 3),
                         next_page_table, cm, reclaim);
      bool unmap_lower = false;
      // Regardless of success or failure we must update the mapping count. Since this involves
      // looking up the vm_page_t we take this opportunity to check if it's empty and needs
      // unmapping.
      vm_page_t* lower_page = nullptr;
      if (lower_unmapped > 0 || pt_check == CheckForEmptyPt::Yes) {
        lower_page = Pmm::Node().PaddrToPage(page_table_paddr);
        DEBUG_ASSERT(lower_page->mmu.num_mappings >= lower_unmapped);
        lower_page->mmu.num_mappings -= lower_unmapped;
        unmap_lower = lower_page->mmu.num_mappings == 0;
      }
      if (unlikely(status != ZX_OK)) {
        return {status, unmapped};
      }

      // If the unmap made the level below us empty we want to free the page table, unless in the
      // top page of an aspace with a prepopulated top page.
      if (unmap_lower && !(IsShared() && index_shift == top_index_shift_)) {
        LTRACEF("pte %p[0x%x] = 0 (was page table phys %#lx)\n", page_table, index,
                page_table_paddr);
        update_pte(&page_table[index], MMU_PTE_DESCRIPTOR_INVALID);
        unmapped++;

        // We can safely defer TLB flushing as the consistency manager will not return the backing
        // page to the PMM until after the tlb is flushed.
        cm.FlushEntry(unmap_vaddr, false);
        FreePageTable(next_page_table, lower_page, cm, reclaim);
      }
    } else {
      // Empty entries were already handled and skipped at the top of the loop
      DEBUG_ASSERT(is_pte_valid(pte));
      LTRACEF("pte %p[0x%x] = 0 (was phys %#lx)\n", page_table, index,
              page_table[index] & MMU_PTE_OUTPUT_ADDR_MASK);
      update_pte(&page_table[index], MMU_PTE_DESCRIPTOR_INVALID);

      if (!!(unmap_options & ArchUnmapOptions::Harvest)) {
        const paddr_t pte_addr = pte & MMU_PTE_OUTPUT_ADDR_MASK;
        const paddr_t paddr = pte_addr + (cursor.vaddr_rel() & (block_size - 1));
        vm_page_t* page = paddr_to_vm_page(paddr);
        if (likely(page) && (pte & MMU_PTE_ATTR_AF)) {
          Pmm::Node().GetPageQueues()->MarkAccessed(page);
        }
      }

      unmapped++;
      cm.FlushEntry(cursor.vaddr(), true);
      cursor.Consume(block_size);
    }
  }

  return {ZX_OK, unmapped};
}

ktl::pair<zx_status_t, uint> ArmArchVmAspace::MapPageTable(pte_t attrs, bool ro, uint index_shift,
                                                           pte_t* page_table,
                                                           ExistingEntryAction existing_action,
                                                           MappingCursor& cursor,
                                                           ConsistencyManager& cm) {
  const vaddr_t block_size = 1UL << index_shift;
  const uint num_entries = (1u << (page_size_shift_ - 3));
  const uint64_t index_mask = num_entries - 1;
  uint index = static_cast<uint>((cursor.vaddr_rel() >> index_shift) & index_mask);
  uint mapped = 0;

  for (; index != num_entries && cursor.size() != 0; ++index) {
    pte_t pte = load_pte(&page_table[index], ktl::memory_order_relaxed);

    // if we're at an unaligned address, not trying to map a block, and not at the terminal level,
    // recurse one more level of the page table tree
    const bool level_valigned = IS_ROUNDED(cursor.vaddr_rel(), block_size);
    const bool level_paligned = IS_ROUNDED(cursor.paddr(), block_size);
    if (!level_valigned || !level_paligned || cursor.PageRemaining() < block_size ||
        (index_shift > MMU_PTE_DESCRIPTOR_BLOCK_MAX_SHIFT)) {
      // Lookup the next level page table, allocating if required.
      paddr_t page_table_paddr = 0;
      pte_t* next_page_table = nullptr;

      switch (pte & MMU_PTE_DESCRIPTOR_MASK) {
        case MMU_PTE_DESCRIPTOR_INVALID: {
          auto result = AllocPageTable();
          if (result.is_error()) {
            TRACEF("failed to allocate page table\n");
            // The mapping wasn't fully updated, but there is work here that might need to be undone
            // as we may have allocated various levels of page tables. By consuming a single page we
            // make the cleanup operation think we have added a mapping here, causing it to check
            // the page table for potential cleanup.
            cursor.Consume(kPageSize);
            return {result.status_value(), mapped};
          }
          page_table_paddr = (*result)->paddr();

          void* pt_vaddr = paddr_to_physmap(page_table_paddr);

          LTRACEF("allocated page table, vaddr %p, paddr 0x%lx\n", pt_vaddr, page_table_paddr);
          // Odd parity means that the 0 pte entry has correct parity, so assert that here, as it
          // allows us to just zero the page.
          static_assert(add_parity(0) == 0);
          arch_zero_page(pt_vaddr);

          // ensure that the zeroing is observable from hardware page table walkers, as we need to
          // do this prior to writing the pte we cannot defer it using the consistency manager.
          __dsb(ARM_MB_ISHST);

          // When new pages are mapped they they have their AF set, under the assumption they are
          // being mapped due to being accessed, and this lets us avoid an accessed fault. Since new
          // terminal mappings start with the AF flag set, we then also need to start non-terminal
          // mappings as having the AF set.
          pte = page_table_paddr | MMU_PTE_L012_DESCRIPTOR_TABLE | MMU_PTE_ATTR_RES_SOFTWARE_AF;
          // To ensure any rcu readers see the zeroed page table use a store-release here, which is
          // paired with the load-acquire in the reader path. This is technically redundant, since
          // the dsb above that synchronized with the hardware page table walkers would have also
          // synchronized with the readers, but the explicit release here is added for clarity.
          update_pte(&page_table[index], pte, ktl::memory_order_release);
          mapped++;

          // Tell the consistency manager that we've mapped an inner node.
          cm.MapEntry(cursor.vaddr(), false);

          LTRACEF("pte %p[%u] = %#" PRIx64 "\n", page_table, index, pte);
          next_page_table = static_cast<pte_t*>(pt_vaddr);
          break;
        }
        case MMU_PTE_L012_DESCRIPTOR_TABLE:
          // Similar to creating a page table, if we end up mapping a page lower down in this
          // hierarchy then it will start off as accessed. As such we set the accessed flag on the
          // way down.
          pte |= MMU_PTE_ATTR_RES_SOFTWARE_AF;
          update_pte(&page_table[index], pte);
          page_table_paddr = pte & MMU_PTE_OUTPUT_ADDR_MASK;
          LTRACEF("found page table %#" PRIxPTR "\n", page_table_paddr);
          next_page_table = static_cast<pte_t*>(paddr_to_physmap(page_table_paddr));
          break;
        case MMU_PTE_L012_DESCRIPTOR_BLOCK:
          return {ZX_ERR_ALREADY_EXISTS, mapped};

        default:
          panic("unexpected pte value %" PRIx64, pte);
      }
      DEBUG_ASSERT(next_page_table);

      auto [ret, lower_mapped] = MapPageTable(attrs, ro, index_shift - (page_size_shift_ - 3),
                                              next_page_table, existing_action, cursor, cm);
      // Regardless of success or failure we must update the mapping counts.
      if (lower_mapped > 0) {
        vm_page_t* lower_page = Pmm::Node().PaddrToPage(page_table_paddr);
        DEBUG_ASSERT(lower_page);
        lower_page->mmu.num_mappings += lower_mapped;
      }
      if (ret != ZX_OK) {
        return {ret, mapped};
      }
    } else {
      pte_t new_pte = cursor.paddr() | attrs;
      if (index_shift > page_size_shift_) {
        new_pte |= MMU_PTE_L012_DESCRIPTOR_BLOCK;
      } else {
        new_pte |= MMU_PTE_L3_DESCRIPTOR_PAGE;
      }

      const bool valid = is_pte_valid(pte);
      if (unlikely(valid && existing_action == ExistingEntryAction::Error)) {
        return {ZX_ERR_ALREADY_EXISTS, mapped};
      } else if (valid && existing_action == ExistingEntryAction::Skip) {
        // Empty case to simplify the other branches.
      } else if (valid && existing_action == ExistingEntryAction::Upgrade &&
                 (pte & MMU_PTE_OUTPUT_ADDR_MASK) == cursor.paddr()) {
        // Doing an upgrade of an existing entry where the output address is not changing. This is
        // just a protect, which we can skip if either nothing is actually changing, or if we would
        // potentially be reducing permissions.
        if (!ro && new_pte != pte) {
          update_pte(&page_table[index], new_pte);
          cm.FlushEntry(cursor.vaddr(), true);
        }
      } else {
        if (!valid) {
          // As we are going to transition an entry form INVALID->VALID we must count this as an
          // additional mapping. All other cases are changing an entry from VALID->VALID.
          mapped++;
        }
        // Either no current entry, or we need to upgrade the existing one, potentially performing
        // a break-before-make.
        if (valid && !ro) {
          // If the output address were not changing we would have hit the protect case above, so if
          // the new entry is not read only then we must perform break-before-make before installing
          // it. Failing to do this could result in writes being temporarily lost due to the
          // different output addresses and so we must ignore the allow_bbm flag.
          update_pte(&page_table[index], MMU_PTE_DESCRIPTOR_INVALID);
          cm.FlushEntry(cursor.vaddr(), true);
          // Must force the flush to happen now before installing the new entry. This will also
          // ensure the page table entries we wrote will be visible before we install it.
          cm.Flush();
        }
        LTRACEF("pte %p[%u] = %#" PRIx64 " (paddr %#lx)\n", page_table, index, pte, cursor.paddr());
        update_pte(&page_table[index], new_pte);

        // Tell the consistency manager we've mapped a new page.
        cm.MapEntry(cursor.vaddr(), true);
      }
      cursor.Consume(block_size);
    }
  }

  return {ZX_OK, mapped};
}

zx_status_t ArmArchVmAspace::ProtectPageTable(vaddr_t vaddr_in, vaddr_t vaddr_rel_in,
                                              size_t size_in, pte_t attrs, ArchUnmapOptions enlarge,
                                              const uint index_shift, pte_t* page_table,
                                              ConsistencyManager& cm) {
  vaddr_t vaddr = vaddr_in;
  vaddr_t vaddr_rel = vaddr_rel_in;
  size_t size = size_in;

  const vaddr_t block_size = 1UL << index_shift;
  const vaddr_t block_mask = block_size - 1;

  LTRACEF("vaddr %#" PRIxPTR ", vaddr_rel %#" PRIxPTR ", size %#" PRIxPTR ", attrs %#" PRIx64
          ", index shift %u, page_size_shift %u, page_table %p\n",
          vaddr, vaddr_rel, size, attrs, index_shift, page_size_shift_, page_table);

  // vaddr_rel and size must be page aligned
  DEBUG_ASSERT(((vaddr_rel | size) & ((1UL << page_size_shift_) - 1)) == 0);

  while (size) {
    const vaddr_t vaddr_rem = vaddr_rel & block_mask;
    const size_t chunk_size = ktl::min(size, block_size - vaddr_rem);
    const vaddr_t index = vaddr_rel >> index_shift;

    pte_t pte = load_pte(&page_table[index], ktl::memory_order_relaxed);

    // If the input range partially covers a large page, split the page.
    if (index_shift > page_size_shift_ &&
        (pte & MMU_PTE_DESCRIPTOR_MASK) == MMU_PTE_L012_DESCRIPTOR_BLOCK &&
        chunk_size != block_size) {
      // Splitting a large page may perform break-before-make, and during that window we will have
      // temporarily unmapped beyond our range, so make sure that is permitted.
      if (!allow_bbm && !(enlarge & ArchUnmapOptions::Enlarge)) {
        return ZX_ERR_NOT_SUPPORTED;
      }
      zx_status_t s = SplitLargePage(vaddr, index_shift, index, page_table, cm);
      if (unlikely(s != ZX_OK)) {
        return s;
      }
      pte = load_pte(&page_table[index], ktl::memory_order_relaxed);
    }

    if (index_shift > page_size_shift_ &&
        (pte & MMU_PTE_DESCRIPTOR_MASK) == MMU_PTE_L012_DESCRIPTOR_TABLE) {
      const paddr_t page_table_paddr = pte & MMU_PTE_OUTPUT_ADDR_MASK;
      pte_t* next_page_table = static_cast<pte_t*>(paddr_to_physmap(page_table_paddr));

      // Recurse a level.
      zx_status_t status =
          ProtectPageTable(vaddr, vaddr_rem, chunk_size, attrs, enlarge,
                           index_shift - (page_size_shift_ - 3), next_page_table, cm);
      if (unlikely(status != ZX_OK)) {
        return status;
      }
    } else if (is_pte_valid(pte)) {
      const pte_t new_pte = (pte & ~MMU_PTE_PERMISSION_MASK) | attrs;
      LTRACEF("pte %p[%#" PRIxPTR "] = %#" PRIx64 " was %#" PRIx64 "\n", page_table, index, new_pte,
              pte);
      // Skip updating the page table entry if the new value is the same as before.
      if (new_pte != pte) {
        update_pte(&page_table[index], new_pte);
        cm.FlushEntry(vaddr, true);
      }
    } else {
      LTRACEF("page table entry does not exist, index %#" PRIxPTR ", %#" PRIx64 "\n", index, pte);
    }
    vaddr += chunk_size;
    vaddr_rel += chunk_size;
    size -= chunk_size;
  }
  return ZX_OK;
}

size_t ArmArchVmAspace::HarvestAccessedPageTable(size_t* entry_limit, vaddr_t vaddr,
                                                 vaddr_t vaddr_rel_in, size_t size,
                                                 const uint index_shift,
                                                 NonTerminalAction non_terminal_action,
                                                 TerminalAction terminal_action, pte_t* page_table,
                                                 ConsistencyManager& cm) {
  const vaddr_t block_size = 1UL << index_shift;
  const vaddr_t block_mask = block_size - 1;

  // We need to retain top level page table entries and their accessed information in shared
  // aspaces. Since the top level page tables are pre-allocated and the corresponding top level
  // entries copied into unified aspaces, these page tables can never be unmapped, because the
  // copied entries cannot change.
  //
  // Because of this, we cannot clear the AF bit at the top level too, otherwise we lose information
  // about whether we should recurse further down to potentially reclaim lower level page tables in
  // the next harvest. (Unmapping would have taken care of recursion if we could unmap.) The
  // alternative to this would be to always recurse down irrespective of whether the AF bit is set.
  // Experiments show that this is wasteful as majority of the top level page tables remain
  // unaccessed. Instead we choose to have the AF bit sticky at the top level in shared aspaces.
  const bool retain_shared_entries = index_shift == top_index_shift_ && IsShared();

  vaddr_t vaddr_rel = vaddr_rel_in;

  // vaddr_rel and size must be page aligned
  DEBUG_ASSERT(((vaddr_rel | size) & ((1UL << page_size_shift_) - 1)) == 0);

  size_t harvested_size = 0;

  vm_page_t* table_page = Pmm::Node().PaddrToPage(physmap_to_paddr((void*)page_table));
  while (size > 0 && *entry_limit > 0) {
    ktrace::Scope trace =
        KTRACE_BEGIN_SCOPE_ENABLE(LOCAL_KTRACE_ENABLE, "kernel:vm", "page_table_loop");

    const vaddr_t vaddr_rem = vaddr_rel & block_mask;
    const vaddr_t index = vaddr_rel >> index_shift;

    size_t chunk_size = ktl::min(size, block_size - vaddr_rem);

    pte_t pte = load_pte(&page_table[index], ktl::memory_order_relaxed);

    if (index_shift > page_size_shift_ &&
        (pte & MMU_PTE_DESCRIPTOR_MASK) == MMU_PTE_L012_DESCRIPTOR_BLOCK &&
        chunk_size != block_size) {
      // Ignore large pages, we do not support harvesting accessed bits from them. Having this empty
      // if block simplifies the overall logic.
    } else if (index_shift > page_size_shift_ &&
               (pte & MMU_PTE_DESCRIPTOR_MASK) == MMU_PTE_L012_DESCRIPTOR_TABLE) {
      const paddr_t page_table_paddr = pte & MMU_PTE_OUTPUT_ADDR_MASK;
      DEBUG_ASSERT_MSG(page_table_paddr && is_physmap_phys_addr(page_table_paddr),
                       "invalid pte %#" PRIxPTR
                       " found in page_table %p at index %lu. updates_enabled %d\n",
                       pte, page_table, index, updates_enabled_);
      pte_t* next_page_table = static_cast<pte_t*>(paddr_to_physmap(page_table_paddr));

      // Start with the assumption that we will unmap if we can.
      bool do_unmap = non_terminal_action == NonTerminalAction::FreeUnaccessed;
      // Check for our emulated non-terminal AF so we can potentially skip the recursion.
      // TODO: make this optional when hardware AF is supported (see todo on
      // MMU_PTE_ATTR_RES_SOFTWARE_AF for details)
      bool should_recurse = pte & MMU_PTE_ATTR_RES_SOFTWARE_AF;
      vm_page_t* lower_page = nullptr;
      if (should_recurse) {
        chunk_size = HarvestAccessedPageTable(
            entry_limit, vaddr, vaddr_rem, chunk_size, index_shift - (page_size_shift_ - 3),
            non_terminal_action, terminal_action, next_page_table, cm);
        // This was accessed so we don't necessarily want to unmap it, unless our recursive call
        // caused the page table to be empty, in which case we are obligated to.
        lower_page = Pmm::Node().PaddrToPage(page_table_paddr);

        do_unmap = lower_page->mmu.num_mappings == 0;

        // We cannot clear the AF bit for top level page table entries in shared aspaces.
        if (!retain_shared_entries) {
          // If we processed till the end of sub page table, and we are not retaining page tables,
          // then we can clear the AF as we know we will not have to process entries from this one
          // again.
          if (!do_unmap && (vaddr_rel + chunk_size) >> index_shift != index &&
              non_terminal_action != NonTerminalAction::Retain) {
            pte &= ~MMU_PTE_ATTR_RES_SOFTWARE_AF;
            update_pte(&page_table[index], pte);
          }
        }
      }
      // We can't unmap any top level page table entries in an address space with a prepopulated
      // top level page.
      if (retain_shared_entries) {
        do_unmap = false;
      }
      if (do_unmap) {
        // Unmapping an exact block, which should not need enlarging and hence should never be able
        // to fail.
        VirtualAddressCursor unmap_cursor(vaddr, chunk_size);
        {
          [[maybe_unused]] bool result =
              unmap_cursor.SetVaddrRelativeOffset(vaddr_base_, 1ull << top_size_shift_);
          // This should never fail as the cursor we are building is a subset of the range we have
          // already processed, which by definition must have been valid.
          DEBUG_ASSERT(result);
        }
        auto [result, lower_unmapped] =
            UnmapPageTable(unmap_cursor, ArchUnmapOptions::None, CheckForEmptyPt::No,
                           index_shift - (page_size_shift_ - 3), next_page_table, cm, Reclaim::Yes);
        ASSERT(result == ZX_OK);
        if (!lower_page) {
          lower_page = Pmm::Node().PaddrToPage(page_table_paddr);
        }
        DEBUG_ASSERT(lower_page->mmu.num_mappings == lower_unmapped);
        lower_page->mmu.num_mappings -= lower_unmapped;
        update_pte(&page_table[index], MMU_PTE_DESCRIPTOR_INVALID);
        table_page->mmu.num_mappings--;

        // We can safely defer TLB flushing as the consistency manager will not return the backing
        // page to the PMM until after the tlb is flushed.
        cm.FlushEntry(vaddr, false);
        FreePageTable(next_page_table, lower_page, cm, Reclaim::Yes);
      }
    } else if (is_pte_valid(pte) && (pte & MMU_PTE_ATTR_AF)) {
      const paddr_t pte_addr = pte & MMU_PTE_OUTPUT_ADDR_MASK;
      const paddr_t paddr = pte_addr + vaddr_rem;

      vm_page_t* page = paddr_to_vm_page(paddr);
      // Mappings for physical VMOs do not have pages associated with them and so there's no state
      // to update on an access.
      if (likely(page)) {
        Pmm::Node().GetPageQueues()->MarkAccessed(page);

        if (terminal_action == TerminalAction::UpdateAgeAndHarvest) {
          // Modifying the access flag does not require break-before-make for correctness and as we
          // do not support hardware access flag setting at the moment we do not have to deal with
          // potential concurrent modifications.
          pte = (pte & ~MMU_PTE_ATTR_AF);
          LTRACEF("pte %p[%#" PRIxPTR "] = %#" PRIx64 "\n", page_table, index, pte);
          update_pte(&page_table[index], pte);

          cm.FlushEntry(vaddr, true);
        }
      }
    }
    vaddr += chunk_size;
    vaddr_rel += chunk_size;
    size -= chunk_size;

    harvested_size += chunk_size;

    // Each iteration of this loop examines a PTE at the current level. The
    // total number of PTEs examined is limited to avoid holding the aspace lock
    // for too long. However, the remaining limit balance is updated at the end
    // of the loop to ensure that harvesting makes progress, even if the initial
    // limit is too small to reach a terminal PTE.
    if (*entry_limit > 1) {
      *entry_limit -= 1;
    } else if (!lock_.lock().IsContested()) {
      // The entry_limit is either about to be, or already is, 0, but since the lock is not
      // contended we can reset the counter and perform another block of work before checking again.
      *entry_limit = kHarvestEntriesBetweenUnlocks;
    } else {
      // This either changes the entry_limit from 1->0, or is a no-op if it was already 0. As the
      // lock is contested this ensures we'll break out back to the parent scope where the lock can
      // be dropped.
      *entry_limit = 0;
    }
  }

  return harvested_size;
}

void ArmArchVmAspace::MarkAccessedPageTable(vaddr_t vaddr, vaddr_t vaddr_rel_in, size_t size,
                                            const uint index_shift, pte_t* page_table) {
  const vaddr_t block_size = 1UL << index_shift;
  const vaddr_t block_mask = block_size - 1;

  vaddr_t vaddr_rel = vaddr_rel_in;

  // vaddr_rel and size must be page aligned
  DEBUG_ASSERT(((vaddr_rel | size) & ((1UL << page_size_shift_) - 1)) == 0);

  while (size) {
    const vaddr_t vaddr_rem = vaddr_rel & block_mask;
    const size_t chunk_size = ktl::min(size, block_size - vaddr_rem);
    const vaddr_t index = vaddr_rel >> index_shift;

    // In the case where this ends up being a reference to another table we load-acquire to ensure
    // the contents of the table reflect the order it become visible to us. This matches the
    // release performed when mapping in new page tables.
    pte_t orig_pte = load_pte(&page_table[index], ktl::memory_order_acquire);
    pte_t pte = orig_pte;

    if (index_shift > page_size_shift_ &&
        (pte & MMU_PTE_DESCRIPTOR_MASK) == MMU_PTE_L012_DESCRIPTOR_BLOCK &&
        chunk_size != block_size) {
      // Ignore large pages as we don't support modifying their access flags. Having this empty if
      // block simplifies the overall logic.
    } else if (index_shift > page_size_shift_ &&
               (pte & MMU_PTE_DESCRIPTOR_MASK) == MMU_PTE_L012_DESCRIPTOR_TABLE) {
      // Set the software bit we use to represent that this page table has been accessed.
      pte |= MMU_PTE_ATTR_RES_SOFTWARE_AF;

      // The result of the compare exchange is ignored as in the unlikely event we have raced with
      // some other page table operation then we will just take another accessed fault.
      if (!compare_exchange_pte(&page_table[index], orig_pte, pte, ktl::memory_order_relaxed)) {
        vm_mmu_set_af_fail.Add(1);
      }
      const paddr_t page_table_paddr = pte & MMU_PTE_OUTPUT_ADDR_MASK;
      pte_t* next_page_table = static_cast<pte_t*>(paddr_to_physmap(page_table_paddr));
      MarkAccessedPageTable(vaddr, vaddr_rem, chunk_size, index_shift - (page_size_shift_ - 3),
                            next_page_table);
    } else if (is_pte_valid(pte) && (pte & MMU_PTE_ATTR_AF) == 0) {
      pte |= MMU_PTE_ATTR_AF;

      // The result of the compare exchange is ignored as in the unlikely event we have raced with
      // some other page table operation then we will just take another accessed fault.
      if (!compare_exchange_pte(&page_table[index], orig_pte, pte, ktl::memory_order_relaxed)) {
        vm_mmu_set_af_fail.Add(1);
      }
    }
    vaddr += chunk_size;
    vaddr_rel += chunk_size;
    size -= chunk_size;
  }
}

zx_status_t ArmArchVmAspace::ProtectPages(vaddr_t vaddr, size_t size, pte_t attrs,
                                          ArchUnmapOptions enlarge, vaddr_t vaddr_base,
                                          ConsistencyManager& cm) {
  vaddr_t vaddr_rel = vaddr - vaddr_base;
  vaddr_t vaddr_rel_max = 1UL << top_size_shift_;

  LTRACEF("vaddr %#" PRIxPTR ", size %#" PRIxPTR ", attrs %#" PRIx64 ", asid %#x\n", vaddr, size,
          attrs, asid_);

  if (vaddr_rel > vaddr_rel_max - size || size > vaddr_rel_max) {
    TRACEF("vaddr %#" PRIxPTR ", size %#" PRIxPTR " out of range vaddr %#" PRIxPTR
           ", size %#" PRIxPTR "\n",
           vaddr, size, vaddr_base, vaddr_rel_max);
    return ZX_ERR_INVALID_ARGS;
  }

  LOCAL_KTRACE("mmu protect", ("vaddr", vaddr), ("size", size));

  zx_status_t ret =
      ProtectPageTable(vaddr, vaddr_rel, size, attrs, enlarge, top_index_shift_, tt_virt_, cm);
  return ret;
}

pte_t ArmArchVmAspace::MmuParamsFromFlags(arch_mmu_flags_t mmu_flags) {
  pte_t attrs = 0;
  switch (type_) {
    case ArmAspaceType::kUser:
      attrs = mmu_flags_to_s1_pte_attr(mmu_flags);
      // User pages are marked non global
      attrs |= MMU_PTE_ATTR_NON_GLOBAL;
      break;
    case ArmAspaceType::kKernel:
      attrs = mmu_flags_to_s1_pte_attr(mmu_flags);
      break;
    case ArmAspaceType::kGuest:
      attrs = mmu_flags_to_s2_pte_attr(mmu_flags);
      break;
    case ArmAspaceType::kHypervisor:
      attrs = mmu_flags_to_s1_pte_attr(mmu_flags, true);
      break;
  }
  return attrs;
}

zx_status_t ArmArchVmAspace::MapContiguous(vaddr_t vaddr, paddr_t paddr, size_t count,
                                           arch_mmu_flags_t mmu_flags) {
  canary_.Assert();
  LTRACEF("vaddr %#" PRIxPTR " paddr %#" PRIxPTR " count %zu flags %#x\n", vaddr, paddr, count,
          mmu_flags);

  DEBUG_ASSERT(tt_virt_);

  DEBUG_ASSERT(IsValidVaddr(vaddr));
  if (!IsValidVaddr(vaddr)) {
    return ZX_ERR_OUT_OF_RANGE;
  }

  if (!(mmu_flags & ARCH_MMU_FLAG_PERM_READ)) {
    return ZX_ERR_INVALID_ARGS;
  }
  if ((mmu_flags & ARCH_MMU_FLAG_PERM_EXECUTE) && arch_mmu_flags_uncached(mmu_flags)) {
    return ZX_ERR_INVALID_ARGS;
  }

  // paddr and vaddr must be aligned.
  DEBUG_ASSERT(IsPageRounded(vaddr));
  DEBUG_ASSERT(IsPageRounded(paddr));
  if (!IsPageRounded(vaddr) || !IsPageRounded(paddr)) {
    return ZX_ERR_INVALID_ARGS;
  }

  if (count == 0) {
    return ZX_OK;
  }

  {
    Guard<CriticalMutex> a{&lock_};
    ASSERT(updates_enabled_);
    if ((mmu_flags & ARCH_MMU_FLAG_PERM_EXECUTE) || type_ == ArmAspaceType::kHypervisor) {
      // The icache gets synced both for executable mappings, which is the expected case, as well as
      // for any hypervisor mapping. For hypervisor mappings we additionally need to clean the cache
      // fully to PoC (not just PoU as required for icache consistency) as guests, who can disable
      // their caches at will, could otherwise see stale data that hasn't been written back to
      // memory yet.
      ArmVmICacheConsistencyManager cache_cm;
      if (type_ == ArmAspaceType::kHypervisor) {
        cache_cm.ForceCleanToPoC();
      }
      cache_cm.SyncAddr(reinterpret_cast<vaddr_t>(paddr_to_physmap(paddr)), count * kPageSize);
    }
    pte_t attrs = MmuParamsFromFlags(mmu_flags);

    __UNINITIALIZED ConsistencyManager cm(*this);
    MappingCursor cursor(/*paddrs=*/&paddr, /*paddr_count=*/1, /*page_size=*/count * kPageSize,
                         /*vaddr=*/vaddr);
    if (!cursor.SetVaddrRelativeOffset(vaddr_base_, 1ull << top_size_shift_)) {
      return ZX_ERR_OUT_OF_RANGE;
    }
    const bool ro = (mmu_flags & ARCH_MMU_FLAG_PERM_RWX_MASK) == ARCH_MMU_FLAG_PERM_READ;
    auto [status, lower_mapped] =
        MapPageTable(attrs, ro, top_index_shift_, tt_virt_, ExistingEntryAction::Error, cursor, cm);
    tt_page_->mmu.num_mappings += lower_mapped;
    if (!accessed_since_last_check_) {
      accessed_since_last_check_ = true;
    }
    if (status != ZX_OK) {
      VirtualAddressCursor unmap_cursor = cursor.ProcessedRange();
      if (unmap_cursor.size() > 0) {
        auto [unmap_status, unmapped] =
            UnmapPageTable(unmap_cursor, ArchUnmapOptions::None, CheckForEmptyPt::Yes,
                           top_index_shift_, tt_virt_, cm, Reclaim::No);
        DEBUG_ASSERT(unmap_status == ZX_OK);
        tt_page_->mmu.num_mappings -= unmapped;
      }
      return status;
    }
    DEBUG_ASSERT(cursor.size() == 0);
  }

#if __has_feature(address_sanitizer)
  if (type_ == ArmAspaceType::kKernel) {
    asan_map_shadow_for(vaddr, count * kPageSize);
  }
#endif  // __has_feature(address_sanitizer)

  return ZX_OK;
}

zx_status_t ArmArchVmAspace::Map(vaddr_t vaddr, paddr_t* phys, size_t count,
                                 arch_mmu_flags_t mmu_flags, ExistingEntryAction existing_action) {
  canary_.Assert();
  LTRACEF("vaddr %#" PRIxPTR " count %zu flags %#x\n", vaddr, count, mmu_flags);

  DEBUG_ASSERT(tt_virt_);

  DEBUG_ASSERT(IsValidVaddr(vaddr));
  if (!IsValidVaddr(vaddr)) {
    return ZX_ERR_OUT_OF_RANGE;
  }
  for (size_t i = 0; i < count; ++i) {
    DEBUG_ASSERT(IsPageRounded(phys[i]));
    if (!IsPageRounded(phys[i])) {
      return ZX_ERR_INVALID_ARGS;
    }
  }

  if (!(mmu_flags & ARCH_MMU_FLAG_PERM_READ)) {
    return ZX_ERR_INVALID_ARGS;
  }
  if ((mmu_flags & ARCH_MMU_FLAG_PERM_EXECUTE) && arch_mmu_flags_uncached(mmu_flags)) {
    return ZX_ERR_INVALID_ARGS;
  }

  // vaddr must be aligned.
  DEBUG_ASSERT(IsPageRounded(vaddr));
  if (!IsPageRounded(vaddr)) {
    return ZX_ERR_INVALID_ARGS;
  }

  if (count == 0) {
    return ZX_OK;
  }

  {
    Guard<CriticalMutex> a{&lock_};
    ASSERT(updates_enabled_);
    if ((mmu_flags & ARCH_MMU_FLAG_PERM_EXECUTE) || type_ == ArmAspaceType::kHypervisor) {
      ArmVmICacheConsistencyManager cache_cm;
      for (size_t idx = 0; idx < count; ++idx) {
        // See comment in MapContiguous for why we do this for the hypervisor.
        if (type_ == ArmAspaceType::kHypervisor) {
          cache_cm.ForceCleanToPoC();
        }
        cache_cm.SyncAddr(reinterpret_cast<vaddr_t>(paddr_to_physmap(phys[idx])), kPageSize);
      }
    }
    pte_t attrs = MmuParamsFromFlags(mmu_flags);

    __UNINITIALIZED ConsistencyManager cm(*this);
    MappingCursor cursor(/*paddrs=*/phys, /*paddr_count=*/count, /*page_size=*/kPageSize,
                         /*vaddr=*/vaddr);
    if (!cursor.SetVaddrRelativeOffset(vaddr_base_, 1ull << top_size_shift_)) {
      return ZX_ERR_OUT_OF_RANGE;
    }
    const bool ro = (mmu_flags & ARCH_MMU_FLAG_PERM_RWX_MASK) == ARCH_MMU_FLAG_PERM_READ;
    auto [status, lower_mapped] =
        MapPageTable(attrs, ro, top_index_shift_, tt_virt_, existing_action, cursor, cm);
    tt_page_->mmu.num_mappings += lower_mapped;
    if (!accessed_since_last_check_) {
      accessed_since_last_check_ = true;
    }
    if (status != ZX_OK) {
      VirtualAddressCursor unmap_cursor = cursor.ProcessedRange();
      if (unmap_cursor.size() > 0) {
        auto [unmap_status, unmapped] =
            UnmapPageTable(unmap_cursor, ArchUnmapOptions::None, CheckForEmptyPt::Yes,
                           top_index_shift_, tt_virt_, cm, Reclaim::No);
        DEBUG_ASSERT(unmap_status == ZX_OK);
        tt_page_->mmu.num_mappings -= unmapped;
      }
      return status;
    }
    DEBUG_ASSERT(cursor.size() == 0);
  }

#if __has_feature(address_sanitizer)
  if (type_ == ArmAspaceType::kKernel) {
    asan_map_shadow_for(vaddr, count * kPageSize);
  }
#endif  // __has_feature(address_sanitizer)

  return ZX_OK;
}

zx_status_t ArmArchVmAspace::Unmap(vaddr_t vaddr, size_t count, ArchUnmapOptions unmap_options) {
  canary_.Assert();
  LTRACEF("vaddr %#" PRIxPTR " count %zu\n", vaddr, count);

  DEBUG_ASSERT(tt_virt_);

  DEBUG_ASSERT(IsValidVaddr(vaddr));

  if (!IsValidVaddr(vaddr)) {
    return ZX_ERR_OUT_OF_RANGE;
  }

  DEBUG_ASSERT(IsPageRounded(vaddr));
  if (!IsPageRounded(vaddr)) {
    return ZX_ERR_INVALID_ARGS;
  }

  Guard<CriticalMutex> a{&lock_};

  ASSERT(updates_enabled_);
  __UNINITIALIZED ConsistencyManager cm(*this);
  VirtualAddressCursor cursor(vaddr, count * kPageSize);
  if (!cursor.SetVaddrRelativeOffset(vaddr_base_, 1ull << top_size_shift_)) {
    return ZX_ERR_OUT_OF_RANGE;
  }
  auto [ret, lower_unmapped] = UnmapPageTable(cursor, unmap_options, CheckForEmptyPt::No,
                                              top_index_shift_, tt_virt_, cm, Reclaim::No);
  tt_page_->mmu.num_mappings -= lower_unmapped;

  DEBUG_ASSERT(cursor.size() == 0 || ret != ZX_OK);

  return ret;
}

zx_status_t ArmArchVmAspace::Protect(vaddr_t vaddr, size_t count, arch_mmu_flags_t mmu_flags,
                                     ArchUnmapOptions enlarge) {
  canary_.Assert();

  if (!IsValidVaddr(vaddr)) {
    return ZX_ERR_INVALID_ARGS;
  }

  if (!IsPageRounded(vaddr)) {
    return ZX_ERR_INVALID_ARGS;
  }

  if (!(mmu_flags & ARCH_MMU_FLAG_PERM_READ)) {
    return ZX_ERR_INVALID_ARGS;
  }
  if ((mmu_flags & ARCH_MMU_FLAG_PERM_EXECUTE) && arch_mmu_flags_uncached(mmu_flags)) {
    return ZX_ERR_INVALID_ARGS;
  }

  // The stage 2 data and instructions aborts do not contain sufficient information for us to
  // resolve permission faults, and these kinds of faults generate a hard error. As such we cannot
  // safely perform protections and instead upgrade any protect to a complete unmap, therefore
  // causing a regular translation fault that we can handle to repopulate the correct mapping.
  if (type_ == ArmAspaceType::kGuest) {
    return Unmap(vaddr, count, ArchUnmapOptions::Enlarge);
  }

  Guard<CriticalMutex> a{&lock_};
  ASSERT(updates_enabled_);
  if (mmu_flags & ARCH_MMU_FLAG_PERM_EXECUTE) {
    // If mappings are going to become executable then we first need to sync their caches.
    // Unfortunately this needs to be done on kernel virtual addresses to avoid taking translation
    // faults, and so we need to first query for the physical address to then get the kernel virtual
    // address in the physmap.
    // This sync could be more deeply integrated into ProtectPages, but making existing regions
    // executable is very uncommon operation and so we keep it simple.
    vm_mmu_protect_make_execute_calls.Add(1);
    ArmVmICacheConsistencyManager cache_cm;
    size_t pages_synced = 0;
    for (size_t idx = 0; idx < count; idx++) {
      paddr_t paddr;
      arch_mmu_flags_t flags;
      if (QueryLocked(vaddr + idx * kPageSize, &paddr, &flags) == ZX_OK &&
          (flags & ARCH_MMU_FLAG_PERM_EXECUTE)) {
        cache_cm.SyncAddr(reinterpret_cast<vaddr_t>(paddr_to_physmap(paddr)), kPageSize);
        pages_synced++;
      }
    }
    vm_mmu_protect_make_execute_pages.Add(pages_synced);
  }

  int ret;
  {
    pte_t attrs = MmuParamsFromFlags(mmu_flags);

    __UNINITIALIZED ConsistencyManager cm(*this);
    ret = ProtectPages(vaddr, count * kPageSize, attrs, enlarge, vaddr_base_, cm);
  }

  return ret;
}

zx_status_t ArmArchVmAspace::HarvestAccessed(vaddr_t vaddr, size_t count,
                                             NonTerminalAction non_terminal_action,
                                             TerminalAction terminal_action) {
  VM_KTRACE_DURATION(2, "ArmArchVmAspace::HarvestAccessed", ("vaddr", vaddr), ("count", count));
  canary_.Assert();

  if (!IsPageRounded(vaddr) || !IsValidVaddr(vaddr)) {
    return ZX_ERR_INVALID_ARGS;
  }

  // Harvesting is done on the restricted and shared aspaces that form the unified aspace, never
  // directly on the unified aspace.
  DEBUG_ASSERT(!IsUnified());

  // Avoid preemption while "involuntarily" holding the arch aspace lock during
  // access harvesting. The harvest loop below is O(n), however, the amount of
  // work performed with the lock held and preemption disabled is limited. Other
  // O(n) operations under this lock are opt-in by the user (e.g. Map, Protect)
  // and are performed with preemption enabled.

  const vaddr_t vaddr_rel = vaddr - vaddr_base_;
  const vaddr_t vaddr_rel_max = 1UL << top_size_shift_;
  const size_t size = count * kPageSize;

  if (vaddr_rel > vaddr_rel_max - size || size > vaddr_rel_max) {
    TRACEF("vaddr %#" PRIxPTR ", size %#" PRIxPTR " out of range vaddr %#" PRIxPTR
           ", size %#" PRIxPTR "\n",
           vaddr, size, vaddr_base_, vaddr_rel_max);
    return ZX_ERR_INVALID_ARGS;
  }

  LOCAL_KTRACE("mmu harvest accessed", ("vaddr", vaddr), ("size", size));

  size_t remaining_size = size;
  vaddr_t current_vaddr = vaddr;
  vaddr_t current_vaddr_rel = vaddr_rel;

  while (remaining_size) {
    // Release and re-acquire the lock to let contending threads have a chance
    // to acquire the arch aspace lock between iterations.
    Guard<CriticalMutex> guard{&lock_};
    ktrace::Scope trace = KTRACE_BEGIN_SCOPE_ENABLE(
        LOCAL_KTRACE_ENABLE, "kernel:vm", "harvest_loop", ("remaining_size", remaining_size));
    size_t entry_limit = kHarvestEntriesBetweenUnlocks;
    __UNINITIALIZED ConsistencyManager cm(*this);
    const size_t harvested_size = HarvestAccessedPageTable(
        &entry_limit, current_vaddr, current_vaddr_rel, remaining_size, top_index_shift_,
        non_terminal_action, terminal_action, tt_virt_, cm);
    DEBUG_ASSERT(harvested_size > 0);
    DEBUG_ASSERT(harvested_size <= remaining_size);

    remaining_size -= harvested_size;
    current_vaddr += harvested_size;
    current_vaddr_rel += harvested_size;
  }

  return ZX_OK;
}

zx_status_t ArmArchVmAspace::MarkAccessed(vaddr_t vaddr, size_t count) {
  VM_KTRACE_DURATION(2, "ArmArchVmAspace::MarkAccessed", ("vaddr", vaddr), ("count", count));
  canary_.Assert();

  if (!IsPageRounded(vaddr) || !IsValidVaddr(vaddr)) {
    return ZX_ERR_OUT_OF_RANGE;
  }

  // Mark accessed is called on the restricted and shared aspaces that form the unified aspace,
  // never directly on the unified aspace.
  DEBUG_ASSERT(!IsUnified());

  const vaddr_t vaddr_rel = vaddr - vaddr_base_;
  const vaddr_t vaddr_rel_max = 1UL << top_size_shift_;
  const size_t size = count * kPageSize;

  if (vaddr_rel > vaddr_rel_max - size || size > vaddr_rel_max) {
    TRACEF("vaddr %#" PRIxPTR ", size %#" PRIxPTR " out of range vaddr %#" PRIxPTR
           ", size %#" PRIxPTR "\n",
           vaddr, size, vaddr_base_, vaddr_rel_max);
    return ZX_ERR_OUT_OF_RANGE;
  }

  LOCAL_KTRACE("mmu mark accessed", ("vaddr", vaddr), ("size", size));

  // Begin our read side critical section that ensures that the page tables we find from tt_virt_
  // will remain valid. Note that the caller of MarkAccessed has guaranteed that the aspace is both
  // alive and that Destroy() will not be called during.
  rcu::AutoSimpleGenerationalReader reader(rcu_);

  MarkAccessedPageTable(vaddr, vaddr_rel, size, top_index_shift_, tt_virt_);
  if (!accessed_since_last_check_) {
    accessed_since_last_check_ = true;
  }

  return ZX_OK;
}

bool ArmArchVmAspace::AccessedSinceLastCheck(bool clear) {
  const bool accessed = accessed_since_last_check_;
  if (clear && accessed_since_last_check_) {
    accessed_since_last_check_ = false;
  }
  return accessed;
}

zx_status_t ArmArchVmAspace::Init() {
  canary_.Assert();
  LTRACEF("aspace %p, base %#" PRIxPTR ", size 0x%zx, type %*s\n", this, base_, size_,
          static_cast<int>(ArmAspaceTypeName(type_).size()), ArmAspaceTypeName(type_).data());

  Guard<CriticalMutex> a{&lock_};

  // Validate that the base + size is sane and doesn't wrap.
  DEBUG_ASSERT(size_ > kPageSize);
  DEBUG_ASSERT(base_ + size_ - 1 > base_);

  if (type_ == ArmAspaceType::kKernel) {
    // At the moment we can only deal with address spaces as globally defined.
    DEBUG_ASSERT(base_ == ~0UL << MMU_KERNEL_SIZE_SHIFT);
    DEBUG_ASSERT(size_ == 1UL << MMU_KERNEL_SIZE_SHIFT);

    vaddr_base_ = ~0UL << MMU_KERNEL_SIZE_SHIFT;
    top_size_shift_ = MMU_KERNEL_SIZE_SHIFT;
    top_index_shift_ = MMU_KERNEL_TOP_SHIFT;
    page_size_shift_ = MMU_KERNEL_PAGE_SIZE_SHIFT;

    tt_virt_ = static_cast<pte_t*>(paddr_to_physmap(root_kernel_page_table_phys));
    tt_phys_ = root_kernel_page_table_phys;
    tt_page_ = Pmm::Node().PaddrToPage(root_kernel_page_table_phys);
    DEBUG_ASSERT(tt_page_);
    DEBUG_ASSERT(tt_page_->state() == vm_page_state::MMU);
    asid_ = (uint16_t)MMU_ARM64_GLOBAL_ASID;
  } else {
    if (type_ == ArmAspaceType::kUser) {
      DEBUG_ASSERT(base_ + size_ <= 1UL << MMU_USER_SIZE_SHIFT);

      vaddr_base_ = 0;
      top_size_shift_ = MMU_USER_SIZE_SHIFT;
      top_index_shift_ = MMU_USER_TOP_SHIFT;
      page_size_shift_ = MMU_USER_PAGE_SIZE_SHIFT;

      if (feat_asid_enabled) {
        auto status = asid->Alloc();
        if (status.is_error()) {
          printf("ARM: out of ASIDs!\n");
          return status.status_value();
        }
        asid_ = status.value();
      } else {
        // Initialize to a valid value even when disabled to distinguish from the destroyed case.
        asid_ = MMU_ARM64_FIRST_USER_ASID;
      }
    } else if (type_ == ArmAspaceType::kGuest) {
      DEBUG_ASSERT(base_ + size_ <= 1UL << MMU_GUEST_SIZE_SHIFT);

      vaddr_base_ = 0;
      top_size_shift_ = MMU_GUEST_SIZE_SHIFT;
      top_index_shift_ = MMU_GUEST_TOP_SHIFT;
      page_size_shift_ = MMU_GUEST_PAGE_SIZE_SHIFT;
    } else {
      DEBUG_ASSERT(type_ == ArmAspaceType::kHypervisor);
      DEBUG_ASSERT(base_ + size_ <= 1UL << MMU_IDENT_SIZE_SHIFT);

      vaddr_base_ = 0;
      top_size_shift_ = MMU_IDENT_SIZE_SHIFT;
      top_index_shift_ = MMU_IDENT_TOP_SHIFT;
      page_size_shift_ = MMU_IDENT_PAGE_SIZE_SHIFT;
    }

    // allocate a top level page table to serve as the translation table
    auto result = AllocPageTable();
    if (result.is_error()) {
      return result.status_value();
    }
    paddr_t pa = (*result)->paddr();

    pte_t* va = static_cast<pte_t*>(paddr_to_physmap(pa));

    tt_virt_ = va;
    tt_phys_ = pa;
    tt_page_ = Pmm::Node().PaddrToPage(tt_phys_);
    DEBUG_ASSERT(tt_page_);

    // zero the top level translation table.
    arch_zero_page(tt_virt_);
    __dsb(ARM_MB_ISHST);
  }
  pt_pages_ = 1;
  kcounter_add(vm_mmu_page_table_alloc, 1);

  LTRACEF("tt_phys %#" PRIxPTR " tt_virt %p\n", tt_phys_, tt_virt_);

  return ZX_OK;
}

zx_status_t ArmArchVmAspace::InitRestricted() {
  role_ = ArmAspaceRole::kRestricted;
  return Init();
}

zx_status_t ArmArchVmAspace::InitShared() {
  zx_status_t status = Init();
  if (status != ZX_OK) {
    return status;
  }
  role_ = ArmAspaceRole::kShared;

  Guard<CriticalMutex> a{&lock_};

  // Prepopulate the portion of the top level page table spanned by this aspace by allocating the
  // necessary second level entries.
  const ulong start = base_ >> top_index_shift_;
  const ulong end = (base_ + size_ - 1) >> top_index_shift_;
  for (ulong i = start; i <= end; i++) {
    DEBUG_ASSERT((load_pte(&tt_virt_[i], ktl::memory_order_relaxed) & MMU_PTE_DESCRIPTOR_MASK) ==
                 MMU_PTE_DESCRIPTOR_INVALID);
    auto result = AllocPageTable();
    if (result.is_error()) {
      return result.status_value();
    }
    paddr_t page_table_paddr = (*result)->paddr();
    void* pt_vaddr = paddr_to_physmap(page_table_paddr);
    arch_zero_page(pt_vaddr);
    __dsb(ARM_MB_ISHST);
    // Do not eagerly set MMU_PTE_ATTR_RES_SOFTWARE_AF for these top level entries. Let the accessed
    // fault (or mapping) path set it only for entries that actually do get accessed, as is the case
    // with any typical aspace.
    //
    // When resolving an accessed fault that originated in the unified aspace, we will look up the
    // faulting address, and if it falls in the shared region, switch to the shared aspace to
    // resolve it. No accessed faults or regular hardware page faults get resolved on the unified
    // aspace. So we do have the ability to set the AF bit only on portions of the shared aspace
    // that are indeed accessed. This saves the accessed bit harvesting code from having to do
    // unnecessary work recursing *all* top level page tables every time.
    //
    // When these top level entries are copied into a unified aspace, they might not have the AF
    // bit set, and since no faults are resolved on the unified aspace, the AF bit might never be
    // set for these entries. This is fine, because we never do any harvesting on the unified aspace
    // anyway, so whether or not AF is set is irrelevant. Similarly any entries that did have AF set
    // at the time of copying are fine too. All harvesting is done on the shared and restricted
    // aspaces that the unified aspace is composed of.
    update_pte(&tt_virt_[i], page_table_paddr | MMU_PTE_L012_DESCRIPTOR_TABLE);
  }
  return ZX_OK;
}

zx_status_t ArmArchVmAspace::InitUnified(ArchVmAspaceInterface& s, ArchVmAspaceInterface& r) {
  canary_.Assert();
  LTRACEF("unified aspace %p, base %#" PRIxPTR ", size 0x%zx, type %*s\n", this, base_, size_,
          static_cast<int>(ArmAspaceTypeName(type_).size()), ArmAspaceTypeName(type_).data());

  ArmArchVmAspace& shared = static_cast<ArmArchVmAspace&>(s);
  ArmArchVmAspace& restricted = static_cast<ArmArchVmAspace&>(r);

  // Initialize this aspace.
  {
    Guard<CriticalMutex> a{&lock_};
    DEBUG_ASSERT(base_ + size_ <= 1UL << MMU_USER_SIZE_SHIFT);

    vaddr_base_ = 0;
    top_size_shift_ = MMU_USER_SIZE_SHIFT;
    top_index_shift_ = MMU_USER_TOP_SHIFT;
    page_size_shift_ = MMU_USER_PAGE_SIZE_SHIFT;

    // Assign the restricted address space's ASID to this address space.
    if (feat_asid_enabled) {
      asid_ = restricted.asid_;
    } else {
      // Initialize to a valid value even when disabled to distinguish from the destroyed case.
      asid_ = MMU_ARM64_FIRST_USER_ASID;
    }

    // Unified aspaces use the same page table root that the restricted page table does.
    tt_virt_ = restricted.tt_virt_;
    tt_phys_ = restricted.tt_phys_;
    tt_page_ = restricted.tt_page_;

    // Set up our pointers to the restricted and shared aspaces.
    restricted_aspace_ = &restricted;
    shared_aspace_ = &shared;
    role_ = ArmAspaceRole::kUnified;

    LTRACEF("tt_phys %#" PRIxPTR " tt_virt %p\n", tt_phys_, tt_virt_);
  }

  const ulong restricted_start = restricted.base_ >> top_index_shift_;
  const ulong restricted_end = (restricted.base_ + restricted.size_ - 1) >> top_index_shift_;
  const ulong shared_start = shared.base_ >> top_index_shift_;
  const ulong shared_end = (shared.base_ + shared.size_ - 1) >> top_index_shift_;
  DEBUG_ASSERT(restricted_end < shared_start);

  // Validate that the restricted aspace is empty and set its metadata.
  {
    Guard<CriticalMutex> a{&restricted.lock_};
    DEBUG_ASSERT(restricted.tt_virt_);
    DEBUG_ASSERT(restricted.num_references_ == 0);
    DEBUG_ASSERT(!restricted.IsUnified());
    for (ulong i = restricted_start; i <= restricted_end; i++) {
      DEBUG_ASSERT((load_pte(&restricted.tt_virt_[i], ktl::memory_order_relaxed) &
                    MMU_PTE_DESCRIPTOR_MASK) == MMU_PTE_DESCRIPTOR_INVALID);
    }
    restricted.num_references_++;
  }

  // Copy all mappings from the shared aspace and set its metadata.
  {
    Guard<CriticalMutex> a{&shared.lock_};
    DEBUG_ASSERT(shared.tt_virt_);
    DEBUG_ASSERT(shared.IsShared());
    DEBUG_ASSERT(!restricted.IsUnified());
    for (ulong i = shared_start; i <= shared_end; i++) {
      pte_t shared_pte = load_pte(&shared.tt_virt_[i], ktl::memory_order_relaxed);
      DEBUG_ASSERT((shared_pte & MMU_PTE_DESCRIPTOR_MASK) == MMU_PTE_L012_DESCRIPTOR_TABLE);
      update_pte(&tt_virt_[i], shared_pte);
    }
    shared.num_references_++;
  }
  return ZX_OK;
}

zx_status_t ArmArchVmAspace::DebugFindFirstLeafMapping(vaddr_t* out_pt, vaddr_t* out_vaddr,
                                                       pte_t* out_pte) const {
  canary_.Assert();

  DEBUG_ASSERT(tt_virt_);
  DEBUG_ASSERT(out_vaddr);
  DEBUG_ASSERT(out_pte);

  const unsigned int count = 1U << (page_size_shift_ - 3);
  pte_t* page_table = tt_virt_;
  uint32_t index_shift = top_index_shift_;
  vaddr_t vaddr = 0;
  while (true) {
    uint64_t index = 0;
    pte_t pte;
    // Walk the page table until we find an entry.
    for (index = 0; index < count; index++) {
      pte = load_pte(&page_table[index], ktl::memory_order_relaxed);
      if (pte != MMU_PTE_DESCRIPTOR_INVALID) {
        break;
      }
    }
    if (index == count) {
      return ZX_ERR_NOT_FOUND;
    }
    // Update the virtual address for the index at the current level.
    vaddr += (index << index_shift);

    const uint descriptor_type = pte & MMU_PTE_DESCRIPTOR_MASK;
    const paddr_t pte_addr = pte & MMU_PTE_OUTPUT_ADDR_MASK;

    // If we have found a leaf mapping, return it.
    if (descriptor_type == ((index_shift > page_size_shift_) ? MMU_PTE_L012_DESCRIPTOR_BLOCK
                                                             : MMU_PTE_L3_DESCRIPTOR_PAGE)) {
      *out_vaddr = vaddr;
      *out_pte = pte;
      *out_pt = reinterpret_cast<vaddr_t>(page_table);
      return ZX_OK;
    }

    // Assume this entry could be corrupted and validate the next table address is valid, and return
    // graceful errors on invalid descriptor types.
    if (!is_physmap_phys_addr(pte_addr) || index_shift <= page_size_shift_ ||
        descriptor_type != MMU_PTE_L012_DESCRIPTOR_TABLE) {
      *out_vaddr = vaddr;
      *out_pte = pte;
      *out_pt = reinterpret_cast<vaddr_t>(page_table);
      return ZX_ERR_BAD_STATE;
    }

    page_table = static_cast<pte_t*>(paddr_to_physmap(pte_addr));
    index_shift -= page_size_shift_ - 3;
  }
}

void ArmArchVmAspace::AssertEmptyLocked() const {
  // Check to see if the top level page table is empty. If not the user didn't
  // properly unmap everything before destroying the aspace.
  const int index = first_used_page_table_entry(tt_virt_, page_size_shift_);
  // Restricted aspaces share their top level page with their associated unified aspace, which
  // maintain shared mappings after base_ + size_. We ignore these mappings when validating that
  // the restricted aspace is empty.
  const int end_index = (int)((base_ + size_ - 1) >> top_index_shift_);
  if (index != -1 && index <= end_index) {
    vaddr_t pt_addr = 0;
    vaddr_t entry_vaddr = 0;
    pte_t pte = 0;
    // Attempt to walk the page table and find the first leaf most mapping that we can. This
    // represents (at least one of) the entries that is holding this page table alive.
    //
    // TODO(https://fxbug.dev/42159319): Once https://fxbug.dev/42159319 is resolved this call, and
    // the entire called method, can be removed.
    zx_status_t status = DebugFindFirstLeafMapping(&pt_addr, &entry_vaddr, &pte);
    panic(
        "top level page table still in use! aspace %p pt_pages_ %zu tt_virt %p index %d entry "
        "%" PRIx64 ". Leaf query status %d pt_addr %zu vaddr %zu entry %" PRIx64 "\n",
        this, pt_pages_, tt_virt_, index, load_pte(&tt_virt_[index], ktl::memory_order_relaxed),
        status, pt_addr, entry_vaddr, pte);
  }

  if (pt_pages_ != 1) {
    panic("allocated page table count is wrong, aspace %p count %zu (should be 1)\n", this,
          pt_pages_);
  }
}

void ArmArchVmAspace::DisableUpdates() {
  canary_.Assert();

  Guard<CriticalMutex> a{&lock_};
  updates_enabled_ = false;
  if (!tt_virt_) {
    // Initialization must not have succeeded.
    return;
  }
  if (!IsShared() && !IsUnified()) {
    AssertEmptyLocked();
  }
}

zx_status_t ArmArchVmAspace::DestroyIndividual() {
  DEBUG_ASSERT(!IsUnified());

  Guard<CriticalMutex> a{&lock_};
  DEBUG_ASSERT(num_references_ == 0);

  // If this page table has a prepopulated top level, we need to manually clean up the entries we
  // created in InitPrepopulated. We know for sure that these entries are no longer referenced by
  // other page tables because we expect those page tables to have been destroyed before this one.
  if (IsShared()) {
    const ulong start = base_ >> top_index_shift_;
    const ulong end = (base_ + size_ - 1) >> top_index_shift_;
    for (ulong i = start; i <= end; i++) {
      const paddr_t paddr =
          load_pte(&tt_virt_[i], ktl::memory_order_relaxed) & MMU_PTE_OUTPUT_ADDR_MASK;
      vm_page_t* page = paddr_to_vm_page(paddr);
      DEBUG_ASSERT(page);
      DEBUG_ASSERT(page->state() == vm_page_state::MMU);
      CacheFreePage(page);
      pt_pages_--;
      update_pte(&tt_virt_[i], MMU_PTE_DESCRIPTOR_INVALID);
    }
  }

  AssertEmptyLocked();

  // Need a DSB to synchronize any page table updates prior to flushing the TLBs.
  __dsb(ARM_MB_ISH);

  // Flush the ASID or VMID associated with this aspace
  FlushAsid();

  // Need a DSB to ensure all other cpus have fully processed the TLB flush.
  __dsb(ARM_MB_ISH);

  // Free any ASID.
  if (type_ == ArmAspaceType::kUser) {
    if (feat_asid_enabled) {
      auto status = asid->Free(asid_);
      ASSERT(status.is_ok());
    } else {
      DEBUG_ASSERT(asid_ == MMU_ARM64_FIRST_USER_ASID);
    }
    asid_ = MMU_ARM64_UNUSED_ASID;
  }

  // Free the top level page table.
  vm_page_t* page = tt_page_;
  DEBUG_ASSERT(page);
  CacheFreePage(page);
  pt_pages_--;
  kcounter_add(vm_mmu_page_table_free, 1);

  tt_phys_ = 0;
  tt_virt_ = nullptr;
  tt_page_ = nullptr;

  return ZX_OK;
}

zx_status_t ArmArchVmAspace::DestroyUnified() {
  {
    Guard<CriticalMutex> a{&shared_aspace_->lock_};
    // The shared page table should be referenced by at least this page table, and could be
    // referenced by many other unified page tables.
    DEBUG_ASSERT(shared_aspace_->num_references_ > 0);
    shared_aspace_->num_references_--;
  }
  {
    Guard<CriticalMutex> a{&restricted_aspace_->lock_};
    // The restricted_aspace_ page table can only be referenced by a singular unified page table.
    DEBUG_ASSERT(restricted_aspace_->num_references_ == 1);
    restricted_aspace_->num_references_--;
  }
  shared_aspace_ = nullptr;
  restricted_aspace_ = nullptr;
  asid_ = MMU_ARM64_UNUSED_ASID;
  tt_phys_ = 0;
  tt_page_ = nullptr;
  tt_virt_ = nullptr;
  return ZX_OK;
}

zx_status_t ArmArchVmAspace::Destroy() {
  canary_.Assert();
  LTRACEF("aspace %p\n", this);

  // We cannot destroy the kernel address space.
  DEBUG_ASSERT(type_ != ArmAspaceType::kKernel);

  // Make sure initialization succeeded.
  if (!tt_virt_) {
    DEBUG_ASSERT(!tt_phys_);
    DEBUG_ASSERT(!tt_page_);
    return ZX_OK;
  }

  if (IsUnified()) {
    return DestroyUnified();
  }
  return DestroyIndividual();
}

// Called during context switches between threads with different address spaces. Swaps the
// mmu context on hardware. Assumes old_aspace != aspace and optimizes as such.
void ArmArchVmAspace::ContextSwitch(ArmArchVmAspace* old_aspace, ArmArchVmAspace* aspace) {
  uint64_t tcr;
  uint64_t ttbr;
  // If we're not using ASIDs, we need to trigger a TLB flush here so we don't leak entries across
  // the context switch. Note that we do not need to perform this flush if we are switching to
  // the kernel's address space, as those mappings are global and will be unaffected by the flush.
  if (aspace && !feat_asid_enabled) {
    // asid_ is always set to MMU_ARM64_FIRST_USER_ASID when ASID use is disabled, so this will
    // invalidate all TLB entries except the global ones.
    DEBUG_ASSERT(aspace->asid_ == MMU_ARM64_FIRST_USER_ASID);
    ARM64_TLBI_ASID(aside1, aspace->asid_);
    // DSB to ensure TLB flushes happen prior to returning to user.
    __dsb(ARM_MB_ISH);
  }
  if (likely(aspace)) {
    aspace->canary_.Assert();
    // Check that we are switching to a user aspace, and that the asid is in the valid range.
    DEBUG_ASSERT(aspace->type_ == ArmAspaceType::kUser);
    DEBUG_ASSERT(aspace->asid_ >= MMU_ARM64_FIRST_USER_ASID);

    // Compute the user space TTBR with the translation table and user space ASID.
    ttbr = ((uint64_t)aspace->asid_ << 48) | aspace->tt_phys_;
    tcr = aspace->Tcr();

    // Update TCR and TTBR0 if the new aspace uses different values, or if we're switching away
    // from the kernel aspace.
    if (unlikely(!old_aspace)) {
      __arm_wsr64("ttbr0_el1", ttbr);
      __arm_wsr64("tcr_el1", tcr);
      __isb(ARM_MB_SY);
    } else {
      uint64_t old_ttbr = ((uint64_t)old_aspace->asid_ << 48) | old_aspace->tt_phys_;
      bool needs_isb = false;
      if (old_ttbr != ttbr) {
        __arm_wsr64("ttbr0_el1", ttbr);
        needs_isb = true;
      }
      if (old_aspace->Tcr() != aspace->Tcr()) {
        __arm_wsr64("tcr_el1", tcr);
        needs_isb = true;
      }
      if (needs_isb) {
        __isb(ARM_MB_SY);
      }
      [[maybe_unused]] uint32_t prev =
          old_aspace->num_active_cpus_.fetch_sub(1, ktl::memory_order_relaxed);
      DEBUG_ASSERT(prev > 0);
    }
    [[maybe_unused]] uint32_t prev =
        aspace->num_active_cpus_.fetch_add(1, ktl::memory_order_relaxed);
    DEBUG_ASSERT(prev < SMP_MAX_CPUS);
  } else {
    // Switching to the null aspace, which means kernel address space only.
    // Load a null TTBR0 and disable page table walking for user space.
    tcr = MMU_TCR_FLAGS_KERNEL;
    __arm_wsr64("tcr_el1", tcr);
    __isb(ARM_MB_SY);

    ttbr = 0;  // MMU_ARM64_UNUSED_ASID
    __arm_wsr64("ttbr0_el1", ttbr);
    __isb(ARM_MB_SY);

    if (likely(old_aspace != nullptr)) {
      [[maybe_unused]] uint32_t prev =
          old_aspace->num_active_cpus_.fetch_sub(1, ktl::memory_order_relaxed);
      DEBUG_ASSERT(prev > 0);
    }
  }
  if (TRACE_CONTEXT_SWITCH) {
    TRACEF("old aspace %p aspace %p ttbr %#" PRIx64 ", tcr %#" PRIx64 "\n", old_aspace, aspace,
           ttbr, tcr);
  }
}

void ArmArchVmAspace::HandoffPageTablesFromPhysboot(VmPageDoublyLinkedList* mmu_pages) {
  while (vm_page_t* page = mmu_pages->pop_front()) {
    page->set_state(vm_page_state::MMU);

    ktl::span entries{
        reinterpret_cast<pte_t*>(paddr_to_physmap(page->paddr())),
        kPageSize / sizeof(pte_t),
    };
    page->mmu.num_mappings = 0;
    for (pte_t& entry : entries) {
      entry = add_parity(entry);
      if ((entry & MMU_PTE_VALID) != 0) {
        page->mmu.num_mappings++;
      }
    }
    page->set_state(vm_page_state::MMU);
  }
}

void arch_zero_page(void* _ptr) {
  const uint32_t zva_size = arm64_cache_features.zva_size;

  uintptr_t ptr = (uintptr_t)_ptr;
  uintptr_t end_ptr = ptr + kPageSize;
  do {
    __asm__ volatile("dc zva, %0" ::"r"(ptr));
    ptr += zva_size;
  } while (ptr != end_ptr);
}

zx_status_t arm64_mmu_translate(vaddr_t va, paddr_t* pa, bool user, bool write) {
  // disable interrupts around this operation to make the at/par instruction combination atomic
  uint64_t par;
  {
    InterruptDisableGuard irqd;

    if (user) {
      if (write) {
        __asm__ volatile("at s1e0w, %0" ::"r"(va) : "memory");
      } else {
        __asm__ volatile("at s1e0r, %0" ::"r"(va) : "memory");
      }
    } else {
      if (write) {
        __asm__ volatile("at s1e1w, %0" ::"r"(va) : "memory");
      } else {
        __asm__ volatile("at s1e1r, %0" ::"r"(va) : "memory");
      }
    }

    // Explicitly synchronize before reading from the PAR register, as documented as a
    // general requirement after modifying a control register that affects the result
    // of another one.
    __isb(ARM_MB_SY);

    par = __arm_rsr64("par_el1");
  }

  // if bit 0 is clear, the translation succeeded
  if (BIT(par, 0)) {
    return ZX_ERR_NOT_FOUND;
  }

  // physical address is stored in bits [51..12], naturally aligned
  *pa = BITS(par, 51, 12) | (va & (kPageSize - 1));

  return ZX_OK;
}

ArmArchVmAspace::ArmArchVmAspace(vaddr_t base, size_t size, ArmAspaceType type, page_alloc_fn_t paf)
    : type_(type), test_page_alloc_func_(paf), base_(base), size_(size) {}

ArmArchVmAspace::ArmArchVmAspace(vaddr_t base, size_t size, arch_mmu_flags_t mmu_flags,
                                 page_alloc_fn_t paf)
    : ArmArchVmAspace(base, size, AspaceTypeFromFlags(mmu_flags), paf) {}

ArmArchVmAspace::~ArmArchVmAspace() {
  // Destroy() will have freed the final page table if it ran correctly, and further validated that
  // everything else was freed.
  DEBUG_ASSERT(pt_pages_ == 0);
}

vaddr_t ArmArchVmAspace::PickSpot(vaddr_t base, vaddr_t end, vaddr_t align, size_t size,
                                  arch_mmu_flags_t mmu_flags) {
  canary_.Assert();
  return RoundUpPageSize(base);
}

void ArmVmICacheConsistencyManager::SyncAddr(vaddr_t start, size_t len) {
  // Validate we are operating on a kernel address range.
  DEBUG_ASSERT(is_kernel_address(start));

  // Use the physmap to clean the range. If we have been requested to clean to PoC then we must do
  // that now, otherwise we can either clean to POU or if the cpu allows it, skip cleaning.
  // Both of the clean functions have a DSB at the end to synchronize the DC cache instructions.
  if (clean_poc_) {
    arch_clean_cache_range(start, len);
  } else if (!arm64_cache_features.idc) {
    // We need to clean to POU to maintain D->I cache coherency.
    arm64_clean_cache_range_pou(start, len);
  } else {  // IDC == 1
    need_dsb_ = true;
  }

  // Do we need to invalidate the icache?
  if (!arm64_cache_features.dic) {  // yes
    // Are we VIPT or PIPT?
    if (arm64_cache_features.pipt) {
      // We can invalidate just this alias of the page.
      if (need_dsb_) {
        // If we haven't DSBed earlier, we must now.
        __dsb(ARM_MB_ISHST);
      }
      arch::InvalidateInstructionCacheRange(start, len);
    } else {
      // We will batch the icache global invalidate and perform it at the end.
      need_invalidate_ = need_dsb_ = true;
    }
  } else {
    // IF ctr.dic == 1, we do not need to invalidate the icache at all, but we will need to
    // insert an ISB at the end, so set need_invalidate here and sync later.
    need_invalidate_ = true;
  }
}

void ArmVmICacheConsistencyManager::Finish() {
  if (!need_invalidate_) {
    return;
  }

  // Previously we were instructed to DSB, so do so here before any icache flushing or syncing.
  if (need_dsb_) {
    __dsb(ARM_MB_ISHST);
  }

  // In the ideal scenario where the cpu is both instruction to data and data to instruction
  // coherent, we only need a data barrier and instruction barrier to keep the cpu from
  // prefetching around the barrier.
  // NOTE 1: if DIC is set, then we do not need to concern ourselves with aliased pages,
  // and thus VIPT and PIPT are not relevant.
  // NOTE 2: if DIC is 1 then IDC must also be 1.
  if (arm64_cache_features.dic) {
    __isb(ARM_MB_SY);
    return;
  }

  // If we're here and need_invalidate_ is set we must be VIPT and we do not know all of
  // the virtual aliases of the sections we cleaned so our only option is to
  // dump the entire icache.
  arch::InvalidateGlobalInstructionCache();
  need_invalidate_ = false;
}

void arm64_mmu_early_init() {
  root_lower_page_table_phys = arch::ArmTtbr0El1::Read().addr();
  root_kernel_page_table_phys = arch::ArmTtbr1El1::Read().addr();

  // Our current ASID allocation scheme is very naive and allocates a unique ASID to every address
  // space, which means that there are often not enough ASIDs when the machine uses 8-bit ASIDs.
  // Therefore, if we detect that we are only given 8-bit ASIDs, disable their use.
  feat_asid_enabled =
      BootOptions::Get()->arm64_enable_asid && arm64_asid_width() != arm64_asid_width::ASID_8;

  // After we've probed the feature set and parsed the boot options, initialize the asid
  // allocator.
  if (feat_asid_enabled) {
    asid.Initialize();
  } else {
    dprintf(INFO, "mmu: not using ASIDs\n");
  }
}

uint32_t arch_address_tagging_features() {
  static_assert(MMU_TCR_FLAGS_USER & MMU_TCR_TBI0, "Expected TBI enabled.");
  return ZX_ARM64_FEATURE_ADDRESS_TAGGING_TBI;
}
