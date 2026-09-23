// Copyright 2018 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT
#ifndef ZIRCON_KERNEL_INCLUDE_KERNEL_SCHEDULER_STATE_H_
#define ZIRCON_KERNEL_INCLUDE_KERNEL_SCHEDULER_STATE_H_

#include <lib/fxt/argument.h>
#include <lib/power-management/energy-model.h>
#include <lib/sched/time.h>
#include <lib/zircon-internal/macros.h>
#include <stddef.h>
#include <stdint.h>
#include <zircon/syscalls/scheduler.h>
#include <zircon/types.h>

#include <cstdint>

#include <fbl/enum_bits.h>
#include <fbl/intrusive_wavl_tree.h>
#include <ffl/fixed.h>
#include <ffl/string.h>
#include <kernel/cpu.h>
#include <kernel/spinlock.h>
#include <ktl/algorithm.h>
#include <ktl/limits.h>
#include <ktl/type_traits.h>
#include <ktl/utility.h>
#include <ktl/variant.h>

// Forward declarations.
struct Thread;
namespace unittest {
class ThreadEffectiveProfileObserver;
}

#ifndef SCHEDULER_EXTRA_INVARIANT_VALIDATION
#define SCHEDULER_EXTRA_INVARIANT_VALIDATION false
#endif

inline constexpr bool kSchedulerExtraInvariantValidation = SCHEDULER_EXTRA_INVARIANT_VALIDATION;

enum thread_state : uint8_t {
  THREAD_INITIAL = 0,
  THREAD_READY,
  THREAD_RUNNING,
  THREAD_BLOCKED,
  THREAD_BLOCKED_READ_LOCK,
  THREAD_SLEEPING,
  THREAD_SUSPENDED,
  THREAD_DEATH,
};

// Fixed-point task weight.
//
// The 10bit fractional component accommodates the exponential curve defining
// the priority-to-weight relation:
//
//      Weight = 1.1^(Priority - 31)
//
// This yields roughly a 4-5% bandwidth difference between adjacent priorities.
//
// Weights should not be negative, however, the value is signed for consistency
// with zx_instant_mono_t (SchedTime) and zx_duration_mono_t (SchedDuration),
// which are the primary types used in conjunction with SchedWeight. This is to
// make it less likely that expressions involving weights are accidentally
// promoted to unsigned.
using SchedWeight = ffl::Fixed<int64_t, 10>;

// Fixed-point utilization factor. Represents the ratio between capacity and
// period or capacity and relative deadline, depending on which type of
// utilization is being evaluated.
//
// The 31bit fractional component represents the utilization with a precision
// of ~0.5ns.
using SchedUtilization = ffl::Fixed<int64_t, 31>;

// Ensure these types stay in sync with lib/power-management.
static_assert(ktl::is_same_v<SchedUtilization, power_management::Utilization>);

// Fixed-point types wrapping time and duration types to make time expressions
// cleaner in the scheduler code.
using SchedDuration = ffl::Fixed<zx_duration_mono_t, 0>;
using SchedTime = ffl::Fixed<zx_instant_mono_t, 0>;

// Ensure these types stay in sync with lib/sched.
static_assert(ktl::is_same_v<SchedDuration, sched::Duration>);
static_assert(ktl::is_same_v<SchedTime, sched::Time>);

// A smaller duration type that can represent up to ~2.15 seconds. This is
// longer than any useful deadline scheduling period.
// TODO(eieio): Use this type more extensively for profile parameter values to
// save space.
using SchedCompactDuration = ffl::Fixed<int32_t, 0>;

// Simplify trace event arguments by automatically converting fixed point values with zero
// fractional bits to the corresponding integral records.
namespace fxt {

constexpr Argument<ArgumentType::kInt64, RefType::kId> MakeArgument(StringRef<RefType::kId> name,
                                                                    ffl::Fixed<int64_t, 0> value) {
  return {name, value.raw_value()};
}
constexpr Argument<ArgumentType::kUint64, RefType::kId> MakeArgument(
    StringRef<RefType::kId> name, ffl::Fixed<uint64_t, 0> value) {
  return {name, value.raw_value()};
}
constexpr Argument<ArgumentType::kInt32, RefType::kId> MakeArgument(StringRef<RefType::kId> name,
                                                                    ffl::Fixed<int32_t, 0> value) {
  return {name, value.raw_value()};
}
constexpr Argument<ArgumentType::kUint32, RefType::kId> MakeArgument(
    StringRef<RefType::kId> name, ffl::Fixed<uint32_t, 0> value) {
  return {name, value.raw_value()};
}

// Until the FXT format supports fixed-point argument values, scale utilization
// values by 1000 to improve the interpretability in trace event arguments. Use
// int32_t for the more compact encoding, since visualized utilization values do
// not need high precision and will not typically have large integral
// components.
constexpr Argument<ArgumentType::kInt32, RefType::kId> MakeArgument(StringRef<RefType::kId> name,
                                                                    SchedUtilization value) {
  return {name, ffl::Round<int32_t>(value * 1000)};
}

}  // namespace fxt

