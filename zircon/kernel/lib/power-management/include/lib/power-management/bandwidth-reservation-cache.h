// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_POWER_MANAGEMENT_INCLUDE_LIB_POWER_MANAGEMENT_BANDWIDTH_RESERVATION_CACHE_H_
#define ZIRCON_KERNEL_LIB_POWER_MANAGEMENT_INCLUDE_LIB_POWER_MANAGEMENT_BANDWIDTH_RESERVATION_CACHE_H_

#include <lib/power-management/types.h>
#include <zircon/assert.h>
#include <zircon/types.h>

#include <ranges>

namespace power_management {

// # Bandwidth Reservation Cache
//
// Bandwidth Reservation Cache (BRC) is an approximation of the robust Bandwidth
// Reservation (BR) algorithm that trades off temporary inaccuracy, compared to
// BR, for increased efficiency and scalability when determining processor
// execution rates from workload bandwidth demands.
//
// ## Instantaneous vs. Reserved Demand
//
// The execution rate R_{p,\omega} of processor p must be sufficient to meet the
// bandwidth demand U_{p,A} of the set of active tasks A running on the
// processor:
//
//   U_{p,A} = \sum_{i \in A} U_i \le R_{p,\omega}
//
// Where the execution rate R_{p,\omega} is in the set of operating points
// available to processor p:
//
//   R_{p,\omega} \in \{ R_{p,1}, R_{p,2}, ..., R_{p,n} \} : 0 \le R_{p,\omega} \le 1
//
// While it is correct to raise the execution rate to the minimum viable
// operating point when instantaneous demand on the processor increases, naively
// decreasing the execution rate when instantaneous demand decreases can result
// in missed deadlines. When task i blocks, the scheduler is still obligated to
// complete the task's remaining work for its pending job J_{i,k} by its finish
// time f_{i,k}. Reducing the execution rate prevents other tasks from getting
// sufficiently ahead using the slack time created by the blocking task, which
// could cause the other tasks to miss their deadlines if the blocking task
// unblocks before its pending job expires.
//
// Instead of immediately removing a blocking task's demand from the processor's
// total demand, the task's demand removal is deferred. The demand is reserved
// until it is certain that the commitment to the pending job has expired or
// migrated to a different processor.
//
// ## Bandwidth Reservation
//
// The robust BR algorithm splits total processor demand into two demand pools:
//
//   U_{p,A}: The sum of demands for the set A of actively runnable tasks.
//   U_{p,D}: The sum of demands for the set D of recently blocked tasks whose
//   pending jobs have not yet expired or migrated to a different processor.
//
// The required processing rate is then determined by the sum of both pools:
//
//   U_{p,A} + U_{p,D} \le R_{p,\omega}
//
// Scheduling and power management operations are modified to manage the demand
// pools:
//
// ### On Task Block
//
// Move the demand U_i of task i from the active pool to the deferred pool:
//
//   U_{p,A} = U_{p,A} - U_i
//   U_{p,D} = U_{p,D} + U_i
//
// Because total demand is unchanged, the required processing rate is also
// unchanged.
//
// ### On Task Wakeup
//
// If the task i still has a pending job (i.e. f_{i,k} has not expired), move
// its demand from the deferred pool of the last processor p it ran on to the
// active pool of the processor q it is currently assigned to, which may be the
// same processor:
//
//   U_{p,D} = U_{p,D} - U_i
//   U_{q,A} = U_{q,A} + U_i
//
// If the total demand of processor q increases, a higher processing rate may be
// required.
//
// ### On Finish Time Expiration
//
// When blocked task i's finish time f_{i,k} passes, its reservation is no
// longer needed and is removed from the deferred pool of the last processor p
// it ran on:
//
//   U_{p,D} = U_{p,D} - U_i
//
// Because the total demand decreases, a lower processing rate may be viable.
//
// ### On Demand Change
//
// If the demand of task i changes while it has a pending job, due to bandwidth
// inheritance or a profile change, either the active demand pool or the
// deferred demand pool of its assigned processor p must be updated:
//
//   \delta U_i = U_i' - U_i
//
//   Runnable: U_{p,A} = U_{p,A} + \delta U_i
//   Blocked:  U_{p,D} = U_{p,D} + \delta U_i
//
// The total demand change may require a change in processing rate.
//
// ## Challenges
//
// The BR algorithm is robust in that it guarantees adequate processor bandwidth
// is available to meet _viable_ workload demands only for as long as absolutely
// necessary. However, its implementation can increase complexity and lock
// contention.
//
// Finish time expiration can be implemented using per-CPU deferred reservation
// queues, which track all of the blocked tasks that most recently ran on a CPU
// having pending finish times that have not expired. The deferred reservation
// queue is similar to a run queue ordered by finish time, and needs to be
// updated when each deferred reservation expires or when a task blocks,
// unblocks, migrates, or changes demand. While the demand queue can reuse the
// same metadata used for run queues, it complicates potential per-thread space
// savings and increases the potential for cross-processor contention when
// removing tasks from the deferral queue during the aforementioned thread
// operations.
//
// ## A Simplifying Local Approximation
//
// The challenges of the BR algorithm can be mitigated using a processor-local
// cache of demand reservations, hence the name Bandwidth Reservation Cache.
// Like BR, BRC maintains active and deferred reservations that sum to determine
// the required processing rate. The key difference with BRC is that deferred
// demand is tracked locally in a limited size cache of recently blocked tasks
// that is not updated when (most) cross-processor thread operations occur. This
// can result in higher apparent bandwidth demand than is strictly necessary,
// but for a limited amount of time.
//
// BRC maintains an array of (i, f_{i,k}, U_i) entries tracking up to N deferred
// bandwidth reservations. Updating the active and deferred bandwidth
// reservations follows a similar process to BR when tasks block and unblock,
// but avoids changing deferred bandwidth reservations for remote processors in
// most cases.
//
template <size_t N>
class BandwidthReservationCache {
  static_assert(N > 0);

