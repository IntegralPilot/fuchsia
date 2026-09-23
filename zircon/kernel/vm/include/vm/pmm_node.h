// Copyright 2018 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT
#ifndef ZIRCON_KERNEL_VM_INCLUDE_VM_PMM_NODE_H_
#define ZIRCON_KERNEL_VM_INCLUDE_VM_PMM_NODE_H_

#include <lib/memalloc/range.h>
#include <lib/page/size.h>

#include <fbl/canary.h>
#include <fbl/intrusive_double_list.h>
#include <kernel/event.h>
#include <kernel/lockdep.h>
#include <kernel/mutex.h>
#include <ktl/span.h>
#include <vm/evictor.h>
#include <vm/page_queues.h>
#include <vm/physical_page_borrowing_config.h>
#include <vm/pmm_arena.h>
#include <vm/pmm_checker.h>

// Forward declaration; defined in <lib/memalloc/range.h>
namespace memalloc {
struct Range;
}

// flags for allocation routines below
#define PMM_ALLOC_FLAG_ANY (0 << 0)  // no restrictions on which arena to allocate from
// The caller is able to wait and retry this allocation and so pmm allocation functions are allowed
// to return ZX_ERR_SHOULD_WAIT, as opposed to ZX_ERR_NO_MEMORY, to indicate that the caller should
// wait and try again. This is intended for the PMM to tell callers who are able to wait that memory
// is low. The caller should not infer anything about memory state if it is told to wait, as the PMM
// may tell it to wait for any reason.
#define PMM_ALLOC_FLAG_CAN_WAIT (1 << 0)

// This enum is used to specify whether a page, when freed, should have its reuse
// (i.e. reallocation) delayed.  This feature exists to both improve the PMM checker's ability to
// detect "bad DMAs" and to reduce the impact when they do occur.  When allocating a page, reusing
// the most recently freed page often has performance benefits.  However, it can amplify the impact
// of use-after-free bugs.  This feature enables part of the kernel to express a preference on
// whether a page should be eligible for immediate reuse or not.  It's a hint.
//
// |Default| means no preference.  When specified, the page may or may not be immediately reused.
// In some build/runtime configurations (e.g. kasan) delayed reuse is the default behavior.
//
// |Yes| indicates that the PMM should delay the reuse of the page by placing it on the "cold" end
// up the free list, thereby maximizing the amount of time before which it is reallocated.
enum class PmmOptDelayReuse : bool { Default, Yes };

class FreeLoanedPagesHolder;
class VmCompression;

// per numa node collection of pmm arenas and worker threads
class PmmNode {
 public:
  // This constructor may be called early in the boot sequence so make sure it does not do any "real
  // work" or depend on any globals.
  PmmNode() = default;
  ~PmmNode() = default;

  DISALLOW_COPY_ASSIGN_AND_MOVE(PmmNode);

  zx_status_t Init(ktl::span<const memalloc::Range> ranges);

  // See pmm_end_handoff().
  void EndHandoff();

  vm_page_t* PaddrToPage(paddr_t addr) TA_NO_THREAD_SAFETY_ANALYSIS;

  static constexpr int kIndexZeroBits = 3;

  // Returns compressed representation a page_t*, with the following characteristics:
  // - zeros in the last kIndexZeroBits bits, used by clients to store metadata.
  // - The value 0 is never returned, it can be used as "no page" marker.
  //
  // Note: This method needs to traverse (up to) all the memory pools so it's cost is
  //       low but not trivial.
  uint32_t PageToIndex(const vm_page_t* page) TA_NO_THREAD_SAFETY_ANALYSIS;
  // Converts the number returned by PageToIndex() back to a page_t* pointer.
  // It does not check for invalid indexes such as 0.
  //
  // Note: This method is faster than PageToIndex(), about the cost of some basic math
  //       and bit manipulation.
  vm_page_t* IndexToPage(uint32_t index) TA_NO_THREAD_SAFETY_ANALYSIS;

