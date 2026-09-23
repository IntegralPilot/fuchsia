// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/intrin.h>
#include <lib/counters.h>
#include <lib/fit/defer.h>
#include <lib/zircon-internal/macros.h>

#include <fbl/ref_counted_upgradeable.h>
#include <kernel/auto_preempt_disabler.h>
#include <object/thread_dispatcher.h>
#include <vm/compression.h>
#include <vm/page.h>
#include <vm/page_queues.h>
#include <vm/pmm.h>
#include <vm/scanner.h>
#include <vm/vm_cow_pages.h>

namespace {

KCOUNTER(pq_aging_reason_timeout, "pq.aging.reason.timeout")
KCOUNTER(pq_aging_reason_active_ratio, "pq.aging.reason.active_ratio")
KCOUNTER(pq_aging_reason_manual, "pq.aging.reason.manual")
KCOUNTER(pq_lru_spurious_wakeup, "pq.lru.spurious_wakeup")
KCOUNTER(pq_lru_pages_evicted, "pq.lru.pages_evicted")
KCOUNTER(pq_lru_pages_compressed, "pq.lru.pages_compressed")
KCOUNTER(pq_lru_pages_discarded, "pq.lru.pages_discarded")
KCOUNTER(pq_lru_sweeping_skipped_unloan, "pq.lru.sweeping_skipped_unloan")
KCOUNTER(pq_accessed_normal, "pq.accessed.normal")
KCOUNTER(pq_accessed_normal_same_queue, "pq.accessed.normal_same_queue")
KCOUNTER(pq_accessed_isolate, "pq.accessed.isolate")
KCOUNTER(pq_accessed_not_reclaim, "pq.accessed.not_reclaim")
KCOUNTER(pq_peek_sync_aging, "pq.peek.sync_aging")
KCOUNTER(pq_peek_process_lru, "pq.peek.process_lru")
KCOUNTER(pq_peek_sync_aging_all_inactive, "pq.peek.sync_aging.all_inactive")
KCOUNTER(pq_peek_process_lru_all_inactive, "pq.peek.process_lru.all_inactive")

// Helper class for building an isolate list for deferred processing when acting on the LRU queues.
// Pages are added while the page queues lock is held, and processed once the lock is dropped.
// Statically sized with the maximum number of items it might need to hold and it is an error to
// attempt to add more than this many items, as Flush() cannot automatically be called due to
// incompatible locking requirements between flushing and adding items.
template <size_t Items>
class LruIsolate {
 public:
  using LruAction = PageQueues::LruAction;
  LruIsolate() = default;
  ~LruIsolate() { Flush(); }
  // Sets the LRU action, this allows the object construction to happen without the page queues
  // lock, where as setting the LruAction can be done within it.
  void SetLruAction(LruAction lru_action) { lru_action_ = lru_action; }

  // Adds a page to be potentially replaced with a loaned page.
  // Requires PageQueues lock to be held
  void AddLoanReplacement(vm_page_t* page, PageQueues* pq) TA_REQ(pq->get_lock()) {
    DEBUG_ASSERT(page);
    DEBUG_ASSERT(!page->is_loaned());
    VmCowPages* cow = reinterpret_cast<VmCowPages*>(page->object.get_object());
    DEBUG_ASSERT(cow);
    // If the VMO does not support borrowing then skip enqueuing the RefPtr, since the
    // ReplacePageWithLoaned can never succeed anyway.
    if (!cow->can_borrow()) {
      return;
    }
    fbl::RefPtr<VmCowPages> cow_ref = fbl::MakeRefPtrUpgradeFromRaw(cow, pq->get_lock());
    DEBUG_ASSERT(cow_ref);
    AddInternal(ktl::move(cow_ref), page, ListAction::ReplaceWithLoaned);
  }

  // Add a page to be reclaimed. Actual reclamation will only be done if the `SetLruAction` is
  // compatible with the page and its VMO owner.
  // Requires PageQueues lock to be held
  void AddReclaimable(vm_page_t* page, PageQueues* pq) TA_REQ(pq->get_lock()) {
    DEBUG_ASSERT(page);
    if (lru_action_ == LruAction::None) {
      return;
    }
    VmCowPages* cow = reinterpret_cast<VmCowPages*>(page->object.get_object());
    DEBUG_ASSERT(cow);
    // Need to get the cow refptr before we can check if our lru action is appropriate for this
    // page.
    fbl::RefPtr<VmCowPages> cow_ref = fbl::MakeRefPtrUpgradeFromRaw(cow, pq->get_lock());
    DEBUG_ASSERT(cow_ref);
    if (lru_action_ == LruAction::EvictAndCompress ||
        ((cow_ref->can_evict() || cow_ref->is_discardable()) ==
         (lru_action_ == LruAction::EvictOnly))) {
      AddInternal(ktl::move(cow_ref), page, ListAction::Reclaim);
    } else {
      // Must not let the cow refptr get dropped till after the lock, so even if not
      // reclaiming must keep this entry.
      AddInternal(ktl::move(cow_ref), page, ListAction::None);
    }
  }

  // Checks if there is space for additional pages to be added. If |true| the Add* methods must not
  // be held.
  bool Full() { return items_ == list_.size(); }

  // Performs any pending operations on the stored pages.
  // Requires PageQueues lock NOT be held
  void Flush() {
    // Cannot check if the page queues lock specifically is held, but can validate that *no*
    // spinlocks at all are held, which also needs to be true for us to acquire VMO locks.
    DEBUG_ASSERT(arch_num_spinlocks_held() == 0);
    // Compression state will be lazily instantiate if needed, and then used for any remaining
    // pages in the list.
    VmCompression* compression = nullptr;
    ktl::optional<VmCompression::CompressorGuard> maybe_compressor;
    VmCompressor* compressor = nullptr;

    for (size_t i = 0; i < items_; ++i) {
      auto [backlink, action] = ktl::move(list_[i]);
      DEBUG_ASSERT(backlink.cow);
      if (action == ListAction::ReplaceWithLoaned) {
        // Only replace with a loaned page if background borrowing/sweeping is still active.
        // We avoid calling ReplacePageWithLoaned if active unloans are in progress to prevent
        // lock contention on paged_vmo_lock_ and redundant borrowing/reclaiming work.
        // Note that checking is_borrowing_active() during the initial sweep logic in
        // ProcessLruQueue is not sufficient because an unloan can start in the window after
        // ProcessLruQueue evaluates the flag but before we flush the deferred list.
        if (PhysicalPageBorrowingConfig::Get().is_borrowing_active()) {
          // We ignore the return value because the page may have moved, become pinned, we may not
          // have any free loaned pages any more, or the VmCowPages may not be able to borrow.
          backlink.cow->ReplacePageWithLoaned(backlink.page, backlink.offset);
        }
      } else if (action == ListAction::Reclaim) {
        // Attempt to acquire any compressor that might exist, unless only evicting. Note that if
        // LruAction::None we would not have enqueued any Reclaim pages, so we can just check for
        // EvictOnly.
        if (lru_action_ != LruAction::EvictOnly && !compression) {
          compression = Pmm::Node().GetPageCompression();
          if (compression) {
            maybe_compressor.emplace(compression->AcquireCompressor());
            compressor = &maybe_compressor->get();
          }
        }
        // If using a compressor, make sure it is Armed between reclamations.
        if (compressor) {
          zx_status_t status = compressor->Arm();
          if (status != ZX_OK) {
            // Continue processing as we might still be able to evict and we need to clear all the
            // refptrs as well.
            continue;
          }
        }
        VmCowReclaimResult reclaimed = backlink.cow->ReclaimPage(
            backlink.page, backlink.offset, VmCowPages::EvictionAction::FollowHint, compressor);
        if (reclaimed.is_ok()) {
          uint64_t num_pages = reclaimed.value().num_pages;
          uint64_t num_loaned_pages = reclaimed.value().num_loaned_pages;

          if (num_pages > 0 || num_loaned_pages > 0) {
            switch (reclaimed.value().type) {
              case VmCowReclaimSuccess::Type::Evict:
                pq_lru_pages_evicted.Add(num_pages + num_loaned_pages);
                break;
              case VmCowReclaimSuccess::Type::Discard:
                DEBUG_ASSERT(num_loaned_pages == 0);
                pq_lru_pages_discarded.Add(num_pages);
                break;
              case VmCowReclaimSuccess::Type::Compress:
                DEBUG_ASSERT(num_loaned_pages == 0);
                pq_lru_pages_compressed.Add(num_pages);
                break;
            }
          }
        }
      }
    }
    items_ = 0;
  }

