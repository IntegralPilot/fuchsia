// Copyright 2017 The Fuchsia Authors
// Copyright (c) 2014 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_VM_INCLUDE_VM_PMM_H_
#define ZIRCON_KERNEL_VM_INCLUDE_VM_PMM_H_

#include <zircon/compiler.h>
#include <zircon/types.h>

#include <vm/page.h>
#include <vm/pmm_node.h>

// Forward declaration; defined in <lib/memalloc/range.h>
namespace memalloc {
struct Range;
}

// Pmm class exists purely to serve as a way to define private storage and public accessor of the
// global PmmNode.
class Pmm {
 public:
  // Retrieve the global PmmNode.
  static PmmNode& Node() { return node_; }

 private:
  // Note: This is referenced directly from Rust and changing the name of this or the class will
  // result in a link error.
  static PmmNode node_;
};

// Initializes the PMM with the provided, unnormalized and normalized memory
// ranges. This in particular initializes its arenas and wires any previously
// allocated special subranges or holes.
zx_status_t pmm_init(ktl::span<const memalloc::Range> ranges);

// Ends the PMM's role within the context of phys handoff: it frees all physical
// memory temporarily used for the hand-off from physboot. Since this memory
// includes that backing the hand-off struct itself (accessible via
// gPhysHandoff), this call is intended to be the last thing done in the process
// of ending the hand-off.
void pmm_end_handoff();

// Returns the number of arenas.
size_t pmm_num_arenas();

// Copies |count| pmm_arena_info_t objects into |buffer| starting with the |i|-th arena ordered by
// base address.  For example, passing an |i| of 1 would skip the 1st arena.
//
// The objects will be sorted in ascending order by arena base address.
//
// Returns ZX_ERR_OUT_OF_RANGE if |count| is 0 or |i| and |count| specify an invalid range.
//
// Returns ZX_ERR_BUFFER_TOO_SMALL if the buffer is too small.
zx_status_t pmm_get_arena_info(size_t count, uint64_t i, pmm_arena_info_t* buffer,
                               size_t buffer_size);

// Allocate count pages of physical memory, adding to the tail of the passed list.
// The list must be initialized.
// Note that if PMM_ALLOC_FLAG_CAN_WAIT is passed in then this could always return
// ZX_ERR_SHOULD_WAIT. Since there is no way to wait until an arbitrary number of pages can be
// allocated (see comment on |pmm_wait_till_should_retry_single_alloc|) passing
// PMM_ALLOC_FLAG_CAN_WAIT here should be used as an optimistic fast path, and the caller should
// have a fallback of allocating single pages.
zx_status_t pmm_alloc_pages(size_t count, uint alloc_flags, VmPageDoublyLinkedList* list)
    __NONNULL((3));

// Allocate a single page of physical memory.
zx_status_t pmm_alloc_page(uint alloc_flags, vm_page** p) __NONNULL((2));
zx_status_t pmm_alloc_page(uint alloc_flags, paddr_t* pa) __NONNULL((2));
zx_status_t pmm_alloc_page(uint alloc_flags, vm_page** p, paddr_t* pa) __NONNULL((2, 3));

// Allocate a specific range of physical pages, adding to the tail of the passed list.
zx_status_t pmm_alloc_range(paddr_t address, size_t count, VmPageDoublyLinkedList* list)
    __NONNULL((3));

// Allocate a run of contiguous pages, aligned on log2 byte boundary (0-31).
// Return the base address of the run in the physical address pointer and
// append the allocate page structures to the tail of the passed in list.
zx_status_t pmm_alloc_contiguous(size_t count, uint alloc_flags, uint8_t align_log2, paddr_t* pa,
                                 VmPageDoublyLinkedList* list) __NONNULL((4, 5));

// Unwires a page and sets it in the ALLOC state.
void pmm_unwire_page(vm_page_t* page);