  struct Entry {
    Time finish_time{0};
    Utilization utilization{0};
    zx_koid_t tid{ZX_KOID_INVALID};

    constexpr bool is_valid() const { return tid != ZX_KOID_INVALID; }
    constexpr void reset() { tid = ZX_KOID_INVALID; }
  };

 public:
  constexpr BandwidthReservationCache() = default;
  constexpr ~BandwidthReservationCache() = default;

  constexpr BandwidthReservationCache(const BandwidthReservationCache&) = delete;
  constexpr BandwidthReservationCache& operator=(const BandwidthReservationCache&) = default;

  // Adds the given thread, finish time, and utilization to the cache. If the
  // given tid is already in the cache, the entry is updated and the previous
  // utilization is returned.
  //
  // Returns the utilization of the reservation that was replaced or evicted
  // from the cache, if any. This quantity should be removed from the total
  // effective utilization bookkeeping.
  constexpr Utilization Add(zx_koid_t tid, Time finish_time, Utilization utilization) {
    ZX_DEBUG_ASSERT(tid != ZX_KOID_INVALID);
    ZX_DEBUG_ASSERT(utilization >= 0);

    // If there is an existing entry for the tid replace it, otherwise find the
    // first empty element.
    Entry* empty_element = nullptr;
    for (Entry& entry : entries_) {
      if (entry.tid == tid) {
        const Utilization utilization_to_remove = entry.utilization;
        total_deferred_utilization_ += utilization - utilization_to_remove;
        ZX_DEBUG_ASSERT(total_deferred_utilization_ >= 0);

        entry.finish_time = finish_time;
        entry.utilization = utilization;

        return utilization_to_remove;
      }
      if (!empty_element && !entry.is_valid()) {
        empty_element = &entry;
      }
    }

    // If there is an empty element, add the new entry there.
    if (empty_element != nullptr) {
      *empty_element = {.finish_time = finish_time, .utilization = utilization, .tid = tid};

      total_deferred_utilization_ += utilization;
      ZX_DEBUG_ASSERT(total_deferred_utilization_ >= 0);
      return Utilization{0};
    }

    // Replace the max finish time when the cache is full. Since N > 0 there will
    // always be at least one element with the max finish time.
    auto element_to_replace = std::ranges::max_element(entries_, {}, &Entry::finish_time);

    if (element_to_replace->finish_time > finish_time) {
      const Utilization utilization_to_remove = element_to_replace->utilization;
      total_deferred_utilization_ += utilization - utilization_to_remove;
      ZX_DEBUG_ASSERT(total_deferred_utilization_ >= 0);

      *element_to_replace = {.finish_time = finish_time, .utilization = utilization, .tid = tid};

      return utilization_to_remove;
    }

    return utilization;
  }

  // Removes the given tid from the cache, if present.
  //
  // Returns the utilization of the reservation that was removed from the cache,
  // if any.
  constexpr Utilization Remove(zx_koid_t tid) {
    auto entry_to_remove = std::ranges::find(entries_, tid, &Entry::tid);
    if (entry_to_remove != entries_.end()) {
      const Utilization utilization_to_remove = entry_to_remove->utilization;
      total_deferred_utilization_ -= utilization_to_remove;
      ZX_DEBUG_ASSERT(total_deferred_utilization_ >= 0);

      entry_to_remove->reset();
      return utilization_to_remove;
    }
    return Utilization{0};
  }

  // Removes any expired entries with finish times that are less than or equal
  // to the given time.
  //
  // Returns the total utilization of the reservations that were pruned from the
  // cache, if any.
  constexpr Utilization Prune(Time now) {
    Utilization utilization_to_remove{0};

    for (Entry& entry : entries_) {
      if (entry.is_valid() && entry.finish_time <= now) {
        utilization_to_remove += entry.utilization;
        total_deferred_utilization_ -= entry.utilization;
        ZX_DEBUG_ASSERT(total_deferred_utilization_ >= 0);

        entry.reset();
      }
    }

    return utilization_to_remove;
  }

  // Clears all entries in the cache.
  //
  // Returns the total utilization of reservations that were cleared from the
  // cache, if any.
  constexpr Utilization Clear() {
    const Utilization utilization_to_remove = total_deferred_utilization_;
    total_deferred_utilization_ = Utilization{0};

    for (Entry& entry : entries_) {
      entry.reset();
    }

    return utilization_to_remove;
  }

  // Returns to minimum of the given time and the minimum finish time of any
  // active entry in the cache.
  constexpr Time ClampToNextFinishTime(Time time) const {
    // Find the minimum valid finish time.
    auto valid_entries = entries_ | std::views::filter(&Entry::is_valid);
    auto min_element = std::ranges::min_element(valid_entries, {}, &Entry::finish_time);
    if (min_element != valid_entries.end()) {
      return std::min(min_element->finish_time, time);
    }
    return time;
  }

  // Returns to total utilization of the deferred reservations tracked by the
  // cache.
  constexpr Utilization total_deferred_utilization() const { return total_deferred_utilization_; }

 private:
  std::array<Entry, N> entries_{};
  Utilization total_deferred_utilization_{0};
};

}  // namespace power_management

#endif  // ZIRCON_KERNEL_LIB_POWER_MANAGEMENT_INCLUDE_LIB_POWER_MANAGEMENT_BANDWIDTH_RESERVATION_CACHE_H_
