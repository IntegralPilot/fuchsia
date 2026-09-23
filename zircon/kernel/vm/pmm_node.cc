// Copyright 2018 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "vm/pmm_node.h"

#include <assert.h>
#include <inttypes.h>
#include <lib/instrumentation/asan.h>
#include <lib/memalloc/range.h>
#include <lib/page/size.h>

#include <kernel/scheduler.h>
#include <phys/handoff.h>
#include <vm/compression.h>
#include <vm/phys/arena.h>
#include <vm/physmap.h>
#include <vm/pmm.h>
#include <vm/pmm_node_ffi.h>
#include <vm/vm_aspace.h>

// We disable thread safety analysis here, since this function is only called
// during early boot before threading exists.
zx_status_t PmmNode::Init(ktl::span<const memalloc::Range> ranges) TA_NO_THREAD_SAFETY_ANALYSIS {
  // Make sure we're in early boot (ints disabled and no active Schedulers)
  DEBUG_ASSERT(Scheduler::PeekActiveMask() == 0);
  DEBUG_ASSERT(arch_ints_disabled());

  zx_status_t status = ZX_OK;
  auto init_arena = [&status, this](const PmmArenaSelection& selected) {
    if (status == ZX_ERR_NO_MEMORY) {
      return;
    }
    zx_status_t init_status = InitArena(selected);
    if (status == ZX_OK) {
      status = init_status;
    }
  };

  bool allocation_excluded = false;
  auto record_error = [&allocation_excluded](const PmmArenaSelectionError& error) {
    bool allocated = memalloc::IsAllocatedType(error.range.type);
    allocation_excluded = allocation_excluded || allocated;

    // If we have to throw out less than two pages of free RAM, don't regard
    // that as a full blown error.
    const char* error_type =
        error.type == PmmArenaSelectionError::Type::kTooSmall && !allocated ? "warning" : "error";
    ktl::string_view reason = PmmArenaSelectionError::ToString(error.type);
    ktl::string_view range_type = memalloc::ToString(error.range.type);
    printf("PMM: %s: unable to include [%#" PRIx64 ", %#" PRIx64 ") (%.*s) in arena: %.*s\n",
           error_type,                                              //
           error.range.addr, error.range.end(),                     //
           static_cast<int>(range_type.size()), range_type.data(),  //
           static_cast<int>(reason.size()), reason.data());
  };

  SelectPmmArenas<kPageSize>(ranges, init_arena, record_error);
  if (status != ZX_OK) {
    return status;
  }

  // If we fail to include a pre-PMM allocation in an arena that could be
  // disastrous in unpredictable/hard-to-debug ways, so fail hard early.
  ZX_ASSERT(!allocation_excluded);

  // Now mark all pre-PMM allocations and holes within our arenas as reserved.
  ktl::span arenas = active_arenas();
  auto reserve_range = [this, arena{arenas.begin()},
                        end{arenas.end()}](const memalloc::Range& range) mutable {
    // Find the first arena encompassing this range.
    //
    // Note that trying to include `range` in an arena may have resulted in an
    // error during the selection process. If we do encounter a range not in an
    // arena, just skip it.
    while (arena != end && arena->end() <= range.addr) {
      ++arena;
    }
    if (arena == end) {
      // In this case the tail of ranges did not end up in any arenas, so we can
      // just short-circuit.
      return false;
    }
    if (!arena->address_in_arena(range.addr)) {
      return true;
    }

    DEBUG_ASSERT(arena->address_in_arena(range.end() - 1));
    InitReservedRange(range);
    return true;
  };
  ForEachAlignedAllocationOrHole<kPageSize>(ranges, reserve_range);

  return ZX_OK;
}

void PmmNode::EndHandoff() { rust_pmm_node_end_handoff(this); }

zx_status_t PmmNode::GetArenaInfo(size_t count, uint64_t i, pmm_arena_info_t* buffer,
                                  size_t buffer_size) {
  return rust_pmm_node_get_arena_info(this, count, i, buffer, buffer_size);
}

void PmmNode::AddFreePages(VmPageDoublyLinkedList* list) TA_NO_THREAD_SAFETY_ANALYSIS {
  rust_pmm_node_add_free_pages(this, list);
}

void PmmNode::FillFreePagesAndArm() { rust_pmm_node_fill_free_pages_and_arm(this); }

void PmmNode::CheckAllFreePages() { rust_pmm_node_check_all_free_pages(this); }

#if __has_feature(address_sanitizer)
void PmmNode::PoisonAllFreePages() { rust_pmm_node_poison_all_free_pages(this); }
#endif  // __has_feature(address_sanitizer)

bool PmmNode::EnableFreePageFilling(size_t fill_size, CheckFailAction action) {
  return rust_pmm_node_enable_free_page_filling(this, fill_size, static_cast<uint8_t>(action));
}