  // Converts the number returned by PageToIndex() back to a paddr_t.
  // It does not check for invalid indexes such as 0 or kIndexReserved0.
  //
  // Note: This method is faster than IndexToPage()->paddr() as the vm_page_t itself does not have
  //       to be de-referenced, saving a memory load.
  paddr_t IndexToPaddr(uint32_t index) TA_NO_THREAD_SAFETY_ANALYSIS;

  // main allocator routines
  zx::result<vm_page_t*> AllocPage(uint alloc_flags);
  zx_status_t AllocPages(size_t count, uint alloc_flags, VmPageDoublyLinkedList* list);
  zx_status_t AllocRange(paddr_t address, size_t count, VmPageDoublyLinkedList* list);
  zx_status_t AllocContiguous(size_t count, uint alloc_flags, uint8_t alignment_log2, paddr_t* pa,
                              VmPageDoublyLinkedList* list);
  void FreePage(vm_page* page, PmmOptDelayReuse delay_reuse = PmmOptDelayReuse::Default);
  void FreeList(VmPageDoublyLinkedList* list,
                PmmOptDelayReuse delay_reuse = PmmOptDelayReuse::Default);

  // Calls the provided function, passing |page| back into it, serialized with any other calls to
  // |AllocLoanedPage|, |BeginFreeLoanedPage| and |FinishFreeLoanedPages|. This allows caller to
  // know that while the |with_page| callback is running there are no in progress calls to these
  // methods, and that the page is not presently in holding object, i.e. it is either fully owned by
  // the PmmNode, or fully owned by an object.
  void WithLoanedPage(vm_page_t* page, fit::inline_function<void(vm_page_t*)> with_page);

  // Allocates a single page from the loaned pages list. The allocated page will always have
  // is_loaned() being true, and must be returned by either FreeLoanedPage or FreeLoanedList. If
  // there are not loaned pages available ZX_ERR_UNAVAILABLE is returned, as an absence of loaned
  // pages does not constitute an out of memory scenario.
  // The provided callback must transition the page into a state such that it has a valid backlink,
  // i.e. it is in the OBJECT state with an owner set, prior to returning.
  // During the execution of the callback the page contents must *not* be modified.
  zx::result<vm_page_t*> AllocLoanedPage(fit::inline_function<void(vm_page_t*), 32> allocated);

  // Begins freeing a loaned page that was previously allocated by AllocLoanPage by moving into a
  // holding object. It is an error to attempt to free a non loaned page. When this method is called
  // the |page| must have a valid backlink (i.e. be in the OBJECT state with an owner set). This
  // backlink should be removed by the |release_page| callback, which is invoked under the loaned
  // pages lock, prior to transition the page into the holding state. The caller *must*, at some
  // point in the future, complete the page freeing process by passing the provided |flph| into a
  // |FinishFreeLoanedPages| call.
  void BeginFreeLoanedPage(vm_page_t* page, fit::inline_function<void(vm_page_t*)> release_page,
                           FreeLoanedPagesHolder& flph);
  // Completes the freeing of any loaned pages in |flph|, after which |flph| is allowed to be
  // destructed. Once this method is called on a given |flph| that object is effectively 'dead' and
  // is not allowed to be passed to any PmmNode methods.
  void FinishFreeLoanedPages(FreeLoanedPagesHolder& flph);

  // Begins freeing multiple pages that were allocated by AllocLoanedPage by moving into a holding
  // object. It is an error to attempt to free any non loaned pages. When this method is called all
  // pages in the array must have a valid backlink (i.e. be in the OBJECT state with an owner set),
  // and the |release_list| method must remove the backlink from all pages, and place them in the
  // provided list in the same order.
  // The caller *must*, at some point in the future, complete the page freeing process by passing
  // the provided |flph| into a |FinishFreeLoanedPages| call.
  void BeginFreeLoanedArray(
      vm_page_t** pages, size_t count,
      fit::inline_function<void(vm_page_t**, size_t, VmPageDoublyLinkedList*)> release_list,
      FreeLoanedPagesHolder& flph);