 private:
  // The None is needed since to know if a page can be reclaimed by the current LruAction a RefPtr
  // to the VMO must first be created. If the page shouldn't be reclaimed the RefPtr must not be
  // dropped till outside the lock, in case it's the last ref. The None action provides a way to
  // retain these RefPtrs and have them dropped outside the lock.
  enum class ListAction {
    None,
    ReplaceWithLoaned,
    Reclaim,
  };

  void AddInternal(fbl::RefPtr<VmCowPages>&& cow, vm_page_t* page, ListAction action) {
    DEBUG_ASSERT(cow);
    DEBUG_ASSERT(!Full());
    if (cow) {
      list_[items_] = {PageQueues::VmoBacklink{cow, page, page->object.get_page_offset()}, action};
      items_++;
    }
  }

  // Cache of the PageQueues LruAction for checking what to do with different reclaimable pages.
  LruAction lru_action_ = LruAction::None;
  // List of pages and the actions to perform on them.
  ktl::array<ktl::pair<PageQueues::VmoBacklink, ListAction>, Items> list_;
  // Number of items in the list_.
  size_t items_ = 0;
};

}  // namespace

// static
uint64_t PageQueues::GetLruPagesCompressed() { return pq_lru_pages_compressed.SumAcrossAllCpus(); }

PageQueues::PageQueues()
    : min_mru_rotate_time_(kDefaultMinMruRotateTime),
      max_mru_rotate_time_(kDefaultMaxMruRotateTime),
      active_ratio_multiplier_(kDefaultActiveRatioMultiplier) {}

PageQueues::~PageQueues() {
  StopThreads();
  for (uint32_t i = 0; i < PageQueueNumQueues; i++) {
    DEBUG_ASSERT(page_queues_[i].is_empty());
  }
  for (size_t i = 0; i < page_queue_counts_.size(); i++) {
    DEBUG_ASSERT_MSG(page_queue_counts_[i] == 0, "i=%zu count=%zu", i,
                     page_queue_counts_[i].load());
  }
}

void PageQueues::StartThreads(zx_duration_mono_t min_mru_rotate_time,
                              zx_duration_mono_t max_mru_rotate_time) {
  // Clamp the max rotate to the minimum.
  max_mru_rotate_time = ktl::max(min_mru_rotate_time, max_mru_rotate_time);
  // Prevent a rotation rate that is too small.
  max_mru_rotate_time = ktl::max(max_mru_rotate_time, ZX_SEC(1));

  min_mru_rotate_time_ = min_mru_rotate_time;
  max_mru_rotate_time_ = max_mru_rotate_time;

  // Cannot perform all of thread creation under the lock as thread creation requires
  // allocations so we create in temporaries first and then stash.
  Thread* mru_thread = Thread::Create(
      "page-queue-mru-thread",
      [](void* arg) -> int {
        static_cast<PageQueues*>(arg)->MruThread();
        return 0;
      },
      this, LOW_PRIORITY);
  DEBUG_ASSERT(mru_thread);

  mru_thread->Resume();

  Thread* lru_thread = Thread::Create(
      "page-queue-lru-thread",
      [](void* arg) -> int {
        static_cast<PageQueues*>(arg)->LruThread();
        return 0;
      },
      this, LOW_PRIORITY);
  DEBUG_ASSERT(lru_thread);
  lru_thread->Resume();

  {
    Guard<CriticalMutex> guard{&lock_};
    ASSERT(!mru_thread_);
    ASSERT(!lru_thread_);
    mru_thread_ = mru_thread;
    lru_thread_ = lru_thread;
    // Kick start any LRU processing that might be pending to ensure it doesn't spuriously timeout.
    MaybeTriggerLruProcessingLocked();
  }
}

void PageQueues::StartDebugCompressor() {
  // The debug compressor should not be enabled without debug asserts as we guard all usages of the
  // debug compressor with compile time checks so that it cannot impact the performance of release
  // versions.
  ASSERT(DEBUG_ASSERT_IMPLEMENTED);
#if DEBUG_ASSERT_IMPLEMENTED
  fbl::AllocChecker ac;
  ktl::unique_ptr<VmDebugCompressor> dc(new (&ac) VmDebugCompressor);
  if (!ac.check()) {
    panic("Failed to allocate VmDebugCompressor");
  }
  zx_status_t status = dc->Init();
  ASSERT(status == ZX_OK);
  Guard<CriticalMutex> guard{&list_lock_};
  // We should only be initializing the debug compressor once.
  DEBUG_ASSERT(!debug_compressor_);
  debug_compressor_ = ktl::move(dc);
#endif
}

void PageQueues::StopThreads() {
  // Cannot wait for threads to complete with the lock held, so update state and then perform any
  // joins outside the lock.
  Thread* mru_thread = nullptr;
  Thread* lru_thread = nullptr;

  {
    Guard<CriticalMutex> guard{&lock_};
    shutdown_threads_ = true;
    mru_thread = mru_thread_;
    lru_thread = lru_thread_;
    mru_event_.Signal();
    lru_event_.Signal();
  }

  int retcode;
  if (mru_thread) {
    zx_status_t status = mru_thread->Join(&retcode, ZX_TIME_INFINITE);
    ASSERT(status == ZX_OK);
  }
  if (lru_thread) {
    zx_status_t status = lru_thread->Join(&retcode, ZX_TIME_INFINITE);
    ASSERT(status == ZX_OK);
  }
}

void PageQueues::SetLruAction(LruAction action) {
  Guard<CriticalMutex> guard{&lock_};
  lru_action_ = action;
}

void PageQueues::SetActiveRatioMultiplier(uint32_t multiplier) {
  Guard<CriticalMutex> guard{&lock_};
  active_ratio_multiplier_ = multiplier;
  // The change in multiplier might have caused us to need to age.
  CheckActiveRatioAgingLocked();
}

void PageQueues::CheckActiveRatioAgingLocked() {
  if (active_ratio_triggered_) {
    // Already triggered, nothing more to do.
    return;
  }
  if (IsActiveRatioTriggeringAging()) {
    active_ratio_triggered_ = true;
    mru_event_.Signal();
  }
}

bool PageQueues::IsActiveRatioTriggeringAging() {
  ActiveInactiveCounts counts = GetActiveInactiveCounts();
  return counts.active * active_ratio_multiplier_ > counts.inactive;
}

void PageQueues::SynchronizeWithAging() {
  for (int iterations = 1;; iterations++) {
    // Typically this loop should exit on its second iteration at most, since it might take two
    // calls to `TryAgingLocked` before the MRU generation is incremented, at which point the LRU
    // gen can be incremented. The exception to this is if racing with another concurrent
    // modifications to the isolate list, such as by another reclamation thread. Still, it is
    // incredibly unlikely to race such that other threads can completely depopulate both the LRU
    // queues and the isolate list so if we see many instances of such an incredibly unlikely race
    // then there's probably a bug and so generate a warning message.
    if (iterations % 20 == 0) {
      printf("[pq]: Warning %s iterated %d times\n", __FUNCTION__, iterations);
    }
    Guard<CriticalMutex> guard{&lock_};
    // If the LRU queue can be processed, then aging already happened and there's no need to wait
    // for it.
    if (CanIncrementLruGenLocked()) {
      return;
    }
    // Check for races with LRU processing that might have already populated the isolate list.
    if (page_queue_counts_[PageQueueReclaimIsolate].load(ktl::memory_order_relaxed) > 0) {
      return;
    }
    auto reason = GetAgeReasonLocked();
    // If there's no pending age reason then we are not going to wait, so return.
    if (ktl::get_if<zx_instant_mono_t>(&reason)) {
      return;
    }
    // Attempt to perform any aging, and then go around the loop to check if we synchronized.
    TryAgingLocked(ktl::get<AgeReason>(reason), guard.take());
  }
}

ktl::variant<PageQueues::AgeReason, zx_instant_mono_t> PageQueues::GetAgeReasonLocked() const {
  const zx_instant_mono_t current = current_mono_time();
  if (active_ratio_triggered_) {
    // Need to have passed the min time though.
    const zx_instant_mono_t min_timeout =
        zx_time_add_duration(last_age_time_.load(ktl::memory_order_relaxed), min_mru_rotate_time_);
    if (current < min_timeout) {
      return min_timeout;
    }
    // At least min time has elapsed, can age via active ratio.
    return AgeReason::ActiveRatio;
  }

  // Exceeding the maximum time forces aging.
  const zx_instant_mono_t max_timeout =
      zx_time_add_duration(last_age_time_.load(ktl::memory_order_relaxed), max_mru_rotate_time_);
  if (max_timeout <= current) {
    return AgeReason::Timeout;
  }
  // With no other reason, we will age once we hit the maximum timeout.
  return max_timeout;
}

