// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-options/boot-options.h>
#include <lib/counters.h>
#include <lib/fit/defer.h>
#include <lib/page/size.h>
#include <lib/zircon-internal/macros.h>

#include <cassert>
#include <cstdint>

#include <kernel/lockdep.h>
#include <ktl/algorithm.h>
#include <vm/compression.h>
#include <vm/discardable_vmo_tracker.h>
#include <vm/evictor.h>
#include <vm/pmm.h>
#include <vm/pmm_node.h>
#include <vm/scanner.h>
#include <vm/vm_cow_pages.h>

#include <ktl/enforce.h>

namespace {

KCOUNTER(pager_backed_pages_evicted, "vm.reclamation.pages_evicted_pager_backed.total")
KCOUNTER(pager_backed_pages_evicted_oom, "vm.reclamation.pages_evicted_pager_backed.oom")
KCOUNTER(compression_evicted, "vm.reclamation.pages_evicted_compressed.total")
KCOUNTER(compression_evicted_oom, "vm.reclamation.pages_evicted_compressed.oom")
KCOUNTER(discardable_pages_evicted, "vm.reclamation.pages_evicted_discardable.total")
KCOUNTER(discardable_pages_evicted_oom, "vm.reclamation.pages_evicted_discardable.oom")

inline void CheckedIncrement(uint64_t* a, uint64_t b) {
  uint64_t result;
  bool overflow = add_overflow(*a, b, &result);
  DEBUG_ASSERT(!overflow);
  *a = result;
}

ktl::optional<ktl::pair<VmCowReclaimResult, const vm_page_t*>> ReclaimFromGlobalPageQueues(
    VmCompression* compression, Evictor::EvictionLevel eviction_level) {
  // Avoid evicting from the newest queue to prevent thrashing.
  const size_t lowest_evict_queue = eviction_level == Evictor::EvictionLevel::IncludeNewest
                                        ? PageQueues::kNumActiveQueues
                                        : PageQueues::kNumReclaim - PageQueues::kNumOldestQueues;
  // If we're going to include newest pages, ignore eviction hints as well, i.e. also consider
  // evicting pages with always_need set if we encounter them in LRU order.
  const VmCowPages::EvictionAction hint_action =
      eviction_level == Evictor::EvictionLevel::IncludeNewest
          ? VmCowPages::EvictionAction::IgnoreHint
          : VmCowPages::EvictionAction::FollowHint;

  if (ktl::optional<PageQueues::VmoBacklink> backlink =
          pmm_page_queues()->PeekIsolate(lowest_evict_queue)) {
    // A valid backlink always has a valid cow
    DEBUG_ASSERT(backlink->cow);

    ktl::optional<VmCompression::CompressorGuard> maybe_instance;
    VmCompressor* compression_instance = nullptr;
    if (compression) {
      maybe_instance.emplace(compression->AcquireCompressor());
      compression_instance = &maybe_instance->get();
      zx_status_t status = compression_instance->Arm();
      if (status != ZX_OK) {
        return ktl::nullopt;
      }
    }

    VmCowReclaimResult reclaimed = backlink->cow->ReclaimPage(backlink->page, backlink->offset,
                                                              hint_action, compression_instance);
    return ktl::make_pair(reclaimed, backlink->page);
  }
  return ktl::nullopt;
}

struct ReclaimFailureStats {
  static constexpr uint64_t kComparisonBase = 1000;
  uint64_t consecutive_reclaim_failures = 0;
  uint64_t failures_to_compare = kComparisonBase;

  uint64_t prev_page_evictions = 0;
  const vm_page_t* prev_evicted_page = nullptr;
  // VmCowReclaimResult doesn't have a default constructor. Initialize to an arbitrary value; this
  // will be overwritten anyway by the actual result of eviction when one happens.
  VmCowReclaimResult prev_eviction_result = fit::ok(VmCowReclaimSuccess{
      .type = VmCowReclaimSuccess::Type::Evict, .num_pages = 0, .num_loaned_pages = 0});
  bool printed_same_page_log = false;