  void UnwirePage(vm_page* page);

  // Frees all pages in the given list and places them in the loaned state available to be returned
  // from AllocLoanedPage.
  //
  // |delay_reuse| controls whether the newly loaned pages are eligile for immediate or delayed
  // reuse.
  void BeginLoan(VmPageDoublyLinkedList* page_list,
                 PmmOptDelayReuse delay_reuse = PmmOptDelayReuse::Default);

  // Marks a page that had been previously provided to BeginLoan as cancelled. This page may be in
  // the FREE_LOANED state, or presently in use.
  //
  // This call prevents the page from being reused for any new purpose until EndLoan(). For
  // presently-FREE_LOANED pages, this removes the pages from free_loaned_list_. For presently-used
  // pages, this specifies that the page will not be added to free_loaned_list_ when later freed.
  // Once this page is FREE_LOANED (to be ensured by the caller via PhysicalPageProvider reclaim of
  // the pages), the loan can be ended with EndLoan().
  void CancelLoan(vm_page_t* page);

  // Allocates the page to the caller as a regular non-loaned page. Must currently be:
  //  * Loaned (via BeginLoan).
  //  * Have had its loan cancelled (via CancelLoan).
  //  * Be in the FREE_LOANED state.
  void EndLoan(vm_page_t* page);

  // See |pmm_set_free_memory_signal|
  bool SetFreeMemorySignal(uint64_t free_lower_bound, uint64_t free_upper_bound,
                           uint64_t delay_allocations_pages, Event* event);

  // Waits the system to exit low memory state and then attempts to allocate.
  //
  // To prevent herding problem, and because allocation compete for the `PmmNode::lock_` anyway,
  // only one thread is woken up a time, only if the previous thread successfully allocated.
  //
  // In normal conditions,  when the system is in low memory state, this method will return
  // `ZX_ERR_TIMED_OUT` if the system didn't transition fast enough. If we run into a TOC to TOU,
  // for the system race `ZX_ERR_SHOULD_WAIT` will be returned, that is the system transitioned
  // out and back into low memory state before we managed to perform the allocation.
  //
  // If `BootOptions::Get()->pmm_alloc_random_wait` is true, then the system
  // may return spurious `ZX_ERR_SHOULD_WAIT`, in such cases, if the system is
  // not in a low memory state, a thread is woken up anyway, so forward
  // progress can be made.
  //
  // If |suspendable| is true, the wait will terminate early with
  // `ZX_ERR_INTERNAL_INTR_RETRY` if the thread is suspended. If false, suspension is ignored and
  // the wait continues.
  zx::result<vm_page_t*> WaitForSinglePageAllocation(Deadline deadline, bool suspendable);

  void StopReturningShouldWait();

  uint64_t CountFreePages() const;
  uint64_t CountLoanedFreePages() const;
  uint64_t CountLoanCancelledPages() const;
  // Not actually used and cancelled is still not free, since the page can't be allocated in that
  // state.
  uint64_t CountLoanedNotFreePages() const;
  uint64_t CountLoanedPages() const;
  uint64_t CountTotalBytes() const;

  // printf free and overall state of the internal arenas
  // NOTE: both functions skip mutexes and can be called inside timer or crash context
  // though the data they return may be questionable
  void DumpFree() const TA_NO_THREAD_SAFETY_ANALYSIS;
  void Dump(bool is_panic) const TA_NO_THREAD_SAFETY_ANALYSIS;

  // Returns the number of active arenas.
  size_t NumArenas() const {
    Guard<Mutex> guard{&lock_};
    return active_arenas().size();
  }