void PageQueues::MaybeTriggerLruProcessingLocked() {
  bool needs_lru_processing = NeedsLruProcessingLocked();
  if (needs_lru_processing) {
    lru_event_.Signal();
  }
}

bool PageQueues::NeedsLruProcessingLocked() const {
  // Currently only reason to trigger lru processing is if the MRU needs space. This requires the
  // lock since the typical use case wants an ordering of any changes to the generation counts with
  // respect to this query. This works since the generation counts are also modified with the lock
  // held.
  if (mru_gen_.load(ktl::memory_order_relaxed) - lru_gen_.load(ktl::memory_order_relaxed) ==
      kNumReclaim - 1) {
    return true;
  }
  return false;
}

void PageQueues::DisableAging() {
  {
    // Take the lock_ over disabling aging to ensure the MruThread is not presently aging and will
    // therefore observe the aging_disabled_ flag next time it runs.
    Guard<CriticalMutex> guard{&lock_};
    // Validate a double DisableAging is not happening.
    if (aging_disabled_) {
      panic("Mismatched disable/enable pair");
    }
    aging_disabled_ = true;
    // Drop the lock_ before pausing the debug_compressor_ below which might trigger VMO destruction
    // and acquire the heap lock.
  }

#if DEBUG_ASSERT_IMPLEMENTED
  // Pause might drop the last reference to a VMO and trigger VMO destruction, which would then call
  // back into the page queues, so we must not hold the list_lock_ over the operation. We can
  // utilize the fact that once the debug_compressor_ is set it is never destroyed, so can take a
  // raw pointer to it.
  VmDebugCompressor* dc = nullptr;
  {
    Guard<CriticalMutex> list_guard{&list_lock_};
    if (debug_compressor_) {
      dc = &*debug_compressor_;
    }
  }
  if (dc) {
    dc->Pause();
  }
#endif
}

void PageQueues::EnableAging() {
  {
    Guard<CriticalMutex> guard{&lock_};
    // Validate a double EnableAging is not happening.
    if (!aging_disabled_) {
      panic("Mismatched disable/enable pair");
    }
    aging_disabled_ = false;

    // Notify the threads, allowing them to proceed with any pending actions if they had been
    // waiting.
    mru_event_.Signal();
    lru_event_.Signal();
  }

#if DEBUG_ASSERT_IMPLEMENTED
  Guard<CriticalMutex> guard{&list_lock_};
  if (debug_compressor_) {
    debug_compressor_->Resume();
  }
#endif
}

const char* PageQueues::string_from_age_reason(PageQueues::AgeReason reason) {
  switch (reason) {
    case AgeReason::ActiveRatio:
      return "Active ratio";
    case AgeReason::Timeout:
      return "Timeout";
    case AgeReason::Manual:
      return "Manual";
    default:
      panic("Unreachable");
  }
}

void PageQueues::Dump() {
  // Need to grab a copy of all the counts and generations. As the lock is needed to acquire the
  // active/inactive counts, also hold the lock over the copying of the counts to avoid needless
  // races.
  uint64_t mru_gen;
  uint64_t lru_gen;
  size_t counts[kNumReclaim] = {};
  size_t inactive_count;
  size_t failed_reclaim;
  size_t dirty;
  zx_instant_mono_t last_age_time;
  AgeReason last_age_reason;
  ActiveInactiveCounts activeinactive;
  {
    Guard<CriticalMutex> guard{&lock_};
    mru_gen = mru_gen_.load(ktl::memory_order_relaxed);
    lru_gen = lru_gen_.load(ktl::memory_order_relaxed);
    failed_reclaim = page_queue_counts_[PageQueueFailedReclaim].load(ktl::memory_order_relaxed);
    inactive_count = page_queue_counts_[PageQueueReclaimIsolate].load(ktl::memory_order_relaxed);
    dirty = page_queue_counts_[PageQueuePagerBackedDirty].load(ktl::memory_order_relaxed);
    for (uint32_t i = 0; i < kNumReclaim; i++) {
      counts[i] = page_queue_counts_[PageQueueReclaimBase + i].load(ktl::memory_order_relaxed);
    }
    activeinactive = GetActiveInactiveCounts();
    last_age_time = last_age_time_.load(ktl::memory_order_relaxed);
    last_age_reason = last_age_reason_;
  }
  // Small arbitrary number that should be more than large enough to hold the constructed string
  // without causing stack allocation pressure.
  constexpr size_t kBufSize = 50;
  // Start with the buffer null terminated. snprintf will always keep it null terminated.
  char buf[kBufSize] __UNINITIALIZED = "\0";
  size_t buf_len = 0;
  // This formats the counts of all buckets, not just those within the mru->lru range, even though
  // any buckets not in that range should always have a count of zero. The format this generates is
  // [active],[active],inactive,inactive,{last inactive},should-be-zero,should-be-zero
  // Although the inactive and should-be-zero use the same formatting, they are broken up by the
  // {last inactive}.
  for (uint64_t i = 0; i < kNumReclaim; i++) {
    PageQueue queue = gen_to_queue(mru_gen - i);
    ASSERT(buf_len < kBufSize);
    const size_t remain = kBufSize - buf_len;
    int write_len;
    if (i < kNumActiveQueues) {
      write_len = snprintf(buf + buf_len, remain, "[%zu],", counts[queue - PageQueueReclaimBase]);
    } else if (i == mru_gen - lru_gen) {
      write_len = snprintf(buf + buf_len, remain, "{%zu},", counts[queue - PageQueueReclaimBase]);
    } else {
      write_len = snprintf(buf + buf_len, remain, "%zu,", counts[queue - PageQueueReclaimBase]);
    }
    // Negative values are returned on encoding errors, which we never expect to get.
    ASSERT(write_len >= 0);
    if (static_cast<uint>(write_len) >= remain) {
      // Buffer too small, just use whatever we have constructed so far.
      break;
    }
    buf_len += write_len;
  }
  zx_instant_mono_t current = current_mono_time();
  timespec age_time = zx_timespec_from_duration(zx_time_sub_time(current, last_age_time));
  printf("pq: MRU:%" PRIu64 " (%ld.%lds ago, %s) LRU:%" PRIu64 " Act/Inact:%zu/%zu\n", mru_gen,
         age_time.tv_sec, age_time.tv_nsec, string_from_age_reason(last_age_reason), lru_gen,
         activeinactive.active, activeinactive.inactive);
  printf("pq: %s Isolate:%zu Dirty:%zu Fail:%zu\n", buf, inactive_count, dirty, failed_reclaim);
}

// This runs the aging thread. Aging, unlike lru processing, scanning or eviction, requires very
// little work and is more about coordination. As such this thread is heavy on checks and signalling
// but generally only needs to hold any locks for the briefest of times.
// There is, currently, one exception to that, which is the calls to scanner_wait_for_accessed_scan.
// The scanner will, eventually, be a separate thread that is synchronized with, but presently
// a full scan may happen inline in that method call, and get attributed directly to this thread.
void PageQueues::MruThread() {
  // Pretend that aging happens during startup to simplify the rest of the loop logic.
  last_age_time_ = current_mono_time();

  while (!shutdown_threads_.load(ktl::memory_order_relaxed)) {
    // Attempt to perform aging steps in a helper lambda that acquires the lock. Once there is no
    // more aging to be done returns a deadline to wait for.
    zx_instant_mono_t wait_deadline = [&]() {
      while (!shutdown_threads_.load(ktl::memory_order_relaxed)) {
        Guard<CriticalMutex> guard{&lock_};
        // If aging is disabled or we are unable to age if we wanted to then exit and wait to be
        // signaled.
        if (aging_disabled_ || !CanIncrementMruGenLocked()) {
          return ZX_TIME_INFINITE;
        }
        auto reason_or_deadline = GetAgeReasonLocked();
        if (const zx_instant_mono_t* age_deadline =
                ktl::get_if<zx_instant_mono_t>(&reason_or_deadline)) {
          // Cannot age yet, so wait till the provided deadline or we are otherwise signaled.
          return *age_deadline;
        }
        // Attempt to proceed with aging.
        TryAgingLocked(ktl::get<AgeReason>(reason_or_deadline), guard.take());
      }
      return ZX_TIME_INFINITE_PAST;
    }();
    // The deadline returned is a suggestion on how long to wait before attempting to age again.
    // However, we additionally wait on the mru_event_ instead of just sleeping as the event will
    // get signaled should anything material change that may cause us to be able to age sooner than
    // the original deadline.
    mru_event_.WaitDeadline(wait_deadline, Interruptible::No);
  }
}

