// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_PAGE_CACHE_INCLUDE_LIB_PAGE_CACHE_H_
#define ZIRCON_KERNEL_LIB_PAGE_CACHE_INCLUDE_LIB_PAGE_CACHE_H_

#include <inttypes.h>
#include <lib/ktrace.h>
#include <lib/zx/result.h>
#include <trace.h>
#include <zircon/compiler.h>
#include <zircon/errors.h>

#include <new>

#include <arch/ops.h>
#include <fbl/alloc_checker.h>
#include <kernel/auto_preempt_disabler.h>
#include <kernel/lockdep.h>
#include <kernel/mutex.h>
#include <ktl/unique_ptr.h>
#include <ktl/utility.h>
#include <vm/page_state.h>
#include <vm/physmap.h>
#include <vm/pmm.h>

// PageCache provides a front end to the PMM that reserves a given number of
// pages in per-CPU caches to reduce contention on the PMM.
//
// TODO(https://fxbug.dev/42147478): Add support for KASAN.
// TODO(https://fxbug.dev/42147480): Flush page caches when CPUs go offline.
//

namespace page_cache {

class PageCache {
  static constexpr bool kTraceEnabled = false;

 public:
  // Creates a page cache with the given number of reserve pages per CPU.
  // Filling the per-CPU page caches is deferred until the first allocation
  // request.
  static zx::result<PageCache> Create(size_t reserve_pages);

  PageCache() = default;
  ~PageCache() = default;

  // PageCache is not copiable.
  PageCache(const PageCache&) = delete;
  PageCache& operator=(const PageCache&) = delete;

  // PageCache is movable.
  PageCache(PageCache&&) = default;
  PageCache& operator=(PageCache&&) = default;

  // Returns true if this PageCache instance is non-empty.
  explicit operator bool() const { return !!per_cpu_caches_; }

  // Utility type for returning a list of pages via zx::result. Automatically
  // frees a non-empty list of pages on destruction to improve safety and
  // ergonomics.
  struct PageList : VmPageDoublyLinkedList {
    PageList() = default;

    ~PageList() {
      if (!is_empty()) {
        pmm_free(this);
      }
    }

    PageList(VmPageDoublyLinkedList&& other) { splice(end(), other); }

    PageList(PageList&& other) noexcept { splice(end(), other); }

    PageList& operator=(PageList&& other) noexcept {
      if (!is_empty()) {
        pmm_free(this);
      }
      splice(end(), other);
      return *this;
    }

    PageList(const PageList&) = delete;
    PageList& operator=(const PageList&) = delete;
  };

  struct AllocateResult {
    // The list of pages allocated by the request.
    PageList page_list{};

    // The number of pages remaining in the cache after the request.
    size_t available_pages{0};
  };

  // Allocates the given number of pages from the page cache. Falls back to the
  // PMM if the cache is insufficient to fulfill the request.
  zx::result<AllocateResult> Allocate(size_t page_count, uint alloc_flags = 0) {
    ktrace::Scope trace =
        KTRACE_BEGIN_SCOPE_ENABLE(kTraceEnabled, "kernel:sched", "PageCache::Allocate",
                                  ("page_count", page_count), ("alloc_flags", alloc_flags));
    DEBUG_ASSERT(Thread::Current::memory_allocation_state().IsEnabled());
    DEBUG_ASSERT(per_cpu_caches_);

    AutoPreemptDisabler preempt_disable;
    const cpu_num_t current_cpu = arch_curr_cpu_num();
    return Allocate(per_cpu_caches_[current_cpu], page_count, alloc_flags);
  }

  // Returns the given pages to the page cache. Excess pages are returned to the
  // PMM.
  void Free(PageList page_list) {
    ktrace::Scope trace =
        KTRACE_BEGIN_SCOPE_ENABLE(kTraceEnabled, "kernel:sched", "PageCache::Free");
    DEBUG_ASSERT(per_cpu_caches_);

    if (!page_list.is_empty()) {
      // Note that preempt_disable is destroyed before page_list, intentionally
      // resulting in excess pages being freed outside of this local preemption
      // disablement.
      AutoPreemptDisabler preempt_disable;
      const cpu_num_t current_cpu = arch_curr_cpu_num();
      Free(per_cpu_caches_[current_cpu], &page_list);
    }
  }