  // Copies |count| pmm_arena_info_t objects into |buffer| starting with the |i|-th arena ordered by
  // base address.  For example, passing an |i| of 1 would skip the 1st arena.
  //
  // The objects will be sorted in ascending order by arena base address.
  //
  // Returns ZX_ERR_OUT_OF_RANGE if |count| is 0 or |i| and |count| specificy an invalid range.
  //
  // Returns ZX_ERR_BUFFER_TOO_SMALL if the buffer is too small.
  zx_status_t GetArenaInfo(size_t count, uint64_t i, pmm_arena_info_t* buffer, size_t buffer_size);

  // add new pages to the free queue. used when boostrapping a PmmArena
  void AddFreePages(VmPageDoublyLinkedList* list);

  PageQueues* GetPageQueues() { return &page_queues_; }

  // Retrieve any page compression instance. If this returns non-null then it's return value will
  // not change and the result can be cached.
  VmCompression* GetPageCompression() {
    Guard<Mutex> guard{&compression_lock_};
    return page_compression_.get();
  }

  // Set the page compression instance. Returns an error if one has already been set.
  zx_status_t SetPageCompression(fbl::RefPtr<VmCompression> compression);

  // Fill all free pages (both non-loaned and loaned) with a pattern and arm the checker.  See
  // |PmmChecker|.
  //
  // This is a no-op if the checker is not enabled.  See |EnableFreePageFilling|
  void FillFreePagesAndArm();

  // Synchronously walk the PMM's free list (and free loaned list) and validate each page.  This is
  // an incredibly expensive operation and should only be used for debugging purposes.
  void CheckAllFreePages();

#if __has_feature(address_sanitizer)
  // Synchronously walk the PMM's free list (and free loaned list) and poison each page.
  void PoisonAllFreePages();
#endif

  // Enable the free fill checker with the specified fill size and action, and begin filling freed
  // pages (including freed loaned pages) going forward.  See |PmmChecker| for definition of fill
  // size.
  //
  // Note, pages freed piror to calling this method will remain unfilled.  To fill them, call
  // |FillFreePagesAndArm|.
  //
  // Returns true if the checker was enabled with the requested fill_size, or |false| otherwise.
  bool EnableFreePageFilling(size_t fill_size, CheckFailAction action);

  // Return a pointer to this object's free fill checker.
  //
  // For test and diagnostic purposes.
  PmmChecker* Checker() { return &checker_; }

  static int64_t get_alloc_failed_count();

  // See |alloc_failed_no_mem_|.
  bool has_alloc_failed_no_mem() const {
    return alloc_failed_no_mem_.load(ktl::memory_order_relaxed);
  }

  Evictor* GetEvictor() { return &evictor_; }

  // If randomly waiting on allocations is enabled, this re-seeds from the global prng, otherwise it
  // does nothing.
  void SeedRandomShouldWait();

  // Tell this PmmNode that we've failed a user-visible allocation.  Calling this method will
  // (optionally) trigger an asynchronous OOM response. To improve diagnostics some information
  // about the source of the failure can be provided.
  struct AllocFailure {
    enum class Type : uint8_t {
      None,
      Pmm,
      Heap,
      Handle,
      Other,
    };
    static const char* TypeToString(Type type);
    Type type = Type::None;
    size_t size = 0;
    uint64_t free_count = 0;
  };
  void ReportAllocFailure(AllocFailure failure) TA_EXCL(lock_);

  // Retrieves information given to |ReportAllocFailure|. Due to book keeping limitations this will
  // only return information from the first failure.
  AllocFailure GetFirstAllocFailure() TA_EXCL(lock_);

 private:
  zx_status_t InitArena(const PmmArenaSelection& selected);

  // An initialization subroutine to update PMM bookkeeping to reflect that a
  // given range is to be regarded as reserved (e.g., wired). This is used for
  // both special pre-PMM allocations (e.g., the kernel memory image) as well as
  // holes in RAM contained within arenas (indicated by type
  // memalloc::Type::kReserved).
  //
  // `range` is expected to be page-aligned and to not intersect with past
  // ranges provided to this method.
  void InitReservedRange(const memalloc::Range& range);