// This thread should, at some point, have some of its logic and signaling merged with the Evictor.
// Currently it might process the lru queue whilst the evictor is already trying to evict, which is
// not harmful but it's a bit wasteful as it doubles the work that happens.
// LRU processing, via ProcessIsolateAndLruQueues, is expensive and happens under the lock_. It is
// expected that ProcessIsolateAndLruQueues perform small units of work to avoid this thread
// causing excessive lock contention.
void PageQueues::LruThread() {
  constexpr uint kLruNeedsProcessingPollSeconds = 90;
  uint64_t pending_target_gen = UINT64_MAX;
  while (!shutdown_threads_.load(ktl::memory_order_relaxed)) {
    zx_status_t wait_status =
        lru_event_.Wait(Deadline::after_mono(ZX_SEC(kLruNeedsProcessingPollSeconds)));

    uint64_t target_gen;
    bool needs_processing = false;
    // Take the lock so we can calculate (race free) a target mru-gen.
    {
      Guard<CriticalMutex> guard{&lock_};
      needs_processing = NeedsLruProcessingLocked();
      // If needs processing is false this will calculate an incorrect target_gen, but that's fine
      // as we'll just discard it and it's simpler to just do it here unconditionally while the lock
      // is already held.
      target_gen = lru_gen_.load(ktl::memory_order_relaxed) + 1;
    }
    if (!needs_processing) {
      pq_lru_spurious_wakeup.Add(1);
      continue;
    }
    if (wait_status == ZX_ERR_TIMED_OUT) {
      // The queue needs processing, but we woke up due to a timeout on the event and not a signal.
      // This could happen due to a race where we woke up before the MruThread could actually set
      // the signal, in which case we want to record the target_gen that we saw and then go back and
      // wait for the signal.
      // In the case where we have timed out *and* the target_gen we want is the same as the last
      // target_gen we were looking for then this means that we have gone a full poll interval with:
      //  * Processing needing to happen.
      //  * No event being signaled.
      //  * No other thread processing the queue for us.
      if (pending_target_gen != target_gen) {
        pending_target_gen = target_gen;
        continue;
      }
      printf("ERROR LruThread signal was not seen after %u seconds and queue needs processing\n",
             kLruNeedsProcessingPollSeconds);
      Dump();
    }

    // Keep processing until we have caught up to what is required. This ensures we are
    // re-synchronized with the mru-thread and will not miss any signals on the lru_event_.
    while (needs_processing) {
      // With the lock dropped process the target. This is not racy as generations are monotonic, so
      // worst case someone else already processed this generation and this call will be a no-op.
      ProcessLruQueue(target_gen, ktl::nullopt);

      // Take the lock so we can calculate (race free) a target mru-gen.
      Guard<CriticalMutex> guard{&lock_};
      needs_processing = NeedsLruProcessingLocked();
      // If needs processing is false this will calculate an incorrect target_gen, but that's fine
      // as we'll just discard it and it's simpler to just do it here unconditionally while the lock
      // is already held.
      target_gen = lru_gen_.load(ktl::memory_order_relaxed) + 1;
    }
  }
}

void PageQueues::TryAgingLocked(AgeReason age_reason, Guard<CriticalMutex>::Adoptable&& adopt) {
  VM_KTRACE_DURATION(2, "LruThread Aging");
  Guard<CriticalMutex> guard{AdoptLock, &lock_, ktl::move(adopt)};
  if (scanner_needs_accessed_scan(last_age_time_)) {
    // Perform the accessed without our lock held to avoid unnecessary contention.
    guard.Release();
    scanner_wait_for_accessed_scan(last_age_time_);
  } else {
    IncrementMruGenLocked(age_reason);
  }
}

void PageQueues::IncrementMruGenLocked(AgeReason age_reason) {
  ASSERT(CanIncrementMruGenLocked());

  // Increment the mru generation and record the current age reason etc.
  mru_gen_.fetch_add(1, ktl::memory_order_relaxed);
  last_age_time_ = current_mono_time();
  last_age_reason_ = age_reason;

  // As aging has happened we should consume any old active ratio trigger and re-calculate it.
  active_ratio_triggered_ = false;
  CheckActiveRatioAgingLocked();

  // Signal any externally supplied aging event if one exists.
  if (aging_event_) {
    aging_event_->Signal();
  }

  // Required to notify the LRU thread of mru generation changes.
  MaybeTriggerLruProcessingLocked();

  // Keep a count of the different reasons we have rotated.
  switch (age_reason) {
    case AgeReason::Timeout:
      pq_aging_reason_timeout.Add(1);
      break;
    case AgeReason::ActiveRatio:
      pq_aging_reason_active_ratio.Add(1);
      break;
    case AgeReason::Manual:
      pq_aging_reason_manual.Add(1);
      break;
    default:
      panic("Unknown age reason");
  }
}

void PageQueues::RotateReclaimQueues() {
  Guard<CriticalMutex> guard{&lock_};
  // 'Force' aging to happen by processing the lru queue until we are able to increment the mru gen.
  // This directly manipulates the mru gen instead of using TryAgingLocked as it intentionally
  // sidesteps certain updates.
  while (!CanIncrementMruGenLocked()) {
    uint64_t target_gen = lru_gen_.load(ktl::memory_order_relaxed) + 1;
    guard.CallUnlocked([&]() { ProcessLruQueue(target_gen, ktl::nullopt); });
  }
  IncrementMruGenLocked(AgeReason::Manual);
}

ktl::optional<PageQueues::VmoBacklink> PageQueues::PeekIsolateList() {
  Guard<CriticalMutex> guard{&list_lock_};
  for (auto& list : isolate_queues_) {
    if (!list.is_empty()) {
      vm_page_t* head = &list.front();
      PageQueue page_queue =
          static_cast<PageQueue>(head->object.get_page_queue_ref().load(ktl::memory_order_relaxed));
      DEBUG_ASSERT(page_queue == PageQueueReclaimIsolate);
      VmCowPages* cow = reinterpret_cast<VmCowPages*>(head->object.get_object());
      DEBUG_ASSERT(cow);
      // Upgrading to a refptr can never fail as all pages are removed from a VmCowPages, and
      // hence from the page queues here, prior to the last reference to a cow pages being
      // dropped.
      fbl::RefPtr<VmCowPages> cow_pages = fbl::MakeRefPtrUpgradeFromRaw(cow, list_lock_);
      DEBUG_ASSERT(cow_pages);
      return VmoBacklink{
          .cow = ktl::move(cow_pages), .page = head, .offset = head->object.get_page_offset()};
    }
  }
  return ktl::nullopt;
}