  struct FailureReasons {
    uint64_t compress_failed = 0;
    uint64_t compress_accessed = 0;
    uint64_t evict_accessed = 0;
    uint64_t incorrect_page = 0;
    uint64_t other = 0;
  };
  FailureReasons failure_reasons = {};

  void Update(bool reclaim_failed) {
    if (reclaim_failed) {
      consecutive_reclaim_failures++;
    } else {
      consecutive_reclaim_failures = 0;
      failures_to_compare = kComparisonBase;
      failure_reasons = {};
    }
  }

  bool ShouldPrintLivelock() {
    if (consecutive_reclaim_failures == failures_to_compare) {
      // Wait for longer before logging the next time to avoid spamming in an actual livelock.
      // Multiply checking for overflow.
      if (failures_to_compare < failures_to_compare * 10) {
        failures_to_compare *= 10;
      }
      return true;
    }
    return false;
  }

  static const char* ToResultString(VmCowReclaimResult result) {
    if (result.is_error()) {
      switch (result.error_value()) {
        case VmCowReclaimFailure::CompressFailed:
          return "fail:compress";
        case VmCowReclaimFailure::CompressAccessed:
          return "fail:access_c";
        case VmCowReclaimFailure::EvictAccessed:
          return "fail:access_e";
        case VmCowReclaimFailure::IncorrectPage:
          return "fail:incorrect";
        case VmCowReclaimFailure::Other:
          return "fail:other";
      }
    }
    switch (result->type) {
      case VmCowReclaimSuccess::Type::Compress:
        return "ok:compress";
      case VmCowReclaimSuccess::Type::Discard:
        return "ok:discard";
      case VmCowReclaimSuccess::Type::Evict:
        return "ok:evict";
    }
    __UNREACHABLE;
  }

  void CheckForSamePage(ktl::pair<VmCowReclaimResult, const vm_page_t*> reclaimed) {
    const vm_page_t* evicted_page = reclaimed.second;
    // Wait until the very end to update prev_evicted_page and prev_eviction_result, because we will
    // use the old values to perform the check.
    auto update_prev = fit::defer([&] {
      prev_evicted_page = evicted_page;
      prev_eviction_result = reclaimed.first;
    });

    if (likely(evicted_page != prev_evicted_page)) {
      return;
    }

    // Evicting the same page twice in a row indicates a potential bug in reclamation, unless we're
    // in the IncorrectPage failure case, since that indicates a race in page ownership, and we
    // can't expect the VmCowPages to have moved a page it does not own out of the way. Every other
    // failure case should have moved the page out of the way (by calling MarkAccessed), so we
    // should not see the same page in the next eviction attempt.
    //
    // It is also possible to see the same page in rare hard-to-check-for cases after having been
    // reclaimed and reused. See the discussion in https://fxbug.dev/434361683.
    if (prev_eviction_result.is_error() &&
        prev_eviction_result.error_value() == VmCowReclaimFailure::IncorrectPage) {
      return;
    }

    prev_page_evictions++;
    // Print only once per eviction attempt to prevent log spam.
    if (!printed_same_page_log) {
      printed_same_page_log = true;
      printf("Evictor reclaiming the same page again %p [prev %s cur %s]\n", evicted_page,
             ToResultString(prev_eviction_result), ToResultString(reclaimed.first));
    }
  }
};

void DiagnoseReclamationFailure(ktl::pair<VmCowReclaimResult, const vm_page_t*>* reclaimed,
                                ReclaimFailureStats* failure_stats, bool has_test_reclaim) {
  bool reclaim_failed = false;
  if (reclaimed->first.is_error()) {
    reclaim_failed = true;
    switch (reclaimed->first.error_value()) {
      case VmCowReclaimFailure::CompressFailed:
        failure_stats->failure_reasons.compress_failed++;
        break;
      case VmCowReclaimFailure::CompressAccessed:
        failure_stats->failure_reasons.compress_accessed++;
        break;
      case VmCowReclaimFailure::EvictAccessed:
        failure_stats->failure_reasons.evict_accessed++;
        break;
      case VmCowReclaimFailure::IncorrectPage:
        failure_stats->failure_reasons.incorrect_page++;
        break;
      case VmCowReclaimFailure::Other:
        failure_stats->failure_reasons.other++;
        break;
    }
  }

  // The test reclaim path might not even evict an actual page, so there's nothing to check.
  if (!has_test_reclaim) {
    failure_stats->CheckForSamePage(*reclaimed);
  }

  // If we've looped many times without being able to reclaim anything, make some noise.
  failure_stats->Update(reclaim_failed);
  if (unlikely(failure_stats->ShouldPrintLivelock())) {
    printf(
        "Evictor failed %zu reclaims in a row (%zu for prev page), possible livelock\n"
        "compress_failed %zu compress_accessed %zu evict_accessed %zu incorrect_page %zu other %zu\n",
        failure_stats->consecutive_reclaim_failures, failure_stats->prev_page_evictions,
        failure_stats->failure_reasons.compress_failed,
        failure_stats->failure_reasons.compress_accessed,
        failure_stats->failure_reasons.evict_accessed,
        failure_stats->failure_reasons.incorrect_page, failure_stats->failure_reasons.other);
    pmm_page_queues()->Dump();
  }
}

}  // namespace