  fbl::Canary<fbl::magic("PNOD")> canary_;

  mutable DECLARE_MUTEX(PmmNode) lock_;

  [[maybe_unused]] uint64_t arena_cumulative_size_ TA_GUARDED(lock_) = 0;
  // This is both an atomic and guarded by lock_ as we would like modifications to require the lock,
  // as logic in the system relies on the free_count_ not changing whilst the lock is held, but also
  // be an atomic so it can be correctly read without the lock.
  [[maybe_unused]] ktl::atomic<uint64_t> free_count_ TA_GUARDED(lock_) = 0;
  [[maybe_unused]] ktl::atomic<uint64_t> free_loaned_count_ TA_GUARDED(loaned_list_lock_) = 0;
  [[maybe_unused]] ktl::atomic<uint64_t> loaned_count_ TA_GUARDED(loaned_list_lock_) = 0;
  [[maybe_unused]] ktl::atomic<uint64_t> loan_cancelled_count_ TA_GUARDED(loaned_list_lock_) = 0;

  // Free pages where !loaned.
  [[maybe_unused]] VmPageDoublyLinkedList free_list_ TA_GUARDED(lock_);
  // Free pages where loaned && !loan_cancelled.
  mutable DECLARE_MUTEX(PmmNode) loaned_list_lock_;
  [[maybe_unused]] VmPageDoublyLinkedList free_loaned_list_ TA_GUARDED(loaned_list_lock_);

  // The pages comprising the memory temporarily used during phys hand-off,
  // populated on Init(). It is the responsibility of EndHandoff() to free this
  // list.
  [[maybe_unused]] VmPageDoublyLinkedList phys_handoff_temporary_list_;

  // The pages comprising the page-aligned regions of memory that we expect to
  // turn into VMOs to hand-off to userspace - as determined by
  // PhysHandoff::IsPhysVmoType() - populated and marked as wired on Init().
  //
  // It is expected that this memory will be unwired and turned into VMOs by the
  // end of the phys hand-off phase, and it is the responsibility of
  // PmmNode::EndHandoff() to ensure afterward that this list is empty.
  [[maybe_unused]] VmPageDoublyLinkedList phys_handoff_vmo_list_ TA_GUARDED(lock_);

  // The pages intended to be permanently reserved.
  [[maybe_unused]] VmPageDoublyLinkedList permanently_reserved_list_ TA_GUARDED(lock_);

  // Controls the behavior of requests that have the PMM_ALLOC_FLAG_CAN_WAIT.
  enum class ShouldWaitState {
    // The PMM_ALLOC_FLAG_CAN_WAIT should never be followed and we will always attempt to perform
    // the allocation, or fail with ZX_ERR_NO_MEMORY. This state is permanent and cannot be left.
    Never,
    // Allocations do not need to be delayed, but the should_wait_free_pages_level_ should be
    // monitored and once tripped should be delayed.
    OnceLevelTripped,
    // State indicates that the level got tripped, and we should delay any allocations until the
    // level is reset.
    UntilReset,
  };
  [[maybe_unused]] ShouldWaitState should_wait_ TA_GUARDED(lock_) =
      ShouldWaitState::OnceLevelTripped;

  // Below this number of free pages the PMM will transition into delaying allocations.
  [[maybe_unused]] uint64_t should_wait_free_pages_level_ TA_GUARDED(lock_) = 0;

  // The event acts a gate keeper for waking up threads waiting for allocations one at time.
  // The event gets signalled when there MAY be pages available.
  [[maybe_unused]] AutounsignalEvent may_allocate_evt_{true};

  // Indicates whether a PMM alloc call has ever failed with ZX_ERR_NO_MEMORY.  Used to trigger an
  // OOM response.  See |MemoryWatchdog::WorkerThread|.
  ktl::atomic<bool> alloc_failed_no_mem_{false};

  // A record of the first time an allocation failure is reported to aid in diagnostics.
  [[maybe_unused]] AllocFailure first_alloc_failure_ TA_GUARDED(lock_);