void PageQueues::ProcessLruQueue(uint64_t target_gen, ktl::optional<size_t> isolate) {
  // This assertion is <=, and not strictly <, since to evict a some queue X, the target must be
  // X+1. Hence to preserve kNumActiveQueues, we can allow target_gen to become equal to the first
  // active queue, as this will process all the non-active queues. Although we might refresh our
  // value for the mru_queue, since the mru_gen_ is monotonic increasing, if this assert passes once
  // it should continue to be true.
  ASSERT(target_gen <= mru_gen_.load(ktl::memory_order_relaxed) - (kNumActiveQueues - 1));

  // Calculate a truly worst case loop iteration count based on every page being in the LRU
  // queue and needing to iterate the LRU multiple steps to the target_gen. Instead of reading the
  // LRU and comparing the target_gen, just add a buffer of the maximum number of page queues.
  ActiveInactiveCounts active_inactive = GetActiveInactiveCounts();
  const uint64_t max_lru_iterations =
      active_inactive.active + active_inactive.inactive + kNumReclaim;
  // Loop iteration counting is just for diagnostic purposes.
  uint64_t loop_iterations = 0;

  // Pages in this list might be reclaimed or replaced with a loaned page, depending on the action
  // specified in deferred_action. Each of these actions must be done outside the lock_, so we
  // accumulate pages and then act after lock_ is released. The number of items collected per batch
  // is limited to avoid excessive stack usage.
  // The deferred_list is declared here as it is expensive to construct/destruct and we would like
  // to reuse it between iterations.
  constexpr uint32_t kMaxDeferredCount = 16;
  LruIsolate<kMaxDeferredCount> deferred_list;

  // Only accumulate pages to try to replace with loaned pages if loaned pages are available and
  // we're allowed to borrow at this code location. We check is_borrowing_active() instead of
  // is_borrowing_on_mru_enabled() to temporarily suspend sweeping if an unloan is currently
  // in progress, avoiding lock contention on the borrowing VMOs' paged_vmo_lock_ between the LRU
  // thread and the unloaning thread.
  const bool has_loaned_pages = (pmm_count_loaned_free_pages() != 0);
  const bool do_sweeping =
      has_loaned_pages && PhysicalPageBorrowingConfig::Get().is_borrowing_active();
  if (do_sweeping) {
    // Sweeping is active, reset the skipped sweeps counter.
    consecutive_skipped_sweeps_ = 0;
  } else if (unlikely(PhysicalPageBorrowingConfig::Get().is_borrowing_on_mru_enabled() &&
                      has_loaned_pages)) {
    // Sweeping is enabled and there are loaned free pages to sweep, but we skipped sweeping due to
    // active unloans.
    pq_lru_sweeping_skipped_unloan.Add(1);
    const uint32_t count = consecutive_skipped_sweeps_.fetch_add(1) + 1;
    // Log a diagnostic warning if sweeping is skipped for a large number of consecutive iterations.
    if (unlikely(count >= 1000 && count % 1000 == 0)) {
      printf(
          "[pq]: WARNING: LRU page sweeping has been skipped for %u consecutive iterations "
          "due to active unloans\n",
          count);
    }
  } else {
    // Sweeping is not applicable at all, reset the skipped sweeps counter.
    consecutive_skipped_sweeps_ = 0;
  }

  size_t isolate_remaining = isolate.value_or(SIZE_MAX);

  VM_KTRACE_DURATION(2, "ProcessLruQueue");
  while (isolate_remaining > 0) {
    if (loop_iterations++ == max_lru_iterations) {
      KERNEL_OOPS("[pq]: WARNING: %s exceeded expected max LRU loop iterations %" PRIu64 "\n",
                  __FUNCTION__, max_lru_iterations);
    }

    deferred_list.Flush();
    bool post = false;
    {
      // Need to hold the general lock, in addition to the list lock, so that the lru_gen_ does not
      // change while we are working.
      Guard<CriticalMutex> guard{&lock_};
      // Fill in the lru action now that the lock is held.
      deferred_list.SetLruAction(lru_action_);
      Guard<CriticalMutex> list_guard{&list_lock_};
      const uint64_t lru = lru_gen_.load(ktl::memory_order_relaxed);
      if (lru >= target_gen) {
        break;
      }
      const PageQueue mru_queue = mru_gen_to_queue();
      const PageQueue lru_queue = gen_to_queue(lru);
      VmPageDoublyLinkedList* list = &page_queues_[lru_queue];

      for (size_t iterations = 0;
           !list->is_empty() && !deferred_list.Full() && isolate_remaining > 0;) {
        // Newer pages accumulate at the head of the list, while older pages are at the tail.
        // Pages are removed from the tail to ensure FIFO processing (oldest first).
        vm_page_t* page = list->pop_back();
        PageQueue page_queue = static_cast<PageQueue>(
            page->object.get_page_queue_ref().load(ktl::memory_order_relaxed));
        DEBUG_ASSERT(page_queue >= PageQueueReclaimBase);

        // If the queue stored in the page does not match then we want to move it to its correct
        // queue with the caveat that its queue could be invalid. The queue would be invalid if
        // MarkAccessed had raced. Should this happen we know that the page is actually *very* old,
        // and so we will fall back to the case of forcibly changing its age to the new lru gen.
        if (page_queue != lru_queue && queue_is_valid(page_queue, lru_queue, mru_queue)) {
          // Pages are added to the head because they are newer pages.
          page_queues_[page_queue].push_front(page);

          if (do_sweeping && !page->is_loaned() && queue_is_active(page_queue, mru_queue)) {
            deferred_list.AddLoanReplacement(page, this);
          }
        } else {
          // Force it the isolate list, don't care about races. If we happened to access it at the
          // same time then too bad.
          // Aged pages go to the standard priority isolate queue.
          VmPageDoublyLinkedList* target_queue = &isolate_queues_[kIsolateQueueStandard];
          PageQueue old_queue = static_cast<PageQueue>(
              page->object.get_page_queue_ref().exchange(PageQueueReclaimIsolate));
          DEBUG_ASSERT(old_queue >= PageQueueReclaimBase);

          page_queue_counts_[old_queue].fetch_sub(1, ktl::memory_order_relaxed);
          page_queue_counts_[PageQueueReclaimIsolate].fetch_add(1, ktl::memory_order_relaxed);
          // PeekIsolate peeks at the head. Pages are added to the tail of the isolate queue to
          // preserve relative age (older pages at the head, newer at the tail).
          target_queue->push_back(page);
          deferred_list.AddReclaimable(page, this);
          isolate_remaining--;
        }
        iterations++;
        if (BatchOpShouldDropLock(iterations)) {
          break;
        }
      }
      if (list->is_empty()) {
        // Note that we held the lock the entire time, and lru_gen_ is always modified with the lock
        // held, so this should always precisely set lru_gen_ to lru + 1.
        [[maybe_unused]] uint64_t prev = lru_gen_.fetch_add(1, ktl::memory_order_relaxed);
        DEBUG_ASSERT(prev == lru);
        post = true;
      }
    }
    if (post) {
      // The lru gen was changed and the MruThread might be waiting for space to increment the mru
      // gen, so kick the MruThread.
      mru_event_.Signal();
    }
  }
}

void PageQueues::MarkAccessedMaybeIsolate(vm_page_t* page) {
  pq_accessed_isolate.Add(1);
  {
    Guard<CriticalMutex> guard{&list_lock_};
    auto queue_ref = page->object.get_page_queue_ref();
    uint8_t old_gen = queue_ref.load(ktl::memory_order_relaxed);
    // With the lock held check if the page is in a reclaim queue. Although it can move between
    // different reclaim queues, it cannot go from reclaim->non-reclaim without the lock.
    if (!queue_is_reclaim(static_cast<PageQueue>(old_gen))) {
      pq_accessed_not_reclaim.Add(1);
      return;
    }
    MoveToQueueLockedList(page, mru_gen_to_queue());
  }
  MaybeCheckActiveRatioAging(1);
}

void PageQueues::MarkAccessed(vm_page_t* page) {
  pq_accessed_normal.Add(1);
  auto queue_ref = page->object.get_page_queue_ref();
  uint8_t old_gen = queue_ref.load(ktl::memory_order_relaxed);
  const uint32_t target_queue = mru_gen_to_queue();
  if (old_gen == target_queue) {
    pq_accessed_normal_same_queue.Add(1);
    return;
  }
  do {
    // If we ever find old_gen to not be in the active/inactive range then this means the page has
    // either been racily removed from, or was never in, the reclaim queue. In which case we
    // can return as there's nothing to be marked accessed.
    if (!queue_is_reclaim(static_cast<PageQueue>(old_gen))) {
      pq_accessed_not_reclaim.Add(1);
      return;
    }
    // The Isolate queue is a reclaim queue, but we cannot just change the queue in the vm_page_t
    // and must actually move it between the lists.
    if (old_gen == PageQueueReclaimIsolate) {
      MarkAccessedMaybeIsolate(page);
      return;
    }
    // Between loading the mru_gen and finally storing it in the queue_ref it's possible for our
    // calculated target_queue to become invalid. This is extremely unlikely as it would require
    // us to stall for long enough for the lru_gen to pass this point, but if it does happen then
    // ProcessLruQueues will notice our queue is invalid and correct our age to be that of lru_gen.
  } while (!queue_ref.compare_exchange_weak(old_gen, static_cast<uint8_t>(target_queue),
                                            ktl::memory_order_relaxed));
  page_queue_counts_[old_gen].fetch_sub(1, ktl::memory_order_relaxed);
  page_queue_counts_[target_queue].fetch_add(1, ktl::memory_order_relaxed);

  MaybeCheckActiveRatioAging(1);
}

void PageQueues::MaybeCheckActiveRatioAging(size_t pages) {
  if (unlikely(!RecordActiveRatioSkips(pages))) {
    Guard<CriticalMutex> guard{&lock_};
    CheckActiveRatioAgingLocked();
  }
}

void PageQueues::MaybeCheckActiveRatioAgingLocked(size_t pages) {
  if (unlikely(!RecordActiveRatioSkips(pages))) {
    CheckActiveRatioAgingLocked();
  }
}

void PageQueues::SetQueueBacklinkLockedList(vm_page_t* page, void* object, uintptr_t page_offset,
                                            PageQueue queue) {
  DEBUG_ASSERT(queue != PageQueueReclaimIsolate);
  DEBUG_ASSERT(page->state() == vm_page_state::OBJECT);
  DEBUG_ASSERT(!page->is_free());
  DEBUG_ASSERT(!page->queue_node.InContainer());
  DEBUG_ASSERT(object);
  DEBUG_ASSERT(!page->object.get_object());
  DEBUG_ASSERT(page->object.get_page_offset() == 0);

  page->object.set_object(object);
  page->object.set_page_offset(page_offset);

  DEBUG_ASSERT(page->object.get_page_queue_ref().load(ktl::memory_order_relaxed) == PageQueueNone);
  page->object.get_page_queue_ref().store(queue, ktl::memory_order_relaxed);
  page_queues_[queue].push_front(page);
  page_queue_counts_[queue].fetch_add(1, ktl::memory_order_relaxed);
}