zx::result<vm_page_t*> PmmNode::AllocLoanedPage(
    fit::inline_function<void(vm_page_t*), 32> allocated) {
  vm_page_t* out_page = nullptr;
  auto cb = [](vm_page_t* page, void* cookie) {
    auto* fn = reinterpret_cast<fit::inline_function<void(vm_page_t*), 32>*>(cookie);
    (*fn)(page);
  };
  zx_status_t status = rust_pmm_node_alloc_loaned_page(this, cb, &allocated, &out_page);
  if (status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(out_page);
}

zx::result<vm_page_t*> PmmNode::AllocPage(uint alloc_flags) {
  vm_page_t* page = nullptr;
  zx_status_t status = rust_pmm_node_alloc_page(this, alloc_flags, &page);
  if (status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(page);
}

zx_status_t PmmNode::AllocPages(size_t count, uint alloc_flags, VmPageDoublyLinkedList* list) {
  return rust_pmm_node_alloc_pages(this, count, alloc_flags, list);
}

zx_status_t PmmNode::AllocRange(paddr_t address, size_t count, VmPageDoublyLinkedList* list) {
  return rust_pmm_node_alloc_range(this, address, count, list);
}

zx_status_t PmmNode::AllocContiguous(const size_t count, uint alloc_flags, uint8_t alignment_log2,
                                     paddr_t* pa, VmPageDoublyLinkedList* list) {
  return rust_pmm_node_alloc_contiguous(this, count, alloc_flags, alignment_log2, pa, list);
  // Forbid zero size contiguous allocations because we are obligated to provide the physical
  // address on success, but have no sensible value to give.
}

// We disable thread safety analysis here, since this function is only called
// during early boot before threading exists.
zx_status_t PmmNode::InitArena(const PmmArenaSelection& selected) TA_NO_THREAD_SAFETY_ANALYSIS {
  if (used_arena_count_ >= kArenaCount) {
    return ZX_ERR_NOT_SUPPORTED;
  }
  if (selected.arena.size > (kMaxPagesPerArena * kPageSize)) {
    // We have this limit since we need to compress a page_t pointer to a 24 bit integer.
    return ZX_ERR_NOT_SUPPORTED;
  }

  arenas_[used_arena_count_++].Init(selected, this);
  arena_cumulative_size_ += selected.arena.size;
  return ZX_OK;
}

void PmmNode::InitReservedRange(const memalloc::Range& range) {
  DEBUG_ASSERT(IsPageRounded(range.addr));
  DEBUG_ASSERT(IsPageRounded(range.size));

  ktl::string_view what =
      range.type == memalloc::Type::kReserved ? "hole in RAM"sv : memalloc::ToString(range.type);
  VmPageDoublyLinkedList reserved;
  zx_status_t status = pmm_alloc_range(range.addr, range.size / kPageSize, &reserved);
  if (status != ZX_OK) {
    dprintf(INFO, "PMM: unable to reserve [%#" PRIx64 ", %#" PRIx64 "): %.*s: %d\n", range.addr,
            range.end(), static_cast<int>(what.size()), what.data(), status);
    return;  // this is probably fatal but go ahead and continue
  }
  dprintf(INFO, "PMM: reserved [%#" PRIx64 ", %#" PRIx64 "): %.*s\n", range.addr, range.end(),
          static_cast<int>(what.size()), what.data());

  // Kernel page tables belong to the arch-specific VM backend, just as they'd
  // be if they were created post-Physboot.
  if (range.type == memalloc::Type::kKernelPageTables) {
    ArchVmAspace::HandoffPageTablesFromPhysboot(&reserved);
    return;
  }

  // Otherwise, mark it as wired and merge it into the appropriate reserved
  // list.
  for (auto& p : reserved) {
    p.set_state(vm_page_state::WIRED);
  }

  VmPageDoublyLinkedList* list;
  if (range.type == memalloc::Type::kTemporaryPhysHandoff) {
    list = &phys_handoff_temporary_list_;
  } else if (PhysHandoff::IsPhysVmoType(range.type)) {
    list = &phys_handoff_vmo_list_;
  } else {
    list = &permanently_reserved_list_;
  }
  list->splice(list->end(), reserved);
}

void PmmNode::BeginFreeLoanedPage(vm_page_t* page,
                                  fit::inline_function<void(vm_page_t*)> release_page,
                                  FreeLoanedPagesHolder& flph) {
  auto cb = [](vm_page_t* page, void* cookie) {
    auto* fn = reinterpret_cast<fit::inline_function<void(vm_page_t*)>*>(cookie);
    (*fn)(page);
  };
  rust_pmm_node_begin_free_loaned_page(this, page, cb, &release_page, &flph);
}

void PmmNode::FinishFreeLoanedPages(FreeLoanedPagesHolder& flph) {
  rust_pmm_node_finish_free_loaned_pages(this, &flph);
}

void PmmNode::WithLoanedPage(vm_page_t* page, fit::inline_function<void(vm_page_t*)> with_page) {
  auto cb = [](vm_page_t* page, void* cookie) {
    auto* fn = reinterpret_cast<fit::inline_function<void(vm_page_t*)>*>(cookie);
    (*fn)(page);
  };
  rust_pmm_node_with_loaned_page(this, page, cb, &with_page);
}

void PmmNode::FreePage(vm_page* page, PmmOptDelayReuse delay_reuse) {
  rust_pmm_node_free_page(this, page, delay_reuse);
}

void PmmNode::BeginFreeLoanedArray(
    vm_page_t** pages, size_t count,
    fit::inline_function<void(vm_page_t**, size_t, VmPageDoublyLinkedList*)> release_list,
    FreeLoanedPagesHolder& flph) {
  auto cb = [](vm_page_t** pages, size_t count, VmPageDoublyLinkedList* list, void* cookie) {
    auto* fn =
        reinterpret_cast<fit::inline_function<void(vm_page_t**, size_t, VmPageDoublyLinkedList*)>*>(
            cookie);
    (*fn)(pages, count, list);
  };
  rust_pmm_node_begin_free_loaned_array(this, pages, count, cb, &release_list, &flph);
}

void PmmNode::FreeList(VmPageDoublyLinkedList* list, PmmOptDelayReuse delay_reuse) {
  rust_pmm_node_free_list(this, list, delay_reuse);
}

void PmmNode::UnwirePage(vm_page* page) { rust_pmm_node_unwire_page(this, page); }

uint64_t PmmNode::CountFreePages() const TA_NO_THREAD_SAFETY_ANALYSIS {
  return rust_pmm_node_count_free_pages(this);
}

uint64_t PmmNode::CountLoanedFreePages() const TA_NO_THREAD_SAFETY_ANALYSIS {
  return rust_pmm_node_count_loaned_free_pages(this);
}

uint64_t PmmNode::CountLoanedNotFreePages() const TA_NO_THREAD_SAFETY_ANALYSIS {
  return rust_pmm_node_count_loaned_not_free_pages(this);
}

uint64_t PmmNode::CountLoanedPages() const TA_NO_THREAD_SAFETY_ANALYSIS {
  return rust_pmm_node_count_loaned_pages(this);
}

uint64_t PmmNode::CountLoanCancelledPages() const TA_NO_THREAD_SAFETY_ANALYSIS {
  return rust_pmm_node_count_loan_cancelled_pages(this);
}

uint64_t PmmNode::CountTotalBytes() const TA_NO_THREAD_SAFETY_ANALYSIS {
  return rust_pmm_node_count_total_bytes(this);
}

void PmmNode::DumpFree() const TA_NO_THREAD_SAFETY_ANALYSIS { rust_pmm_node_dump_free(this); }

void PmmNode::Dump(bool is_panic) const { rust_pmm_node_dump(this, is_panic); }

bool PmmNode::SetFreeMemorySignal(uint64_t free_lower_bound, uint64_t free_upper_bound,
                                  uint64_t delay_allocations_pages, Event* event) {
  return rust_pmm_node_set_free_memory_signal(this, free_lower_bound, free_upper_bound,
                                              delay_allocations_pages, event);
}

void PmmNode::StopReturningShouldWait() { rust_pmm_node_stop_returning_should_wait(this); }

int64_t PmmNode::get_alloc_failed_count() { return rust_pmm_node_get_alloc_failed_count(); }

void PmmNode::BeginLoan(VmPageDoublyLinkedList* page_list, PmmOptDelayReuse delay_reuse) {
  rust_pmm_node_begin_loan(this, page_list, delay_reuse);
}

void PmmNode::CancelLoan(vm_page_t* page) { rust_pmm_node_cancel_loan(this, page); }

void PmmNode::EndLoan(vm_page_t* page) { rust_pmm_node_end_loan(this, page); }

void PmmNode::ReportAllocFailure(AllocFailure failure) {
  rust_pmm_node_report_alloc_failure(this, &failure);
}

PmmNode::AllocFailure PmmNode::GetFirstAllocFailure() {
  AllocFailure failure;
  rust_pmm_node_get_first_alloc_failure(this, &failure);
  return failure;
}

void PmmNode::SeedRandomShouldWait() { rust_pmm_node_seed_random_should_wait(this); }

zx_status_t PmmNode::SetPageCompression(fbl::RefPtr<VmCompression> compression) {
  return rust_pmm_node_set_page_compression(this, fbl::ExportToRawPtr(&compression));
}

const char* PmmNode::AllocFailure::TypeToString(Type type) {
  switch (type) {
    case Type::None:
      return "None";
    case Type::Pmm:
      return "PMM";
    case Type::Heap:
      return "Heap";
    case Type::Handle:
      return "Handle";
    case Type::Other:
      return "Other";
  }
  return "UNKNOWN";
}

zx::result<vm_page_t*> PmmNode::WaitForSinglePageAllocation(Deadline deadline, bool suspendable) {
  vm_page_t* out_page = nullptr;
  zx_status_t status = rust_pmm_node_wait_for_single_page_allocation(
      this, deadline.when(), deadline.slack().amount(), deadline.slack().mode(), suspendable,
      &out_page);
  if (status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(out_page);
}