namespace internal {
// Conversion table entry. Scales the integer argument to a fixed-point weight
// in the interval (0.0, 1.0].
struct WeightTableEntry {
  constexpr WeightTableEntry(int64_t value)
      : value{ffl::FromRatio<int64_t>(value, SchedWeight::Format::Power)} {}
  constexpr operator SchedWeight() const { return value; }
  const SchedWeight value;
};

// Table of fixed-point constants converting from kernel priority to fair
// scheduler weight.
inline constexpr WeightTableEntry kPriorityToWeightTable[] = {
    53,  58,  64,  71,  78,  85,  94,  103, 114, 125, 138, 152, 167, 184, 202, 222,
    245, 269, 296, 326, 358, 394, 434, 477, 525, 578, 635, 699, 769, 846, 930, 1024,
};
}  // namespace internal

// Represents the key deadline scheduler parameters using fixed-point types.
// This is a fixed point version of the ABI type zx_sched_deadline_params_t that
// makes expressions in the scheduler logic less verbose.
struct SchedDeadlineParams {
  SchedDuration capacity_ns{0};
  SchedDuration deadline_ns{0};
  SchedUtilization utilization{0};

  constexpr SchedDeadlineParams() = default;
  constexpr SchedDeadlineParams(SchedDuration capacity_ns, SchedDuration deadline_ns)
      : capacity_ns{capacity_ns}, deadline_ns{deadline_ns}, utilization{capacity_ns / deadline_ns} {
    DEBUG_ASSERT(0 < capacity_ns);
    DEBUG_ASSERT(capacity_ns <= deadline_ns);
  }

  constexpr SchedDeadlineParams(SchedDuration capacity_ns, SchedDuration deadline_ns,
                                SchedUtilization utilization)
      : capacity_ns{capacity_ns}, deadline_ns{deadline_ns}, utilization{utilization} {
    DEBUG_ASSERT(0 < capacity_ns);
    DEBUG_ASSERT(capacity_ns <= deadline_ns);
    // Because different code paths either use capacity and deadline to derive
    // utilization or utilization and deadline to derive capacity, there can be
    // small differences, due to rounding, between the resulting utilization
    // values, even when the capacity and deadline values are the same here.
    // Checking consistency requires verifying that at least one relationship is
    // valid.
    DEBUG_ASSERT_MSG(
        utilization == capacity_ns / deadline_ns || capacity_ns == utilization * deadline_ns,
        "%" PRId64 " = %" PRId64 "/%" PRId64 " ; %" PRId64 " = %" PRId64 "*%" PRId64,
        SchedUtilization{capacity_ns / deadline_ns}.raw_value(), capacity_ns.raw_value(),
        deadline_ns.raw_value(), SchedDuration{utilization * deadline_ns}.raw_value(),
        utilization.raw_value(), deadline_ns.raw_value());
  }

  constexpr SchedDeadlineParams(const SchedDeadlineParams&) = default;
  constexpr SchedDeadlineParams& operator=(const SchedDeadlineParams&) = default;

  constexpr SchedDeadlineParams(const zx_sched_deadline_params_t& params)
      : capacity_ns{params.capacity},
        deadline_ns{params.relative_deadline},
        utilization{capacity_ns / deadline_ns} {
    DEBUG_ASSERT(0 < capacity_ns);
    DEBUG_ASSERT(capacity_ns <= deadline_ns);
  }
  constexpr SchedDeadlineParams& operator=(const zx_sched_deadline_params_t& params) {
    *this = SchedDeadlineParams{params};
    return *this;
  }

  friend bool operator==(SchedDeadlineParams a, SchedDeadlineParams b) {
    return a.capacity_ns == b.capacity_ns && a.deadline_ns == b.deadline_ns;
  }
  friend bool operator!=(SchedDeadlineParams a, SchedDeadlineParams b) { return !(a == b); }
};

// Utilities that return fixed-point Expression representing the given integer
// time units in terms of system time units (nanoseconds).
template <typename T>
constexpr auto SchedNs(T nanoseconds) {
  return ffl::FromInteger(ZX_NSEC(nanoseconds));
}
template <typename T>
constexpr auto SchedUs(T microseconds) {
  return ffl::FromInteger(ZX_USEC(microseconds));
}
template <typename T>
constexpr auto SchedMs(T milliseconds) {
  return ffl::FromInteger(ZX_MSEC(milliseconds));
}

static constexpr SchedDuration SchedDefaultFairPeriod = SchedMs(10);

// Specifies the type of scheduling algorithm applied to a thread.
enum class SchedDiscipline {
  Fair,
  Deadline,
};

// Per-thread state used by the unified version of Scheduler.
class SchedulerState {
 public:
  // The key type of this node operated on by WAVLTree.
  using KeyType = ktl::pair<SchedTime, uintptr_t>;

  struct BaseProfile {
    constexpr BaseProfile() : fair{} {}

    explicit constexpr BaseProfile(int priority, bool inheritable = true)
        : discipline{SchedDiscipline::Fair},
          inheritable{inheritable},
          fair{.weight{SchedulerState::ConvertPriorityToWeight(priority)}} {}
    explicit constexpr BaseProfile(SchedWeight weight, bool inheritable = true)
        : inheritable{inheritable}, fair{.weight{weight}} {}
    explicit constexpr BaseProfile(SchedDeadlineParams deadline_params, bool critical = false)
        : discipline{SchedDiscipline::Deadline},
          inheritable{true},  // Deadline profiles are always inheritable.
          critical{critical},
          deadline{deadline_params} {}