  // If mem_signal_ is not null, then once the available free memory falls outside of the defined
  // lower and upper bound the signal is raised. This is a one-shot signal and is cleared after
  // firing.
  [[maybe_unused]] Event* mem_signal_ TA_GUARDED(lock_) = nullptr;
  [[maybe_unused]] uint64_t mem_signal_lower_bound_ TA_GUARDED(lock_) = 0;
  [[maybe_unused]] uint64_t mem_signal_upper_bound_ TA_GUARDED(lock_) = 0;

  PageQueues page_queues_;

  Evictor evictor_;

  // The page_compression_ is a lazily initialized RefPtr to keep the PmmNode constructor simple, at
  // the cost needing to hold a lock to read the RefPtr. To avoid unnecessarily contending on the
  // main pmm lock_, use a separate one.
  DECLARE_MUTEX(PmmNode) compression_lock_;
  fbl::RefPtr<VmCompression> page_compression_ TA_GUARDED(compression_lock_);

  // Indicates whether pages should have a pattern filled into them when they are freed. This value
  // can only transition from false->true, and never back to false again. Once this value is set,
  // the fill size in checker_ may no longer be changed, and it becomes safe to call FillPattern
  // even without the lock held.
  // This is an atomic to allow for reading this outside of the lock, but modifications only happen
  // with the lock held.
  [[maybe_unused]] ktl::atomic<bool> free_fill_enabled_ TA_GUARDED(lock_)
      TA_GUARDED(loaned_list_lock_) = false;
  // Indicates whether it is known that all pages in the free list have had a pattern filled into
  // them. This value can only transition from false->true, and never back to false again. Once this
  // value is set the action and armed state in checker_ may no longer be changed, and it becomes
  // safe to call AssertPattern even without the lock held.
  [[maybe_unused]] bool all_free_pages_filled_ TA_GUARDED(loaned_list_lock_)
      TA_GUARDED(lock_) = false;
  PmmChecker checker_;

  // The rng state for random waiting on allocations. This allows us to use rand_r, which requires
  // no further thread synchronization, unlike rand().
  [[maybe_unused]] uintptr_t random_should_wait_seed_ TA_GUARDED(lock_) = 0;

  // Arenas are allocated from the node itself to avoid any boot allocations. Walking linearly
  // through them at run time should also be fairly efficient.
  static constexpr size_t kArenaCount = 16;

  // Bit constants to fold a vm_page_t pointer into a uint32 and back. The format from LSB
  // to MSB is : zero-bits | arena-index | page-index |. Where arena-index is 4 bits wide and
  // zero-bits is 3 bits wide. This limits the number of pages per arena to 2^25.
  static constexpr int kArenaBits = 4;
  static_assert(kArenaCount == (1ul << kArenaBits));
  static constexpr size_t kMaxPagesPerArena = 1u << (32u - (kArenaBits + kIndexZeroBits));
  static constexpr uint32_t kArenaMask = (1u << kArenaBits) - 1u;

  size_t used_arena_count_ TA_GUARDED(lock_) = 0;
  PmmArena arenas_[kArenaCount] TA_GUARDED(lock_);

  // Return the span of arenas from the built-in array that are known to be active. Used in loops
  // that iterate across all arenas.
  ktl::span<PmmArena> active_arenas() TA_REQ(lock_) {
    return ktl::span<PmmArena>(arenas_, used_arena_count_);
  }
  ktl::span<const PmmArena> active_arenas() const TA_REQ(lock_) {
    return ktl::span<const PmmArena>(arenas_, used_arena_count_);
  }
};

// We don't need to hold the arena lock while executing this, since it is
// only accesses values that are set once during system initialization.
inline vm_page_t* PmmNode::PaddrToPage(paddr_t addr) TA_NO_THREAD_SAFETY_ANALYSIS {
  for (auto& a : active_arenas()) {
    if (a.address_in_arena(addr)) {
      size_t index = (addr - a.base()) / kPageSize;
      return a.get_page(index);
    }
  }
  return nullptr;
}