void PageQueues::MoveToQueueLockedList(vm_page_t* page, PageQueue queue) {
  DEBUG_ASSERT(queue != PageQueueReclaimIsolate);
  DEBUG_ASSERT(page->state() == vm_page_state::OBJECT);
  DEBUG_ASSERT(!page->is_free());
  DEBUG_ASSERT(page->queue_node.InContainer());
  DEBUG_ASSERT(page->object.get_object());
  uint32_t old_queue = page->object.get_page_queue_ref().exchange(queue, ktl::memory_order_relaxed);
  DEBUG_ASSERT(old_queue != PageQueueNone);

  page->queue_node.template RemoveFromContainer<vm_page_node_traits>();
  page_queues_[queue].push_front(page);
  page_queue_counts_[old_queue].fetch_sub(1, ktl::memory_order_relaxed);
  page_queue_counts_[queue].fetch_add(1, ktl::memory_order_relaxed);
}

void PageQueues::MoveToIsolateLockedList(vm_page_t* page, size_t isolate_queue_index) {
  DEBUG_ASSERT(isolate_queue_index < kNumIsolateQueues);
  DEBUG_ASSERT(page->state() == vm_page_state::OBJECT);
  DEBUG_ASSERT(!page->is_free());
  DEBUG_ASSERT(page->queue_node.InContainer());
  DEBUG_ASSERT(page->object.get_object());
  uint32_t old_queue = page->object.get_page_queue_ref().exchange(PageQueueReclaimIsolate,
                                                                  ktl::memory_order_relaxed);
  DEBUG_ASSERT(old_queue != PageQueueNone);

  page->queue_node.template RemoveFromContainer<vm_page_node_traits>();
  isolate_queues_[isolate_queue_index].push_back(page);
  page_queue_counts_[old_queue].fetch_sub(1, ktl::memory_order_relaxed);
  page_queue_counts_[PageQueueReclaimIsolate].fetch_add(1, ktl::memory_order_relaxed);
}

void PageQueues::SetWired(vm_page_t* page, VmCowPages* object, uint64_t page_offset) {
  Guard<CriticalMutex> guard{&list_lock_};
  SetQueueBacklinkLockedList(page, object, page_offset, PageQueueWired);
}

void PageQueues::MoveToWired(vm_page_t* page) {
  {
    Guard<CriticalMutex> guard{&list_lock_};
    MoveToQueueLockedList(page, PageQueueWired);
  }
  MaybeCheckActiveRatioAging(1);
}

void PageQueues::SetAnonymous(vm_page_t* page, VmCowPages* object, uint64_t page_offset,
                              bool skip_reclaim) {
  {
    Guard<CriticalMutex> guard{&list_lock_};
    SetQueueBacklinkLockedList(
        page, object, page_offset,
        anonymous_is_reclaimable_ && !skip_reclaim ? mru_gen_to_queue() : PageQueueAnonymous);
#if DEBUG_ASSERT_IMPLEMENTED
    if (debug_compressor_) {
      debug_compressor_->Add(page, object, page_offset);
    }
#endif
  }
  MaybeCheckActiveRatioAging(1);
}

void PageQueues::SetHighPriority(vm_page_t* page, VmCowPages* object, uint64_t page_offset) {
  Guard<CriticalMutex> guard{&list_lock_};
  SetQueueBacklinkLockedList(page, object, page_offset, PageQueueHighPriority);
}

void PageQueues::MoveToHighPriority(vm_page_t* page) {
  {
    Guard<CriticalMutex> guard{&list_lock_};
    MoveToQueueLockedList(page, PageQueueHighPriority);
  }
  MaybeCheckActiveRatioAging(1);
}

void PageQueues::MoveToAnonymous(vm_page_t* page, bool skip_reclaim) {
  {
    Guard<CriticalMutex> guard{&list_lock_};
    MoveToQueueLockedList(
        page, anonymous_is_reclaimable_ && !skip_reclaim ? mru_gen_to_queue() : PageQueueAnonymous);
#if DEBUG_ASSERT_IMPLEMENTED
    if (debug_compressor_) {
      debug_compressor_->Add(page, reinterpret_cast<VmCowPages*>(page->object.get_object()),
                             page->object.get_page_offset());
    }
#endif
  }
  MaybeCheckActiveRatioAging(1);
}

void PageQueues::SetReclaim(vm_page_t* page, VmCowPages* object, uint64_t page_offset) {
  {
    Guard<CriticalMutex> guard{&list_lock_};
    SetQueueBacklinkLockedList(page, object, page_offset, mru_gen_to_queue());
  }
  MaybeCheckActiveRatioAging(1);
}

void PageQueues::MoveToReclaim(vm_page_t* page) {
  {
    Guard<CriticalMutex> guard{&list_lock_};
    MoveToQueueLockedList(page, mru_gen_to_queue());
  }
  MaybeCheckActiveRatioAging(1);
}

void PageQueues::MoveToReclaimDontNeed(vm_page_t* page) {
  {
    Guard<CriticalMutex> guard{&list_lock_};
    MoveToIsolateLockedList(page, kIsolateQueueDontNeed);
  }
  MaybeCheckActiveRatioAging(1);
}

void PageQueues::SetPagerBackedDirty(vm_page_t* page, VmCowPages* object, uint64_t page_offset) {
  Guard<CriticalMutex> guard{&list_lock_};
  SetQueueBacklinkLockedList(page, object, page_offset, PageQueuePagerBackedDirty);
}

void PageQueues::MoveToPagerBackedDirty(vm_page_t* page) {
  {
    Guard<CriticalMutex> guard{&list_lock_};
    MoveToQueueLockedList(page, PageQueuePagerBackedDirty);
  }
  MaybeCheckActiveRatioAging(1);
}

void PageQueues::SetAnonymousZeroFork(vm_page_t* page, VmCowPages* object, uint64_t page_offset) {
  {
    Guard<CriticalMutex> guard{&list_lock_};
    SetQueueBacklinkLockedList(
        page, object, page_offset,
        zero_fork_is_reclaimable_ ? mru_gen_to_queue() : PageQueueAnonymousZeroFork);
#if DEBUG_ASSERT_IMPLEMENTED
    if (debug_compressor_) {
      debug_compressor_->Add(page, object, page_offset);
    }
#endif
  }
  MaybeCheckActiveRatioAging(1);
}

void PageQueues::MoveAnonymousToAnonymousZeroFork(vm_page_t* page) {
  // First perform a common case short-circuit where the page is already in the anonymous queue and
  // both the anonymous and zero fork queues are the same reclaimable queue. In this case the page
  // is already in the correct queue, and nothing needs to be done.
  if (zero_fork_is_reclaimable_ && anonymous_is_reclaimable_ &&
      queue_is_reclaim(static_cast<PageQueue>(
          page->object.get_page_queue_ref().load(ktl::memory_order_relaxed)))) {
    return;
  }
  {
    Guard<CriticalMutex> guard{&list_lock_};
    // First check if the page is presently in whatever counts as the anonymous queue. If it isn't,
    // then we don't move it.
    PageQueue queue =
        static_cast<PageQueue>(page->object.get_page_queue_ref().load(ktl::memory_order_relaxed));
    if (anonymous_is_reclaimable_ && !queue_is_reclaim(queue)) {
      return;
    }
    if (!anonymous_is_reclaimable_ && queue != PageQueueAnonymous) {
      return;
    }
    // In the anonymous queue, move to whatever counts as the anonymous zero fork queue.
    MoveToQueueLockedList(
        page, zero_fork_is_reclaimable_ ? mru_gen_to_queue() : PageQueueAnonymousZeroFork);
#if DEBUG_ASSERT_IMPLEMENTED
    if (debug_compressor_) {
      debug_compressor_->Add(page, reinterpret_cast<VmCowPages*>(page->object.get_object()),
                             page->object.get_page_offset());
    }
#endif
  }
  MaybeCheckActiveRatioAging(1);
}

void PageQueues::CompressFailed(vm_page_t* page) {
  {
    Guard<CriticalMutex> guard{&list_lock_};
    // Move the page if its currently in some kind of reclaimable queue.
    if (queue_is_reclaim(static_cast<PageQueue>(
            page->object.get_page_queue_ref().load(ktl::memory_order_relaxed)))) {
      MoveToQueueLockedList(page, PageQueueFailedReclaim);
    }
  }
  MaybeCheckActiveRatioAging(1);
}