  size_t reserve_pages() const { return reserve_pages_; }

  void SeedRandomShouldWait();

 private:
  struct alignas(MAX_CACHE_LINE) CpuCache {
    DECLARE_MUTEX(CpuCache) fill_lock;
    DECLARE_MUTEX(CpuCache) cache_lock;

    TA_GUARDED(cache_lock)
    size_t available_pages{0};

    TA_GUARDED(cache_lock)
    PageList free_list;

    TA_GUARDED(cache_lock)
    ktl::optional<uintptr_t> random_should_wait_state = ktl::nullopt;
  };

  explicit PageCache(size_t reserve_pages, fbl::Array<CpuCache> entries)
      : reserve_pages_{reserve_pages}, per_cpu_caches_{ktl::move(entries)} {}

  static void CountHitPages(size_t page_count);
  static void CountMissPages(size_t page_count);
  static void CountRefillPages(size_t page_count);
  static void CountReturnPages(size_t page_count);
  static void CountFreePages(size_t page_count);

  // Attempts to allocate the given number of pages from the CPU cache. If the
  // cache is insufficient for the request, falls back to the PMM to fulfill the
  // request and refill the cache. The requested number of pages may be zero, in
  // which case only the cache is filled.
  zx::result<AllocateResult> Allocate(CpuCache& entry, size_t requested_pages, uint alloc_flags)
      TA_EXCL(entry.fill_lock, entry.cache_lock) {
    if (requested_pages > 0) {
      Guard<Mutex> guard{&entry.cache_lock};
      // If the allocation is waitable, check if user wants random should waits, and if so randomly
      // error 10% of allocation requests.
      if ((alloc_flags & PMM_ALLOC_FLAG_CAN_WAIT) && entry.random_should_wait_state.has_value() &&
          rand_r(&*entry.random_should_wait_state) < (RAND_MAX / 10)) {
        return zx::error{ZX_ERR_SHOULD_WAIT};
      }
      if (requested_pages <= entry.available_pages) {
        return zx::ok(AllocateCachePages(entry, requested_pages));
      }
    }
    return AllocatePagesAndFillCache(entry, requested_pages, alloc_flags);
  }

  // Allocates the given number of pages from the given CPU cache.
  static AllocateResult AllocateCachePages(CpuCache& entry, size_t requested_pages)
      TA_REQ(entry.cache_lock) {
    DEBUG_ASSERT(requested_pages <= entry.available_pages);
    DEBUG_ASSERT(requested_pages > 0);

    entry.available_pages -= requested_pages;
    PageList return_pages;
    for (size_t i = 0; i < requested_pages; i++) {
      vm_page_t* page = entry.free_list.pop_back();
      DEBUG_ASSERT(page);
      page->set_state(vm_page_state::ALLOC);
      return_pages.push_front(page);
    }

    CountHitPages(requested_pages);

    return AllocateResult{ktl::move(return_pages), entry.available_pages};
  }

  // Returns the given list of pages to the given CPU cache, returning excess
  // pages to the PMM.
  void Free(CpuCache& entry, PageList* page_list) const TA_EXCL(entry.fill_lock, entry.cache_lock) {
    Guard<Mutex> guard{&entry.cache_lock};

    size_t free_count = 0;
    size_t return_count = 0;
    PageList return_list;

    // Filter out non-borrowed pages to return to the free list. Pages remaining
    // in page_list are freed to the PMM outside the lock in the PageList dtor.
    auto iter = page_list->begin();
    while (iter != page_list->end()) {
      auto& page = *iter;
      auto next = iter;
      next++;

      DEBUG_ASSERT(!page.is_loaned());
      if (entry.available_pages < reserve_pages_) {
        page.set_state(vm_page_state::CACHE);
        page_list->erase(page);
        return_list.push_back(&page);

        entry.available_pages++;
        return_count++;
      } else {
        free_count++;
      }
      iter = next;
    }

    // Return the selected pages to the free list.
    entry.free_list.splice(entry.free_list.begin(), return_list);
    CountReturnPages(return_count);
    CountFreePages(free_count);
  }