// Same locking requirements as PaddrToPage().
inline vm_page_t* PmmNode::IndexToPage(uint32_t index) TA_NO_THREAD_SAFETY_ANALYSIS {
  index = index >> kIndexZeroBits;
  uint32_t arena_ix = index & kArenaMask;
  uint32_t page_ix = (index >> kArenaBits);
  return active_arenas()[arena_ix].get_page(page_ix - 1);
}

// Same locking requirements as PaddrToPage().
inline paddr_t PmmNode::IndexToPaddr(uint32_t index) TA_NO_THREAD_SAFETY_ANALYSIS {
  index = index >> kIndexZeroBits;
  uint32_t arena_ix = index & kArenaMask;
  uint32_t page_ix = (index >> kArenaBits);
  return active_arenas()[arena_ix].base() + (static_cast<uint64_t>(page_ix - 1) * kPageSize);
}

// Same locking requirements as PaddrToPage().
inline uint32_t PmmNode::PageToIndex(const vm_page_t* page) TA_NO_THREAD_SAFETY_ANALYSIS {
  uint32_t arena_ix = 0;
  for (auto& a : active_arenas()) {
    if (a.page_belongs_to_arena(page)) {
      auto page_ix = static_cast<uint32_t>(a.get_index(page) + 1);
      return ((page_ix << kArenaBits) | arena_ix) << kIndexZeroBits;
    }
    arena_ix += 1;
  }
  return 0;
}

// Object for managing freeing of loaned pages via a temporary holding object. Can be instantiated
// on the stack and then passed into different PmmNode methods, the object itself has no publicly
// available methods.
// This object is not thread safe, and multiple threads must not pass the same instance of this
// object into PmmNode methods.
// A given FreeLoanedPagesHolder, as described in |FinishFreeLoanedPages|, may only be used for a
// single call to |FinishFreeLoanedPages|, after which it is 'dead', and may not be passed to any
// other PmmNode methods.
class FreeLoanedPagesHolder {
 public:
  FreeLoanedPagesHolder() = default;
  ~FreeLoanedPagesHolder() {
    ASSERT(pages_.is_empty());
    ASSERT(waiters_.is_empty());
  }

 private:
  // A given FreeLoanedPagesHolder interval is only allowed to be used once to return pages to the
  // PMM, this tracks whether this has happened or not.
  // Only permitting a single instance of freeing simplifies any need to reason about a single
  // FreeLoanedPagesHolder repeatedly having pages moved into it and free'd to the PMM concurrently
  // with attempts to wait on it.
  // Although the lock cannot be annotated, this member is guarded by the relevant
  // PmmNode::loaned_list_lock_.
  [[maybe_unused]] bool used_ = false;
  // List of pages presently owned by this object. Every page in this list is defined to be in the
  // ALLOC state with |owner| set to this object.
  // Although the lock cannot be annotated, this member is guarded by the relevant
  // PmmNode::loaned_list_lock_.
  [[maybe_unused]] VmPageDoublyLinkedList pages_;

  struct Waiter : fbl::SinglyLinkedListable<Waiter*> {
    Event event;
  };
  // Maintain a list of waiters to be notified once pages_ have been freed. The Waiter object itself
  // is stack allocated in the WithLoanedPage method and registered into this list. Having this be a
  // list of Events of single waiting thread, instead of a single Event with a list of waiting
  // threads, allows the waiters to retain a reference to the FLPH object while waiting. This
  // ensures that once FinishFreeLoanedPages performs the signal on the waiters, the FLPH object can
  // be safely destroyed.
  [[maybe_unused]] fbl::SinglyLinkedList<Waiter*> waiters_;
  friend PmmNode;
};

#endif  // ZIRCON_KERNEL_VM_INCLUDE_VM_PMM_NODE_H_