void PageQueues::ChangeObjectOffset(vm_page_t* page, VmCowPages* object, uint64_t page_offset) {
  Guard<CriticalMutex> guard{&list_lock_};
  ChangeObjectOffsetLockedList(page, object, page_offset);
}

void PageQueues::ChangeObjectOffsetArray(vm_page_t** pages, VmCowPages* object, uint64_t* offsets,
                                         size_t count) {
  DEBUG_ASSERT(pages);
  DEBUG_ASSERT(offsets);
  DEBUG_ASSERT(object);

  for (size_t i = 0; i < count;) {
    Guard<CriticalMutex> guard{&list_lock_};
    // Use a do/while structure for the inner loop to ensure we at least make some progress before
    // checking again for a lock drop.
    do {
      DEBUG_ASSERT(pages[i]);
      ChangeObjectOffsetLockedList(pages[i], object, offsets[i]);
      i++;
    } while (i < count && !BatchOpShouldDropLock(i));
  }
}

void PageQueues::ChangeObjectOffsetLockedList(vm_page_t* page, VmCowPages* object,
                                              uint64_t page_offset) {
  DEBUG_ASSERT(page->state() == vm_page_state::OBJECT);
  DEBUG_ASSERT(!page->is_free());
  DEBUG_ASSERT(page->queue_node.InContainer());
  DEBUG_ASSERT(object);
  DEBUG_ASSERT(page->object.get_object());
  page->object.set_object(object);
  page->object.set_page_offset(page_offset);
}

void PageQueues::RemoveLockedList(vm_page_t* page) {
  // Directly exchange the old gen.
  uint32_t old_queue =
      page->object.get_page_queue_ref().exchange(PageQueueNone, ktl::memory_order_relaxed);
  DEBUG_ASSERT(old_queue != PageQueueNone);
  page_queue_counts_[old_queue].fetch_sub(1, ktl::memory_order_relaxed);
  page->object.set_object(nullptr);
  page->object.set_page_offset(0);
  page->queue_node.template RemoveFromContainer<vm_page_node_traits>();
}

void PageQueues::Remove(vm_page_t* page) {
  {
    Guard<CriticalMutex> guard{&list_lock_};
    RemoveLockedList(page);
  }
  MaybeCheckActiveRatioAging(1);
}

void PageQueues::RemoveArrayIntoList(vm_page_t** pages, size_t count,
                                     VmPageDoublyLinkedList* out_list) {
  DEBUG_ASSERT(pages);

  for (size_t i = 0; i < count;) {
    Guard<CriticalMutex> guard{&list_lock_};
    // Use a do/while structure for the inner loop to ensure we at least make some progress before
    // checking again for a lock drop.
    do {
      DEBUG_ASSERT(pages[i]);
      RemoveLockedList(pages[i]);
      out_list->push_back(pages[i]);
      i++;
    } while (i < count && !BatchOpShouldDropLock(i));
  }

  MaybeCheckActiveRatioAging(count);
}

PageQueues::ReclaimCounts PageQueues::GetReclaimQueueCounts() const {
  ReclaimCounts counts;

  // Grab the lock to prevent LRU processing, this lets us get a slightly less racy snapshot of
  // the queue counts, although we may still double count pages that move after we count them.
  // Specifically any parallel callers of MarkAccessed could move a page and change the counts,
  // causing us to either double count or miss count that page. As these counts are not load
  // bearing we accept the very small chance of potentially being off a few pages.
  Guard<CriticalMutex> guard{&list_lock_};
  uint64_t lru = lru_gen_.load(ktl::memory_order_relaxed);
  uint64_t mru = mru_gen_.load(ktl::memory_order_relaxed);

  counts.total = 0;
  for (uint64_t index = lru; index <= mru; index++) {
    uint64_t count = page_queue_counts_[gen_to_queue(index)].load(ktl::memory_order_relaxed);
    // Distance to the MRU, and not the LRU, determines the bucket the count goes into. This is to
    // match the logic in PeekPagerBacked, which is also based on distance to MRU.
    if (index > mru - kNumActiveQueues) {
      counts.newest += count;
    } else if (index <= mru - (kNumReclaim - kNumOldestQueues)) {
      counts.oldest += count;
    }
    counts.total += count;
  }
  // Account the Isolate queue length under |oldest|, since (Isolate + oldest LRU) pages are
  // eligible for reclamation first. |oldest| is meant to track pages eligible for eviction first.
  uint64_t inactive_count =
      page_queue_counts_[PageQueueReclaimIsolate].load(ktl::memory_order_relaxed);
  counts.oldest += inactive_count;
  counts.total += inactive_count;
  return counts;
}

PageQueues::Counts PageQueues::QueueCounts() const {
  Counts counts = {};

  // Grab the lock to prevent LRU processing, this lets us get a slightly less racy snapshot of
  // the queue counts. We may still double count pages that move after we count them.
  Guard<CriticalMutex> guard{&list_lock_};
  uint64_t lru = lru_gen_.load(ktl::memory_order_relaxed);
  uint64_t mru = mru_gen_.load(ktl::memory_order_relaxed);

  for (uint64_t index = lru; index <= mru; index++) {
    counts.reclaim[mru - index] =
        page_queue_counts_[gen_to_queue(index)].load(ktl::memory_order_relaxed);
  }
  counts.reclaim_isolate =
      page_queue_counts_[PageQueueReclaimIsolate].load(ktl::memory_order_relaxed);
  counts.pager_backed_dirty =
      page_queue_counts_[PageQueuePagerBackedDirty].load(ktl::memory_order_relaxed);
  counts.anonymous = page_queue_counts_[PageQueueAnonymous].load(ktl::memory_order_relaxed);
  counts.wired = page_queue_counts_[PageQueueWired].load(ktl::memory_order_relaxed);
  counts.anonymous_zero_fork =
      page_queue_counts_[PageQueueAnonymousZeroFork].load(ktl::memory_order_relaxed);
  counts.failed_reclaim =
      page_queue_counts_[PageQueueFailedReclaim].load(ktl::memory_order_relaxed);
  counts.high_priority = page_queue_counts_[PageQueueHighPriority].load(ktl::memory_order_relaxed);
  return counts;
}

template <typename F>
bool PageQueues::DebugPageIsSpecificReclaim(const vm_page_t* page, F validator,
                                            size_t* queue) const {
  fbl::RefPtr<VmCowPages> cow_pages;
  {
    Guard<CriticalMutex> guard{&list_lock_};
    PageQueue q = (PageQueue)page->object.get_page_queue_ref().load(ktl::memory_order_relaxed);
    if (q < PageQueueReclaimBase || q > PageQueueReclaimLast) {
      return false;
    }
    if (queue) {
      *queue = queue_age(q, mru_gen_to_queue());
    }
    VmCowPages* cow = reinterpret_cast<VmCowPages*>(page->object.get_object());
    DEBUG_ASSERT(cow);
    cow_pages = fbl::MakeRefPtrUpgradeFromRaw(cow, guard);
    DEBUG_ASSERT(cow_pages);
  }
  return validator(cow_pages);
}

template <typename F>
bool PageQueues::DebugPageIsSpecificQueue(const vm_page_t* page, PageQueue queue,
                                          F validator) const {
  fbl::RefPtr<VmCowPages> cow_pages;
  {
    Guard<CriticalMutex> guard{&list_lock_};
    PageQueue q = (PageQueue)page->object.get_page_queue_ref().load(ktl::memory_order_relaxed);
    if (q != queue) {
      return false;
    }
    VmCowPages* cow = reinterpret_cast<VmCowPages*>(page->object.get_object());
    DEBUG_ASSERT(cow);
    cow_pages = fbl::MakeRefPtrUpgradeFromRaw(cow, guard);
    DEBUG_ASSERT(cow_pages);
  }
  return validator(cow_pages);
}

bool PageQueues::DebugPageIsReclaim(const vm_page_t* page, size_t* queue) const {
  return DebugPageIsSpecificReclaim(page, [](auto cow) { return true; }, queue);
}

bool PageQueues::DebugPageIsReclaimIsolate(const vm_page_t* page) const {
  return DebugPageIsSpecificQueue(page, PageQueueReclaimIsolate,
                                  [](auto cow) { return cow->can_evict(); });
}

bool PageQueues::DebugPageIsPagerBackedDirty(const vm_page_t* page) const {
  return page->object.get_page_queue_ref().load(ktl::memory_order_relaxed) ==
         PageQueuePagerBackedDirty;
}