// static
Evictor::EvictorStats Evictor::GetGlobalStats() {
  EvictorStats stats;
  stats.pager_backed_oom = pager_backed_pages_evicted_oom.SumAcrossAllCpus();
  stats.pager_backed_other = pager_backed_pages_evicted.SumAcrossAllCpus() - stats.pager_backed_oom;
  stats.compression_oom = compression_evicted_oom.SumAcrossAllCpus();
  stats.compression_other = compression_evicted.SumAcrossAllCpus() - stats.compression_oom;
  stats.discarded_oom = discardable_pages_evicted_oom.SumAcrossAllCpus();
  stats.discarded_other = discardable_pages_evicted.SumAcrossAllCpus() - stats.discarded_oom;
  return stats;
}

Evictor::Evictor() : Evictor(nullptr, nullptr) {}

Evictor::Evictor(ReclaimFunction reclaim_function, FreePagesFunction free_pages_function)
    : test_reclaim_function_(ktl::move(reclaim_function)),
      test_free_pages_function_(ktl::move(free_pages_function)) {}

Evictor::~Evictor() { DisableEviction(); }

bool Evictor::IsEvictionEnabled() const {
  Guard<MonitoredSpinLock, IrqSave> guard{&lock_, SOURCE_TAG};
  return eviction_enabled_;
}

bool Evictor::IsCompressionEnabled() const {
  Guard<MonitoredSpinLock, IrqSave> guard{&lock_, SOURCE_TAG};
  return use_compression_;
}

void Evictor::EnableEviction(bool use_compression) {
  {
    Guard<MonitoredSpinLock, IrqSave> guard{&lock_, SOURCE_TAG};
    // It's an error to call this whilst the eviction thread is still exiting.
    ASSERT(!eviction_thread_exiting_);
    eviction_enabled_ = true;
    use_compression_ = use_compression;

    if (eviction_thread_) {
      return;
    }
  }

  // Set up the eviction thread to process asynchronous eviction requests.
  auto eviction_thread = [](void* arg) -> int {
    Evictor* evictor = reinterpret_cast<Evictor*>(arg);
    return evictor->EvictionThreadLoop();
  };
  eviction_thread_ = Thread::Create("eviction-thread", eviction_thread, this, DEFAULT_PRIORITY);
  DEBUG_ASSERT(eviction_thread_);
  eviction_thread_->Resume();
}