    constexpr bool IsFair() const { return discipline == SchedDiscipline::Fair; }
    constexpr bool IsDeadline() const { return discipline == SchedDiscipline::Deadline; }

    SchedDiscipline discipline{SchedDiscipline::Fair};
    bool inheritable{true};
    bool critical{false};

    union {
      struct {
        SchedWeight weight{0};
      } fair;
      SchedDeadlineParams deadline;
    };
  };

  enum class ProfileDirtyFlag {
    Clean = 0,
    BaseDirty = 1,
    InheritedDirty = 2,
  };

  // TODO(johngro) 2022-09-21:
  //
  // This odd pattern requires some explanation.  Typically, we would use full
  // specialization in order to control at compile time whether or not
  // dirty-tracking was enabled. So, we would have:
  //
  // ```
  // template <bool Enable>
  // class Tracker { // disabled tracker impl };
  //
  // template <>
  // class Tracker<true> { // enabled tracker impl };
  // ```
  //
  // Unfortunately, there is a bug in GCC which prevents us from doing this in
  // the logical way.  Specifically, GCC does not currently allow full
  // specialization of classes declared in class/struct scope, even though this
  // should be supported as of C++17 (which the kernel is currently using).
  //
  // The bug writeup is here:
  // https://gcc.gnu.org/bugzilla/show_bug.cgi?id=85282
  //
  // It has been confirmed as a real bug, but it has been open for over 4 years
  // now.  The most recent update was about 6 months ago, and it basically said
  // "well, the fix is not going to make it into GCC 12".
  //
  // So, we are using the workaround suggested in the bug's discussion during
  // the most recent update.  Instead of using full specialization as we
  // normally would, we use an odd form of partial specialization instead, which
  // basically boils down to the same thing.
  //
  // If/when GCC finally fixes this, we can come back here and fix this.
  //
  template <bool EnableDirtyTracking, typename = void>
  class EffectiveProfileDirtyTracker {
   public:
    static inline constexpr bool kDirtyTrackingEnabled = false;
    void MarkBaseProfileChanged() {}
    void MarkInheritedProfileChanged() {}
    void Clean() {}
    void AssertDirtyState(ProfileDirtyFlag) const {}
    void AssertDirty() const {}
    ProfileDirtyFlag dirty_flags() const { return ProfileDirtyFlag::Clean; }
  };

  template <bool EnableDirtyTracking>
  class EffectiveProfileDirtyTracker<EnableDirtyTracking,
                                     std::enable_if_t<EnableDirtyTracking == true>> {
   public:
    static inline constexpr bool kDirtyTrackingEnabled = true;
    inline void MarkBaseProfileChanged();
    inline void MarkInheritedProfileChanged();
    void Clean() { dirty_flags_ = ProfileDirtyFlag::Clean; }
    void AssertDirtyState(ProfileDirtyFlag expected) const {
      ASSERT_MSG(expected == dirty_flags_, "Expected %u, Observed %u",
                 static_cast<uint32_t>(expected), static_cast<uint32_t>(dirty_flags_));
    }
    void AssertDirty() const {
      ASSERT_MSG(ProfileDirtyFlag::Clean != dirty_flags_, "Expected != 0, Observed %u",
                 static_cast<uint32_t>(dirty_flags_));
    }
    ProfileDirtyFlag dirty_flags() const { return dirty_flags_; }

   private:
    ProfileDirtyFlag dirty_flags_{ProfileDirtyFlag::Clean};
  };