// Free a list of physical pages. This list must not contained loaned pages returned from
// PmmNode::AllocLoanedPage.
//
// |delay_reuse| specifies whether reuse of the pages (i.e. reallocation) should be delayed or not.
void pmm_free(VmPageDoublyLinkedList* list,
              PmmOptDelayReuse delay_reuse = PmmOptDelayReuse::Default) __NONNULL((1));

// Free a single page. This page must not be a loaned page returned from PmmNode::AllocLoanedPage.
//
// See |pmm_free| for meaning of the |delay_reuse| parameter.
void pmm_free_page(vm_page_t* page, PmmOptDelayReuse delay_reuse = PmmOptDelayReuse::Default)
    __NONNULL((1));

// Return count of unallocated physical pages in system.
uint64_t pmm_count_free_pages();

// Return count of unallocated loaned physical pages in system.
uint64_t pmm_count_loaned_free_pages();

uint64_t pmm_count_loaned_used_pages();

// Return count of loaned pages, including both allocated and unallocated.
uint64_t pmm_count_loaned_pages();

// Return count of pages which are presently loaned with the loan cancelled.  This is a transient
// state so we shouldn't see a non-zero value persisting for long unless the system is constantly
// seeing loan/cancel churn.
uint64_t pmm_count_loan_cancelled_pages();

// Return amount of physical memory in system, in bytes.
uint64_t pmm_count_total_bytes();

// Return the PageQueues.
PageQueues* pmm_page_queues();

// Return the Evictor.
Evictor* pmm_evictor();

// virtual to physical for kernel addresses.
paddr_t vaddr_to_paddr(const void* va);

// paddr to vm_page_t
vm_page_t* paddr_to_vm_page(paddr_t addr);

// Configures the free memory bounds and allows for setting a one shot signal as well as a level
// where allocations should start being delayed.
//
// The event is signaled once the number of PMM free pages falls outside of the range given by
// |free_lower_bound| and |free_upper_bound|. As the event is one shot, one signaled this must be
// called again to configure a new range. If the number of free pages is already outside the
// requested bound then this method fails (returns false) and no event is setup. In this case the
// caller should recalculate a correct bounds and try again.
//
// In addition to exiting the provided memory bounds, the event will also get signaled on the first
// time an allocation fails (i.e. the first time at which has_alloc_failed_no_mem would return
// true).
//
// |delay_allocations_level| is the number of PMM free pages below which the PMM will transition to
// delaying allocations that can wait, i.e. those with PMM_ALLOC_FLAG_CAN_WAIT. This transition is
// sticky, and even if pages are freed to go back above this line, allocations will remain delayed
// until this method is called again to re-set the level. For this reason, and since there is only a
// single common Event, the |delay_allocations_level| must either be <= the |free_lower_bound|,
// ensuring that the caller will have been notified and can respond by freeing memory and/or setting
// a new level, or |delay_allocations_level| can be UINT64_MAX, indicating allocations should start
// and remain delayed.
bool pmm_set_free_memory_signal(uint64_t free_lower_bound, uint64_t free_upper_bound,
                                uint64_t delay_allocations_level, Event* event);

// Tells the PMM that it should never return ZX_ERR_SHOULD_WAIT (even in the presence of
// PMM_ALLOC_FLAG_CAN_WAIT) and from now on must either succeed an allocation, or fail with
// ZX_ERR_NO_MEMORY.
// There is no way to re-enable this as disabling is intended for use in the panic/shutdown path.
void pmm_stop_returning_should_wait();

// Should be called after the kernel command line has been parsed.
void pmm_checker_init_from_cmdline();

// Synchronously walk the PMM's free list and validate each page.  This is an incredibly expensive
// operation and should only be used for debugging purposes.
void pmm_checker_check_all_free_pages();

// Synchronously walk the PMM's free list and poison (via kASAN) each page. This is an
// incredibly expensive operation and should be used with care.
void pmm_asan_poison_all_free_pages();

void pmm_print_physical_page_borrowing_stats();

#endif  // ZIRCON_KERNEL_VM_INCLUDE_VM_PMM_H_