void Evictor::DisableEviction() {
  Thread* eviction_thread = nullptr;
  {
    // Grab the lock and update any state. We cannot actually wait for the eviction thread to
    // complete whilst the lock is held, however.
    Guard<MonitoredSpinLock, IrqSave> guard{&lock_, SOURCE_TAG};
    if (!eviction_thread_) {
      return;
    }
    // It's an error to call this in parallel with another DisableEviction call.
    ASSERT(!eviction_thread_exiting_);
    eviction_thread = eviction_thread_;
    eviction_thread_exiting_ = true;
    eviction_signal_.Signal();
  }
  // Now with the lock dropped wait for the thread to complete. Use a locally cached copy of the
  // pointer so that even if the scanner performs a concurrent EnableEviction call we should not
  // crash or have races, although the eviction thread may fail to join.
  int res = 0;
  eviction_thread->Join(&res, ZX_TIME_INFINITE);
  DEBUG_ASSERT(res == 0);
  {
    Guard<MonitoredSpinLock, IrqSave> guard{&lock_, SOURCE_TAG};
    // Now update the state to indicate that eviction is disabled.
    eviction_thread_ = nullptr;
    eviction_enabled_ = false;
    eviction_thread_exiting_ = false;
  }
}

Evictor::EvictionTarget Evictor::DebugGetEvictionTarget() const {
  Guard<MonitoredSpinLock, IrqSave> guard{&lock_, SOURCE_TAG};
  return eviction_target_;
}

void Evictor::CombineEvictionTarget(EvictionTarget target) {
  Guard<MonitoredSpinLock, IrqSave> guard{&lock_, SOURCE_TAG};
  eviction_target_.pending = eviction_target_.pending || target.pending;
  eviction_target_.level = ktl::max(eviction_target_.level, target.level);
  CheckedIncrement(&eviction_target_.min_pages_to_free, target.min_pages_to_free);
  eviction_target_.free_pages_target =
      ktl::max(eviction_target_.free_pages_target, target.free_pages_target);
  eviction_target_.print_counts = eviction_target_.print_counts || target.print_counts;
}

Evictor::EvictionResult Evictor::EvictFromExternalTarget(Evictor::EvictionTarget target) {
  return EvictFromTargetInternal(target);
}

Evictor::EvictedPageCounts Evictor::EvictFromPreloadedTarget() {
  // Create a local copy of the eviction target to operate against.
  EvictionTarget target;
  {
    Guard<MonitoredSpinLock, IrqSave> guard{&lock_, SOURCE_TAG};
    target = eviction_target_;
  }
  EvictionResult result = EvictFromTargetInternal(target);
  {
    Guard<MonitoredSpinLock, IrqSave> guard{&lock_, SOURCE_TAG};
    uint64_t total = result.counts.non_loaned_total();
    // Clear the eviction target but retain any min pages that we might still need to free in a
    // subsequent eviction attempt.
    eviction_target_ = {};
    eviction_target_.min_pages_to_free =
        (total < target.min_pages_to_free) ? target.min_pages_to_free - total : 0;
  }
  return result.counts;
}

Evictor::EvictionResult Evictor::EvictFromTargetInternal(Evictor::EvictionTarget target) {
  if (!target.pending) {
    return EvictionResult{};
  }

  const uint64_t free_pages_before = CountFreePages();
  EvictionResult result = EvictUntilTargetsMet(target.min_pages_to_free, target.free_pages_target,
                                               target.level, target.oom_trigger);
  const uint64_t free_pages_after = CountFreePages();

  if (target.print_counts) {
    // Helper lambdas to print the counts as values in KB or MB.
    auto format_val = [](uint64_t count) {
      return count < MB / kPageSize ? count * kPageSize / KB : count * kPageSize / MB;
    };
    auto format_unit = [](uint64_t count) { return count < MB / kPageSize ? "K" : "M"; };

    // This should be large enough to hold the formatted string.
    constexpr size_t kBufSize = 56;
    char buf[kBufSize] __UNINITIALIZED = "\0";
    size_t buf_len = 0;
    if (result.counts.pager_backed > 0) {
      buf_len +=
          snprintf(buf + buf_len, kBufSize - buf_len, " pager:%zu%s",
                   format_val(result.counts.pager_backed), format_unit(result.counts.pager_backed));
    }
    if (result.counts.discardable > 0) {
      ASSERT(buf_len < kBufSize);
      buf_len +=
          snprintf(buf + buf_len, kBufSize - buf_len, " discardable:%zu%s",
                   format_val(result.counts.discardable), format_unit(result.counts.discardable));
    }
    if (result.counts.compressed > 0) {
      ASSERT(buf_len < kBufSize);
      buf_len +=
          snprintf(buf + buf_len, kBufSize - buf_len, " compressed:%zu%s",
                   format_val(result.counts.compressed), format_unit(result.counts.compressed));
    }
    if (buf_len > 0) {
      printf("[EVICT]:%s free:%zuM->%zuM\n", buf, free_pages_before * kPageSize / MB,
             free_pages_after * kPageSize / MB);
    }
  }

  if (target.oom_trigger) {
    pager_backed_pages_evicted_oom.Add(static_cast<int64_t>(result.counts.pager_backed));
    compression_evicted_oom.Add(static_cast<int64_t>(result.counts.compressed));
    discardable_pages_evicted_oom.Add(static_cast<int64_t>(result.counts.discardable));
  }

  return result;
}