bool PageQueues::DebugPageIsAnonymous(const vm_page_t* page) const {
  if (ReclaimIsOnlyPagerBacked()) {
    return page->object.get_page_queue_ref().load(ktl::memory_order_relaxed) == PageQueueAnonymous;
  }
  return DebugPageIsSpecificReclaim(page, [](auto cow) { return !cow->can_evict(); }, nullptr);
}

bool PageQueues::DebugPageIsWired(const vm_page_t* page) const {
  return page->object.get_page_queue_ref().load(ktl::memory_order_relaxed) == PageQueueWired;
}

bool PageQueues::DebugPageIsHighPriority(const vm_page_t* page) const {
  return page->object.get_page_queue_ref().load(ktl::memory_order_relaxed) == PageQueueHighPriority;
}

bool PageQueues::DebugPageIsAnonymousZeroFork(const vm_page_t* page) const {
  if (ReclaimIsOnlyPagerBacked()) {
    return page->object.get_page_queue_ref().load(ktl::memory_order_relaxed) ==
           PageQueueAnonymousZeroFork;
  }
  return DebugPageIsSpecificReclaim(page, [](auto cow) { return !cow->can_evict(); }, nullptr);
}

bool PageQueues::DebugPageIsAnyAnonymous(const vm_page_t* page) const {
  return DebugPageIsAnonymous(page) || DebugPageIsAnonymousZeroFork(page);
}

ktl::optional<PageQueues::VmoBacklink> PageQueues::PopAnonymousZeroFork() {
  ktl::optional<PageQueues::VmoBacklink> ret;
  {
    Guard<CriticalMutex> guard{&list_lock_};

    auto& q = page_queues_[PageQueueAnonymousZeroFork];
    vm_page_t* page = q.is_empty() ? nullptr : &q.back();
    if (!page) {
      return ktl::nullopt;
    }

    VmCowPages* cow = reinterpret_cast<VmCowPages*>(page->object.get_object());
    uint64_t page_offset = page->object.get_page_offset();
    DEBUG_ASSERT(cow);
    MoveToQueueLockedList(page, PageQueueAnonymous);
    ret = VmoBacklink{fbl::MakeRefPtrUpgradeFromRaw(cow, guard), page, page_offset};
  }
  MaybeCheckActiveRatioAging(1);
  return ret;
}

ktl::optional<PageQueues::VmoBacklink> PageQueues::PeekIsolate(size_t lowest_queue) {
  // Ignore any requests to evict from the active queues as this is never allowed.
  lowest_queue = ktl::max(lowest_queue, kNumActiveQueues);
  // Whether we're allowed to peek from all but the active queues. Typically true only under OOM.
  const bool peek_all_inactive = lowest_queue == kNumActiveQueues;

  // TODO(adanis): Restructure this loop such that there is no question about its termination, but
  // for now be paranoid.
  constexpr uint kMaxIterations = kNumReclaim * 2;
  uint loop_iterations = 0;

  while (true) {
    // Peek the Isolate queue in case anything is ready for us.
    ktl::optional<VmoBacklink> result = PeekIsolateList();
    if (result) {
      return result;
    }
    if (loop_iterations++ > kMaxIterations) {
      KERNEL_OOPS("[pq]: %s iterated more than %u times (%u). lru:%zu mru:%zu\n", __FUNCTION__,
                  kMaxIterations, loop_iterations, lru_gen_.load(ktl::memory_order_relaxed),
                  mru_gen_.load(ktl::memory_order_relaxed));
    }

    // Synchronize with any aging that was outstanding at the time of this call, and then use that
    // updated mru_gen_ to compute the lru_target. Only do this for the first iteration of the loop,
    // because we want to meet the termination condition to break out of the loop, which might not
    // happen if we update mru_gen_ every time.
    if (loop_iterations == 1) {
      SynchronizeWithAging();
      if (peek_all_inactive) {
        pq_peek_sync_aging_all_inactive.Add(1);
      } else {
        pq_peek_sync_aging.Add(1);
      }
    }
    // The limit gen is 1 larger than the lowest queue because evicting from queue X is done by
    // attempting to make the lru queue be X+1.
    const uint64_t lru_limit = mru_gen_.load(ktl::memory_order_relaxed) - (lowest_queue - 1);
    // Attempt to process one generation at a time to limit the work done before we find a
    // reclaimable page.
    const uint64_t lru_target = lru_gen_.load(ktl::memory_order_relaxed) + 1;
    if (lru_target > lru_limit) {
      return PeekIsolateList();
    }
    // Although we only need a single page in the Isolate queue to return for the peek result, under
    // the assumption that the caller is probably going to peek multiple pages move a few pages for
    // efficiency.
    // We do not want to process the entire LRU queue since it could contain tends to hundreds of
    // thousands of items and so the full processing is left to the LruThread.
    ProcessLruQueue(lru_target, 16);
    if (peek_all_inactive) {
      pq_peek_process_lru_all_inactive.Add(1);
    } else {
      pq_peek_process_lru.Add(1);
    }
  }
}

PageQueues::ActiveInactiveCounts PageQueues::GetActiveInactiveCounts() const {
  uint64_t active_count = 0;
  uint64_t inactive_count = 0;
  PageQueue mru = mru_gen_to_queue();
  for (uint8_t queue = 0; queue < PageQueueNumQueues; queue++) {
    uint64_t count = page_queue_counts_[queue].load(ktl::memory_order_relaxed);
    if (queue_is_active(static_cast<PageQueue>(queue), mru)) {
      active_count += count;
    }
    if (queue_is_inactive(static_cast<PageQueue>(queue), mru)) {
      inactive_count += count;
    }
  }
  return ActiveInactiveCounts{.active = active_count, .inactive = inactive_count};
}

void PageQueues::SetAgingEvent(Event* event) {
  Guard<CriticalMutex> guard{&lock_};
  ASSERT(!event || !aging_event_);
  aging_event_ = event;
}

void PageQueues::EnableAnonymousReclaim(bool zero_forks) {
  {
    Guard<CriticalMutex> guard{&list_lock_};
    anonymous_is_reclaimable_ = true;
    zero_fork_is_reclaimable_ = zero_forks;

    const PageQueue mru_queue = mru_gen_to_queue();

    // Migrate any existing pages into the reclaimable queues.

    while (!page_queues_[PageQueueAnonymous].is_empty()) {
      vm_page_t* page = &page_queues_[PageQueueAnonymous].front();
      MoveToQueueLockedList(page, mru_queue);
    }
    while (zero_forks && !page_queues_[PageQueueAnonymousZeroFork].is_empty()) {
      vm_page_t* page = &page_queues_[PageQueueAnonymousZeroFork].front();
      MoveToQueueLockedList(page, mru_queue);
    }
  }
  Guard<CriticalMutex> guard{&lock_};
  CheckActiveRatioAgingLocked();
}

ktl::optional<PageQueues::VmoBacklink> PageQueues::GetCowForLoanedPage(vm_page_t* page) {
  DEBUG_ASSERT(page->is_loaned());
  vm_page_state state = page->state();
  switch (state) {
    case vm_page_state::FREE_LOANED:
      // Page is not owned by the page queues, so no cow pages to lookup.
      return ktl::nullopt;
    case vm_page_state::OBJECT: {
      // Delaying the lock acquisition and then reading the object field here is safe since the
      // caller has guaranteed that the page state is not changing, and then the  object field is
      // only modified under lock_, which we will be holding.
      Guard<CriticalMutex> guard{&list_lock_};
      VmCowPages* cow = reinterpret_cast<VmCowPages*>(page->object.get_object());
      if (!cow) {
        // Our examination of the state was racy and this page may or may not be owned by a VMO, but
        // it's not in the page queues. It is the responsibility of the caller to deal with scenario
        // and the fact that once we drop our lock the page could get inserted into the page queues.
        return ktl::nullopt;
      }
      // There is a using/borrowing cow and we know it is still alive as we hold the
      // PageQueues lock, and the cow may not destruct while it still has pages.
      uint64_t page_offset = page->object.get_page_offset();
      VmoBacklink backlink{fbl::MakeRefPtrUpgradeFromRaw(cow, guard), page, page_offset};
      DEBUG_ASSERT(backlink.cow);
      return backlink;
    }
    case vm_page_state::ALLOC:
      // Page is moving between the PMM and a VMO in some direction, but is not in the page
      // queues.
      return ktl::nullopt;
    default:
      // A loaned page in any other state is invalid and represents a programming error or bug.
      panic("Unexpected page state %s for loaned page", page_state_to_string(state));
      return ktl::nullopt;
  }
}