  zx::result<AllocateResult> AllocatePagesAndFillCache(CpuCache& entry, size_t requested_pages,
                                                       uint alloc_flags) const
      TA_EXCL(entry.fill_lock, entry.cache_lock) {
    ktrace::Scope trace = KTRACE_BEGIN_SCOPE_ENABLE(
        kTraceEnabled, "kernel:sched", "PageCache::AllocatePagesAndFillCache",
        ("requested_pages", requested_pages), ("alloc_flags", alloc_flags));

    // Serialize cache fill + allocate operations on this cache. Contention
    // means another thread tried to allocate from the PMM and blocked on the
    // PMM lock. There's no benefit to following the owning thread into the PMM
    // allocator, by the time this lock is released the cache may have enough
    // pages to satisfy this request without falling back to the PMM again.
    Guard<Mutex> fill_guard{&entry.fill_lock};

    // Acquire the cache lock after the fill lock. If this thread blocked on the
    // previous lock, there is a chance this thread is now running on a
    // different CPU. However, there's also a good chance the cache is now
    // sufficient to fulfill the request without falling back to the PMM.
    Guard<Mutex> cache_guard{&entry.cache_lock};

    PageList return_list;

    // Re-validate the request after acquiring the locks. Another thread may
    // have filled the cache sufficiently already.
    if (requested_pages > entry.available_pages) {
      const size_t refill_pages = reserve_pages_ - entry.available_pages;
      const size_t total_pages = requested_pages + refill_pages;

      CountRefillPages(refill_pages);
      CountMissPages(requested_pages);

      VmPageDoublyLinkedList page_list;
      zx_status_t status;

      // Release the cache lock while calling into the PMM to permit other
      // threads to access the cache if this thread blocks.
      cache_guard.CallUnlocked([total_pages, alloc_flags, &page_list, &status]() {
        size_t pages = total_pages;
        status = pmm_alloc_pages(pages, alloc_flags, &page_list);
        // If the alloc_flags contained PMM_ALLOC_FLAG_CAN_WAIT then could get ZX_ERR_SHOULD_WAIT as
        // a return code. In this case, as per the documentation of pmm_alloc_pages, we must fall
        // back to attempting to allocate single pages.
        if (status == ZX_ERR_SHOULD_WAIT) {
          vm_page_t* page = nullptr;
          while (pages > 0 && (status = pmm_alloc_page(alloc_flags, &page)) == ZX_OK) {
            page_list.push_back(page);
            pages--;
          }
        }
      });
      if (status != ZX_OK) {
        // Even if allocating all the pages failed, may have received a few. If so, put them into
        // the cache and then see if we have enough for the actual requested allocation. Since this
        // is an uncommon edge cases do not try and be too optimal with the implementation.
        while (!page_list.is_empty()) {
          vm_page_t* page = page_list.pop_front();
          page->set_state(vm_page_state::CACHE);
          entry.free_list.push_back(page);
          entry.available_pages++;
        }
        if (requested_pages <= entry.available_pages) {
          return zx::ok(AllocateCachePages(entry, requested_pages));
        }
        return zx::error_result(status);
      }

      // Set the page state of the refill pages and find the end of the refill list.
      if (refill_pages > 0) {
        auto node = page_list.begin();
        for (size_t i = 0; i < refill_pages; i++, node++) {
          vm_page_t* page = &*node;
          DEBUG_ASSERT(page);
          page->set_state(vm_page_state::CACHE);
        }
        node--;

        // Separate the return list from the refill list and add the latter to the
        // free list.
        return_list = page_list.split_after(node);
      } else {
        return_list = ktl::move(page_list);
      }
      entry.free_list.splice(entry.free_list.begin(), page_list);

      entry.available_pages += refill_pages;
    } else if (requested_pages > 0) {
      return zx::ok(AllocateCachePages(entry, requested_pages));
    }

    return zx::ok(AllocateResult{ktl::move(return_list), entry.available_pages});
  }

  size_t reserve_pages_{0};
  fbl::Array<CpuCache> per_cpu_caches_;
};

}  // namespace page_cache

#endif  // ZIRCON_KERNEL_LIB_PAGE_CACHE_INCLUDE_LIB_PAGE_CACHE_H_