Evictor::EvictionResult Evictor::EvictSynchronous(uint64_t min_mem_to_free,
                                                  uint64_t free_mem_target,
                                                  EvictionLevel eviction_level, Output output,
                                                  TriggerReason reason) {
  if (!IsEvictionEnabled()) {
    return EvictionResult{};
  }
  EvictionTarget target = {
      .pending = true,
      .free_pages_target = free_mem_target / kPageSize,
      .min_pages_to_free = min_mem_to_free / kPageSize,
      .level = eviction_level,
      .print_counts = (output == Output::Print),
      .oom_trigger = (reason == TriggerReason::OOM),
  };

  auto eviction_result = EvictFromExternalTarget(target);
  return eviction_result;
}

void Evictor::EvictAsynchronous(uint64_t min_mem_to_free, uint64_t free_mem_target,
                                Evictor::EvictionLevel eviction_level, Evictor::Output output) {
  if (!IsEvictionEnabled()) {
    return;
  }
  CombineEvictionTarget(Evictor::EvictionTarget{
      .pending = true,
      .free_pages_target = free_mem_target / kPageSize,
      .min_pages_to_free = min_mem_to_free / kPageSize,
      .level = eviction_level,
      .print_counts = (output == Output::Print),
  });
  // Unblock the eviction thread.
  eviction_signal_.Signal();
}

Evictor::EvictionResult Evictor::EvictUntilTargetsMet(uint64_t min_pages_to_evict,
                                                      uint64_t free_pages_target,
                                                      EvictionLevel level, bool oom_trigger) {
  if (!IsEvictionEnabled()) {
    return EvictionResult{};
  }

  // Helper to read the total_evicted_ counts under the lock.
  auto read_counts = [&]() {
    Guard<Mutex> guard{&eviction_lock_};
    return total_evicted_;
  };

  // Take a snapshot of the eviction counts at the beginning of our attempts.
  const EvictedPageCounts starting_counts = read_counts();

  bool free_target_reached = false;
  while (true) {
    // Lock the eviction attempts so that we don't overshoot the free pages target.
    Guard<Mutex> evict_guard{&eviction_lock_};

    const uint64_t pages_freed_so_far =
        total_evicted_.non_loaned_total() - starting_counts.non_loaned_total();

    const uint64_t free_pages = CountFreePages();
    uint64_t pages_to_free = 0;
    // Need to evict at least min_pages_to_evict, and then potentially up to the free_pages_target.
    if (pages_freed_so_far < min_pages_to_evict) {
      pages_to_free = min_pages_to_evict - pages_freed_so_far;
    }
    if (free_pages < free_pages_target) {
      pages_to_free = ktl::max(free_pages_target - free_pages, pages_to_free);
    } else {
      free_target_reached = true;
    }
    if (pages_to_free == 0) {
      // The targets have been met. No more eviction is required right now.
      break;
    }

    // Cap the number of pages to free so that we do not monopolize the eviction_lock_, ensuring
    // that parallel attempts at achieving different eviction outcomes do not unnecessarily block
    // each other.
    //
    // For OOM evictions we do not cap the number of pages so that we can evict as fast as possible
    // to resolve the OOM condition quickly.
    if (!oom_trigger) {
      pages_to_free = ktl::min(128ul, pages_to_free);
    }

    EvictedPageCounts pages_freed = EvictPageQueues(pages_to_free, level);
    // Should we fail to free any pages then we give up and consider the eviction request complete.
    if (pages_freed.non_loaned_total() == 0) {
      break;
    }
    total_evicted_ += pages_freed;
  }

  // Return the difference of the current eviction counts compared to when we started.
  return EvictionResult{.counts = read_counts() - starting_counts,
                        .free_target_reached = free_target_reached};
}