  struct EffectiveProfile
      : public EffectiveProfileDirtyTracker<kSchedulerExtraInvariantValidation> {
    EffectiveProfile() = default;

    EffectiveProfile(const EffectiveProfile&) = default;
    EffectiveProfile& operator=(const EffectiveProfile&) = default;

    explicit constexpr EffectiveProfile(const BaseProfile& base_profile) {
      if (base_profile.IsFair()) {
        DEBUG_ASSERT(base_profile.fair.weight >= kMinFairWeight);
        weight_ = base_profile.fair.weight;
      } else {
        DEBUG_ASSERT(base_profile.deadline.capacity_ns <= SchedCompactDuration::Max());
        DEBUG_ASSERT(base_profile.deadline.deadline_ns <= SchedCompactDuration::Max());
        weight_ = kMaxDeadlineWeight;
        capacity_ns_ = SchedCompactDuration{base_profile.deadline.capacity_ns};
        deadline_ns_ = SchedCompactDuration{base_profile.deadline.deadline_ns};
        utilization_ = base_profile.deadline.utilization;
        critical_ = base_profile.critical;
      }
    }

    constexpr SchedDiscipline discipline() const {
      return weight_ >= kMinFairWeight ? SchedDiscipline::Fair : SchedDiscipline::Deadline;
    }

    constexpr bool IsFair() const { return discipline() == SchedDiscipline::Fair; }
    constexpr bool IsDeadline() const { return discipline() == SchedDiscipline::Deadline; }

    constexpr void SetFair(SchedWeight weight) {
      DEBUG_ASSERT(weight >= kMinFairWeight);
      weight_ = weight;
    }
    constexpr void SetDeadline(SchedDeadlineParams deadline_params, bool is_critical) {
      weight_ = kMaxDeadlineWeight;
      capacity_ns_ = SchedCompactDuration{deadline_params.capacity_ns};
      deadline_ns_ = SchedCompactDuration{deadline_params.deadline_ns};
      utilization_ = deadline_params.utilization;
      critical_ = is_critical;
    }

    constexpr SchedWeight weight() const {
      DEBUG_ASSERT(IsFair());
      return weight_;
    }
    constexpr SchedWeight weight_or(SchedWeight alternative) const {
      return IsFair() ? weight_ : alternative;
    }
    constexpr SchedDeadlineParams deadline() const {
      DEBUG_ASSERT(IsDeadline());
      return SchedDeadlineParams{SchedDuration{capacity_ns_}, SchedDuration{deadline_ns_},
                                 utilization_};
    }

    constexpr bool is_critical() const { return critical_; }

    // Returns a signed 32bit diagnostic value representing the effective
    // profile state. Returns either the positive weight or the negative packed
    // version of the deadline parameters. Memoizes the packed deadline
    // parameters in a reserved range of the weight to avoid unnecessary
    // overhead during context switches when parameters remain constant.
    //
    // The packed deadline parameter format renders in decimal as -CCCCDDDD,
    // where CCCC is the capacity and DDDD is the relative deadline (period),
    // with a precision of 10us. This is useful for visualization in Perfetto,
    // which only supports 32bit signed priority values.
    constexpr int32_t GetWeightOrPackedDeadlineParams() {
      if (weight_ >= kMinFairWeight || weight_ <= kMaxDiagnosticWeight) {
        return static_cast<int32_t>(weight_.raw_value());
      }

      const int32_t clamped_scaled_capacity =
          ktl::clamp(static_cast<int32_t>(capacity_ns_.raw_value() / 10'000), 1, 9'999);
      const int32_t clamped_scaled_deadline =
          ktl::clamp(static_cast<int32_t>(deadline_ns_.raw_value() / 10'000), 1, 9'999);

      const int32_t diagnostic_value =
          -((clamped_scaled_capacity * 10'000) + clamped_scaled_deadline);
      weight_ = SchedWeight::FromRaw(diagnostic_value);

      return diagnostic_value;
    }

   private:
    // Internal weight values greater than or equal to zero indicate a fair
    // profile with that weight. The internal deadline parameter values are
    // arbitrary and reading them is not allowed.
    static constexpr SchedWeight kMinFairWeight{0};

    // Weight values in the range (-10,001, -1] indicate a deadline profile that
    // has not been memoized into a packed diagnostic value. The internal values
    // of the deadline parameters are valid. Directly reading the weight is not
    // allowed.
    static constexpr SchedWeight kMaxDeadlineWeight = SchedWeight::FromRaw(-1);

    // Weight values less than or equal to -10,001 indicate a deadline profile
    // that has been memoized into a packed diagnostic value. The internal
    // values of the deadline parameters are valid. Directly reading the weight
    // is not allowed.
    static constexpr SchedWeight kMaxDiagnosticWeight = SchedWeight::FromRaw(-10'001);

    SchedWeight weight_{0};
    SchedCompactDuration capacity_ns_{0};
    SchedCompactDuration deadline_ns_{0};
    SchedUtilization utilization_{0};
    bool critical_{false};
  };

  // Values stored in the SchedulerState of Thread instances which tracks the
  // aggregate profile values inherited from upstream contributors.
  struct InheritedProfileValues {
    // Inherited from fair threads.
    SchedWeight total_weight{0};

    // Inherited from deadline threads.
    SchedUtilization uncapped_utilization{0};
    SchedDuration min_deadline{SchedDuration::Max()};
    int32_t critical_count{0};

    constexpr bool is_consequential() const {
      return total_weight != SchedWeight{0} || uncapped_utilization != SchedUtilization{0};
    }

    void AssertConsistency() const {
      DEBUG_ASSERT_MSG(min_deadline > 0, "min_deadline=%" PRId64, min_deadline.raw_value());
      DEBUG_ASSERT_MSG(total_weight >= 0, "total_weight=%s", ffl::Format(total_weight).c_str());
      DEBUG_ASSERT_MSG(uncapped_utilization >= 0, "uncapped_utilization=%s",
                       ffl::Format(uncapped_utilization).c_str());
      DEBUG_ASSERT_MSG(uncapped_utilization == 0 || min_deadline < SchedDuration::Max(),
                       "uncapped_utilization=%s min_deadline=%" PRId64,
                       ffl::Format(uncapped_utilization).c_str(), min_deadline.raw_value());
    }
  };

  struct WaitQueueInheritedSchedulerState {
   public:
    WaitQueueInheritedSchedulerState() = default;
    ~WaitQueueInheritedSchedulerState() { AssertIsReset(); }

    void Reset() { new (this) WaitQueueInheritedSchedulerState{}; }

    // If we have extra validation enabled, and this queue is no longer
    // inheriting any deadline pressure (even if there are still waiters),
    // then reset the dynamic parameters as well.
    //
    // The dynamic parameters (start time, finish time, time slice) are
    // technically undefined when we are not inheriting any utilization.  Fair
    // thread do not have defined dynamic parameters when they are blocked.
    //
    // In a production build with no extra validation checks, it should not be
    // necessary to ever touch them once they become undefined. Their values
    // will be overwritten later on if/when they do finally become defined
    // again.  In a build with extra checks enabled, however, it can be
    // beneficial to reset them to known default values when they are in the
    // "undefined" state, in order to make it easier to catch an accidental use
    // of the parameters when they have no defined meaning.
    void ResetDynamicParameters() {
      if constexpr (kSchedulerExtraInvariantValidation) {
        ASSERT(ipvs.uncapped_utilization == SchedUtilization{0});
        ASSERT(ipvs.min_deadline == SchedDuration::Max());
        start_time = SchedTime{0};
        finish_time = SchedTime{0};
        time_slice_ns = SchedDuration{0};
        time_slice_used_ns = SchedDuration{0};
      }
    }

    void AssertDynamicParametersAreReset() const {
      if constexpr (kSchedulerExtraInvariantValidation) {
        ASSERT(ipvs.uncapped_utilization == SchedUtilization{0});
        ASSERT(ipvs.min_deadline == SchedDuration::Max());
        ASSERT(start_time == SchedTime{0});
        ASSERT(finish_time == SchedTime{0});
        ASSERT(time_slice_ns == SchedDuration{0});
        ASSERT(time_slice_used_ns == SchedDuration{0});
      }
    }

    void AssertIsReset() const {
      if constexpr (kSchedulerExtraInvariantValidation) {
        ASSERT(ipvs.total_weight == SchedWeight{0});
        AssertDynamicParametersAreReset();
      }
    }

    InheritedProfileValues ipvs{};
    SchedTime start_time{0};  // TODO(johngro): Do we need this?
    SchedTime finish_time{0};
    SchedDuration time_slice_ns{0};
    SchedDuration time_slice_used_ns{0};
  };

  // Converts from kernel priority value in the interval [0, 31] to weight in
  // the interval (0.0, 1.0]. See the definition of SchedWeight for an
  // explanation of the weight distribution.
  static constexpr SchedWeight ConvertPriorityToWeight(int priority) {
    return internal::kPriorityToWeightTable[priority];
  }

  SchedulerState() {}
  explicit SchedulerState(const SchedulerState::BaseProfile& base_profile)
      : base_profile_(base_profile), effective_profile_(base_profile) {}

  SchedulerState(const SchedulerState&) = delete;
  SchedulerState& operator=(const SchedulerState&) = delete;

  // Returns the effective mask of CPUs a thread may run on, based on the
  // thread's affinity masks and CPUs currently active on the system.
  cpu_mask_t GetEffectiveCpuMask(cpu_mask_t active_mask) const {
    // The thread may run on any active CPU allowed by both its hard and
    // soft CPU affinity.
    const cpu_mask_t available_mask = active_mask & soft_affinity_ & hard_affinity_;

    // Return the mask honoring soft affinity if it is viable, otherwise ignore
    // soft affinity and honor only hard affinity.
    if (likely(available_mask != 0)) {
      return available_mask;
    }

    return active_mask & hard_affinity_;
  }

  // Returns the current effective profile for this thread.
  const EffectiveProfile& effective_profile() const { return effective_profile_; }

  // Returns the type of scheduling discipline for this thread.
  SchedDiscipline discipline() const { return effective_profile_.discipline(); }

  // Returns the key used to order the run queue.
  KeyType key() const { return {start_time_, reinterpret_cast<uintptr_t>(this)}; }

  uint64_t flow_id() const { return flow_id_; }

  zx_instant_mono_t last_started_running() const { return last_started_running_.raw_value(); }
  zx_duration_mono_t runtime_ns() const { return runtime_ns_.raw_value(); }

  SchedDuration expected_runtime_ns() const { return expected_runtime_ns_; }
  SchedDuration time_slice_ns() const { return time_slice_ns_; }
  SchedDuration time_slice_used_ns() const { return time_slice_used_ns_; }
  SchedDuration remaining_time_slice_ns() const { return time_slice_ns_ - time_slice_used_ns_; }

  SchedTime start_time() const { return start_time_; }
  SchedTime finish_time() const { return finish_time_; }
  SchedDuration effective_period() const { return finish_time() - start_time(); }
  uint64_t activation_count() const { return activation_count_; }

  cpu_mask_t hard_affinity() const { return hard_affinity_; }
  cpu_mask_t soft_affinity() const { return soft_affinity_; }

  // This value requires the thread's lock to be held, since it may need to
  // memoize the packed deadline parameters.
  int32_t GetWeightOrPackedDeadlineParams() {
    return effective_profile_.GetWeightOrPackedDeadlineParams();
  }

  cpu_num_t curr_cpu() const { return curr_cpu_; }
  cpu_num_t last_cpu() const { return last_cpu_; }

  thread_state state() const { return state_; }
  void set_state(thread_state state) { state_ = state; }

 private:
  friend class Scheduler;
  friend class OwnedWaitQueue;
  friend class WaitQueue;
  friend class WaitQueueBase;
  friend class WaitQueueCollection;

  // Allow tests to observe/modify our state.
  friend class LoadBalancerTest;
  friend struct WaitQueueOrderingTests;
  friend class unittest::ThreadEffectiveProfileObserver;
  friend Thread;

  // TODO(eieio): Remove these once all of the members accessed by Thread are
  // moved to accessors.
  friend void thread_construct_first(Thread*, const char*);
  friend void dump_thread_locked(const Thread*, bool);

  // RecomputeEffectiveProfile should only ever be called from the accessor in
  // Thread (where we can use static analysis to ensure that we are holding the
  // thread's lock, as required).
  void RecomputeEffectiveProfile();

  // The start time of the thread's current bandwidth request. This is the
  // virtual start time for fair tasks and the period start for deadline tasks.
  SchedTime start_time_{0};

  // The finish time of the thread's current bandwidth request. This is the
  // virtual finish time for fair tasks and the absolute deadline for deadline
  // tasks.
  SchedTime finish_time_{0};

  // Counts how many times this thread has been activated since it was created.
  uint64_t activation_count_{0};

  struct SubtreeInvariants {
    // Minimum finish time of all the descendants of this node in the run queue.
    // Used to perform a partition search in O(log n) time, to find the thread
    // with the earliest finish time that also has an eligible start time.
    SchedTime min_finish_time{0};

    // Minimum weight of all the descendants of this node in the run queue. Used
    // to determine when period expansion is required to ensure that time slices
    // are not too small.
    SchedWeight min_weight{0};
  };

  // Subtree invariants for the augmented binary search tree implemented by the
  // run queue WAVLTrees. The WAVLTree observer hooks maintain these per-node
  // values that describe properties of the node's subtree when the node is in
  // the tree.
  SubtreeInvariants subtree_invariants_;

  // The scheduling state of the thread.
  thread_state state_{THREAD_INITIAL};

  BaseProfile base_profile_;
  InheritedProfileValues inherited_profile_values_;
  EffectiveProfile effective_profile_;

  // The current timeslice allocated to the thread.
  SchedDuration time_slice_ns_{0};

  // The runtime used in the current period.
  SchedDuration time_slice_used_ns_{0};

  // The total time in THREAD_RUNNING state. If the thread is currently in
  // THREAD_RUNNING state, this excludes the time accrued since it last left the
  // scheduler.
  SchedDuration runtime_ns_{0};

  // Tracks the exponential moving average of the runtime of the thread.
  SchedDuration expected_runtime_ns_{0};

  // Tracks runtime accumulated until voluntarily blocking or exhausting the
  // allocated time slice. Used to exclude involuntary preemption when updating
  // the expected runtime estimate to improve accuracy.
  SchedDuration banked_runtime_ns_{0};

  // Tracks the accumulated energy consumption of the thread, as estimated by
  // the processor energy model. This counter can accumulate ~580 watt years
  // (e.g. 1W continuously for ~580 years, 10W continuously for ~58 years, ...)
  // before overflowing.
  uint64_t estimated_energy_consumption_nj{0};

  // The time the thread last ran. The exact point in time this value represents
  // depends on the thread state:
  //   * THREAD_RUNNING: The time of the last reschedule that selected the thread.
  //   * THREAD_READY: The time the thread entered the run queue.
  //   * Otherwise: The time the thread last ran.
  SchedTime last_started_running_{0};

  // The current sched_latency flow id for this thread.
  uint64_t flow_id_{0};

  // The current CPU the thread is READY or RUNNING on, INVALID_CPU otherwise.
  cpu_num_t curr_cpu_{INVALID_CPU};

  // The last CPU the thread ran on. INVALID_CPU before it first runs.
  cpu_num_t last_cpu_{INVALID_CPU};

  // The set of CPUs the thread is permitted to run on. The thread is never
  // assigned to CPUs outside of this set.
  cpu_mask_t hard_affinity_{CPU_MASK_ALL};

  // The set of CPUs the thread should run on if possible. The thread may be
  // assigned to CPUs outside of this set if necessary.
  cpu_mask_t soft_affinity_{CPU_MASK_ALL};
};

// SchedulerQueueState tracks the association with a scheduler and run queue. To
// accommodate the different ways a thread can be accessed (i.e. starting from a
// thread vs. starting from a run queue), specific locking rules apply to this
// member of Thread (i.e. Thread::scheduler_queue_state_).
//
// There are two locks that apply to interactions with the queue state:
//
//  1. Thread::lock_ (or the thread's lock): Protects most of a thread's data
//     members, including the scheduling state (i.e. Thread::scheduling_state_).
//     Thread::scheduling_state_::curr_cpu_ indicates which CPU and scheduler
//     the thread is currently associated with.
//
//  2. Scheduler::queue_lock_ (or the queue lock): Protects a scheduler's data
//     members, including the run queue and associated bookkeeping. This lock
//     also protects the thread's scheduler queue state member while the thread
//     is associated with a particular scheduler.
//
//  When both a thread's lock and a scheduler's queue lock must be held at
//  the same time, commonly needed during rescheduling and PI operations,
//  the defined lock order is to acquire the thread's lock before the queue
//  lock. Fortunately, most operations start from a thread and proceed to
//  interact with a scheduler (e.g. blocking, unblocking, PI operations),
//  naturally conforming to the required lock order. However, several
//  operations start from a scheduler (e.g. finding the next thread to run,
//  stealing a thread from another CPU), and require some lock juggling to
//  complete their operations.
//
//  Operations that start from a scheduler typically dequeue a thread from a
//  run queue while holding the queue lock protecting that run queue.
//  Locking the thread to complete the operation involves releasing the
//  currently held queue lock, acquiring the thread's lock, and then
//  acquiring either the same queue lock or a different queue lock.
//
//  For example, selecting the next thread to run during a reschedule
//  involves the following lock juggling sequence:
//  1. With the local queue lock held, dequeue the next thread to run.
//  2. Release the queue lock.
//  3. Acquire the next thread's lock.
//  4. Re-acquire the local queue lock and continue the reschedule.
//
//  Stealing a thread from another CPU involves a similar lock juggling
//  sequence:
//  1. With the source queue lock held, dequeue the thread to steal and
//     (mostly) disassociate it with the source scheduler.
//  2. Release the source queue lock.
//  3. Acquire the stolen thread's lock.
//  4. Acquire the local queue lock, associate the thread with the local
//     scheduler, and continue the reschedule.
//
//  In both cases, since the thread is no longer in a queue after step 1,
//  it cannot be selected or stolen by another CPU after the queue lock is
//  released. However, because the thread is not yet locked between steps 2 and
//  3, an operation starting at the thread and holding the thread's lock could
//  interleave with the locking sequence. If such an interleaved operation
//  involves updating the thread's run queue position and/or the associated
//  scheduler's bookkeeping, care must be taken to avoid double-dequeuing the
//  thread, updating the wrong scheduler's bookkeeping, or rescheduling the
//  wrong CPU.
//
// SchedulerQueueState::Disposition is an enumeration of the valid combinations
// of a thread's scheduler queue states that may be used in conjunction with the
// thread state (i.e. Thread::state()) to determine how the thread and the
// associated scheduler can be updated.
//
// The following state combinations may be observed by operations that start at
// a thread and hold both the thread's lock and the queue lock:
//
// 1. state=RUNNING, disposition=Associated:
//
//    Observable when a thread is running. Because the currently running
//    thread's lock is held over a reschedule, there are no observable
//    transitional states to deal with in PI and affinity change operations.
//
//    Holding the thread lock delays rescheduling and transitioning from RUNNING
//    to any other state. The thread's scheduling state and scheduler
//    bookkeeping may be updated by a PI operation or affinity change. The CPU
//    performing the update is obligated to reschedule the CPU the thread is
//    running on when the update is complete.
//
// 2. state=READY, disposition=Enqueued:
//
//    Observable when a thread is waiting in the run queue and is not in the
//    process of rescheduling.
//
//    Holding the queue lock delays the thread from transitioning to RUNNING,
//    being stolen, or migrated by another CPU. The thread's scheduling state,
//    scheduler bookkeeping, and scheduler queue state may be updated by a PI
//    operation or affinity change. The CPU performing the update is obligated
//    to reschedule the CPU the thread is associated with if the change might
//    affect the currently running thread.
//
// 3. state=READY, disposition=Associated:
//
//    Observable when a thread is rescheduling and transitioning from READY to
//    RUNNING and the lock juggling of the rescheduling CPU races with an
//    update.
//
//    During the reschedule, when the next thread has been dequeued it is
//    expected to be the next thread to run on the CPU. To complete the
//    transition, the scheduler releases the queue lock (allowing this
//    observation from another CPU), acquires the thread lock, and re-acquires
//    the queue lock.
//
//    Holding the thread lock on another CPU delays the completion of the
//    transition, allowing the effective profile, dynamic parameters, and
//    scheduler bookkeeping or affinity to be updated.
//
//    Note: An update of the thread's effective profile and dynamic parameters
//    can invalidate the thread as the correct/best choice to run. The CPU
//    performing the update is obligated to reschedule the CPU the thread is
//    running on, either unconditionally (current behavior) or conditionally if
//    another thread should run instead (future optimization).
//
//    Likewise, a change to the thread's affinity mask may invalidate the CPU
//    the thread is about to run on as a viable option. The CPU performing the
//    update is obligated to reschedule the CPU to cause it to migrate the
//    thread to viable target.
//
// 4. state=READY, disposition=Stolen:
//
//    Observable when a thread is being stolen and the lock juggling of
//    the stealing CPU races with an update from another CPU.
//
//    When the thread is being stolen, it is dequeued and removed from the
//    previous scheduler's bookkeeping. To complete the steal, the source
//    queue lock is released (allowing this observation from another CPU),
//    the thread lock is acquired, and the destination queue lock is
//    acquired. Since the stolen thread will become the currently running
//    thread on the stealing CPU, rescheduling is unnecessary and can be
//    omitted in a future optimization.
//
//    Holding the thread lock on another CPU delays the steal, allowing the
//    effective profile and dynamic parameters or affinity mask to be updated.
//    In this state, the thread is not associated with any scheduler
//    bookkeeping, but the current CPU of thread is stale. However, holding the
//    queue lock of the stale CPU allows the CPU performing the update to make a
//    coherent observation of the stolen_by member, which can be used to
//    determine if an affinity mask change has invalidated the stealing CPU as a
//    viable option. The updating CPU is obligated to reschedule the stealing
//    CPU if it became an invalid option due to the update.
//
// 5. state=INITIAL,BLOCKED*,SLEEPING,SUSPENDED,DEATH,
//    disposition=Unassociated
//
//    Observable only by the CPU performing a reschedule that transitions the
//    current thread from RUNNING to one of the non-runnable states. Since a
//    reschedule of the current thread occurs with the thread lock held, and
//    transitioning to a non-runnable state clears the current CPU for the
//    thread, no other CPU can observe this state while holding the thread's
//    lock and a queue lock.
//
class SchedulerQueueState {
 public:
  SchedulerQueueState() = default;
  ~SchedulerQueueState() = default;

  SchedulerQueueState(const SchedulerQueueState&) = delete;
  SchedulerQueueState& operator=(const SchedulerQueueState&) = delete;
  SchedulerQueueState(SchedulerQueueState&&) = delete;
  SchedulerQueueState& operator=(SchedulerQueueState&&) = delete;

  // The disposition is a concise representation of the valid combinations of
  // states of the members of this class. It is used in conjunction with the
  // thread state to determine which operations are valid on a thread and its
  // associated scheduler, if any.
  enum class Disposition : uint8_t {
    // Corresponds to active=false, in_queue=false, stolen_by=INVALID_CPU.
    Unassociated,

    // Corresponds to active=true, in_queue=false, stolen_by=INVALID_CPU.
    Associated,

    // Corresponds to active=true, in_queue=true, stolen_by=INVALID_CPU.
    Enqueued,

    // Corresponds to active=false, in_queue=false, stolen_by=<CPU num>.
    Stolen,
  };

  // Sets the thread state to active (i.e. associated with a specific CPU's
  // scheduler and bookkeeping).
  //
  // Returns true if the thread was not previously active.
  bool OnInsert() {
    const bool was_active = active_;
    active_ = true;
    stolen_by_ = INVALID_CPU;
    return !was_active;
  }

  // Sets the thread state to inactive (i.e. not associated with a CPU's
  // scheduler or bookkeeping). If the thread is being stolen from another CPU's
  // run queue, stolen_by must be the CPU id of the stealing CPU, otherwise it
  // must be INVALID_CPU.
  //
  // Returns true if the task was previously active.
  bool OnRemove(cpu_num_t stolen_by) {
    const bool was_active = active_;
    active_ = false;
    stolen_by_ = stolen_by;
    return was_active;
  }

  // Returns the run queue node.
  fbl::WAVLTreeNodeState<Thread*>& run_queue_node() { return run_queue_node_; }
  const fbl::WAVLTreeNodeState<Thread*>& run_queue_node() const { return run_queue_node_; }

  // Returns true if the thread is currently enqueued in a run queue.
  bool in_queue() const { return run_queue_node_.InContainer(); }

  // Returns the CPU id of the CPU currently stealing the thread.
  cpu_num_t stolen_by() const { return stolen_by_; }

  // Returns true if the thread is currently associated with a scheduler.
  bool active() const { return active_; }

  // Returns the disposition of this scheduler queue state. Asserts on invalid
  // combinations.
  Disposition disposition() const {
    if (active_) {
      DEBUG_ASSERT(stolen_by_ == INVALID_CPU);
      if (in_queue()) {
        return Disposition::Enqueued;
      }
      return Disposition::Associated;
    }
    DEBUG_ASSERT(!in_queue());
    if (stolen_by_ == INVALID_CPU) {
      return Disposition::Unassociated;
    }
    return Disposition::Stolen;
  }

 private:
  // The WAVLTree node for enqueuing the thread in a run queue.
  fbl::WAVLTreeNodeState<Thread*> run_queue_node_;

  // The id of the CPU currently in the process of stealing the thread, or
  // INVALID_CPU if the thread is not being stolen.
  cpu_num_t stolen_by_{INVALID_CPU};

  // Flag indicating whether this thread is associated with a specific CPU's
  // scheduler and bookkeeping.
  bool active_{false};
};

FBL_ENABLE_ENUM_BITS(SchedulerState::ProfileDirtyFlag)

template <>
inline void SchedulerState::EffectiveProfileDirtyTracker<true>::MarkBaseProfileChanged() {
  dirty_flags_ |= ProfileDirtyFlag::BaseDirty;
}

template <>
inline void SchedulerState::EffectiveProfileDirtyTracker<true>::MarkInheritedProfileChanged() {
  dirty_flags_ |= ProfileDirtyFlag::InheritedDirty;
}

#endif  // ZIRCON_KERNEL_INCLUDE_KERNEL_SCHEDULER_STATE_H_