Evictor::EvictedPageCounts Evictor::EvictPageQueues(uint64_t target_pages,
                                                    EvictionLevel eviction_level) const {
  EvictedPageCounts counts = {};

  if (!IsEvictionEnabled()) {
    return counts;
  }

  VmCompression* compression = nullptr;
  if (IsCompressionEnabled()) {
    compression = Pmm::Node().GetPageCompression();
  }

  ReclaimFailureStats failure_stats;
  // Evict until we've counted enough pages to hit the target_pages. Explicitly do not consider
  // pager_backed_loaned towards our total, as loaned pages do not go to the free memory pool.
  while (counts.non_loaned_total() < target_pages) {
    // Use the helper to perform a single 'step' of eviction.
    auto reclaimed = EvictPageQueuesHelper(compression, eviction_level);
    // An empty return from the helper indicates that there are no more eviction candidates, so
    // regardless of our desired target we must give up.
    if (!reclaimed.has_value()) {
      break;
    }

    // Check for any reclamation failures and print diagnostic logs if needed.
    DiagnoseReclamationFailure(&reclaimed.value(), &failure_stats, !!test_reclaim_function_);

    VmCowReclaimResult reclaimed_result = reclaimed->first;
    if (reclaimed_result.is_ok()) {
      uint64_t num_pages = reclaimed_result.value().num_pages;
      uint64_t num_loaned_pages = reclaimed_result.value().num_loaned_pages;

      switch (reclaimed_result.value().type) {
        case VmCowReclaimSuccess::Type::Evict:
          counts.pager_backed += num_pages;
          counts.pager_backed_loaned += num_loaned_pages;
          break;
        case VmCowReclaimSuccess::Type::Discard:
          DEBUG_ASSERT(num_loaned_pages == 0);
          counts.discardable += num_pages;
          break;
        case VmCowReclaimSuccess::Type::Compress:
          DEBUG_ASSERT(num_loaned_pages == 0);
          counts.compressed += num_pages;
          break;
      }
    }
  }

  pager_backed_pages_evicted.Add(counts.pager_backed + counts.pager_backed_loaned);
  compression_evicted.Add(counts.compressed);
  return counts;
}

int Evictor::EvictionThreadLoop() {
  while (!eviction_thread_exiting_) {
    eviction_signal_.Wait();

    if (eviction_thread_exiting_) {
      break;
    }

    // Process an eviction target if there is one. This is a no-op and no pages are evicted if no
    // target is pending.
    EvictFromPreloadedTarget();
  }
  return 0;
}

uint64_t Evictor::CountFreePages() const {
  if (unlikely(test_free_pages_function_)) {
    return test_free_pages_function_();
  }
  return pmm_count_free_pages();
}

ktl::optional<ktl::pair<VmCowReclaimResult, const vm_page_t*>> Evictor::EvictPageQueuesHelper(
    VmCompression* compression, EvictionLevel eviction_level) const {
  if (unlikely(test_reclaim_function_)) {
    auto test_ret_val = test_reclaim_function_(compression, eviction_level);
    if (test_ret_val.has_value()) {
      return ktl::make_pair(test_ret_val.value(), nullptr);
    }
    return ktl::nullopt;
  }
  return ReclaimFromGlobalPageQueues(compression, eviction_level);
}
