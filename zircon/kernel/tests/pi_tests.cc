// Copyright 2019 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/fit/defer.h>
#include <lib/fit/function.h>
#include <lib/kconcurrent/chainlock_transaction.h>
#include <lib/unittest/unittest.h>
#include <lib/zircon-internal/macros.h>
#include <lib/zx/time.h>
#include <platform.h>
#include <zircon/types.h>

#include <fbl/alloc_checker.h>
#include <fbl/macros.h>
#include <fbl/ref_counted.h>
#include <fbl/ref_ptr.h>
#include <kernel/auto_preempt_disabler.h>
#include <kernel/event.h>
#include <kernel/owned_wait_queue.h>
#include <kernel/scheduler.h>
#include <kernel/thread.h>
#include <kernel/wait.h>
#include <ktl/algorithm.h>
#include <ktl/array.h>
#include <ktl/atomic.h>
#include <ktl/concepts.h>
#include <ktl/iterator.h>
#include <ktl/limits.h>
#include <ktl/new.h>
#include <ktl/type_traits.h>
#include <ktl/unique_ptr.h>

#include "tests.h"

#include <ktl/enforce.h>

namespace {

constexpr SchedWeight TEST_LOWEST_WEIGHT =
    SchedulerState::ConvertPriorityToWeight(LOWEST_PRIORITY + 1);
constexpr SchedWeight TEST_HIGHEST_WEIGHT =
    SchedulerState::ConvertPriorityToWeight(HIGHEST_PRIORITY);
constexpr SchedWeight TEST_DEFAULT_WEIGHT =
    SchedulerState::ConvertPriorityToWeight(DEFAULT_PRIORITY);
constexpr SchedWeight TEST_EPSILON_WEIGHT{ffl::FromRatio<int64_t>(1, SchedWeight::Format::Power)};

class TestThread;  // fwd decl

enum class InheritableProfile { No, Yes };

// An RAII style helper which automatically assigns a deadline profile (with a
// short deadline and high utilization)to a thread, restoring the base profile
// automatically when the test ends.  Many of these tests need to rely on timing
// in order to control the order with which threads time out of various wait
// queues.  Since we don't have deterministic control over timing in our tests,
// we rely on our high priority test thread being scheduled and pre-empting all
// other threads when it's timer goes off in order to reduce the chances of
// timing related flake in the tests.
class AutoProfileBooster {
 public:
  AutoProfileBooster() : initial_base_profile_(Thread::Current::Get()->SnapshotBaseProfile()) {
    constexpr SchedUtilization utilization = ffl::FromRatio(90, 100);
    constexpr SchedDuration deadline{ZX_USEC(200)};
    constexpr SchedDuration capacity = utilization * deadline;
    const SchedulerState::BaseProfile new_base_profile{SchedDeadlineParams{capacity, deadline}};
    Thread::Current::Get()->SetBaseProfile(new_base_profile);
  }

  ~AutoProfileBooster() { Thread::Current::Get()->SetBaseProfile(initial_base_profile_); }

  DISALLOW_COPY_ASSIGN_AND_MOVE(AutoProfileBooster);

 private:
  const SchedulerState::BaseProfile initial_base_profile_;
};

// A small helper which creates different permutations of an input array based
// on a distribution method, and optional random seed. Used for things like
// determining which profiles will be assigned to which test threads, or which
// order threads will be released from the blocked state during  various tests.
class DistroSpec {
 public:
  enum class Type { ASCENDING, DESCENDING, RANDOM, SHUFFLE };
  constexpr DistroSpec(Type t, uint64_t s = 0) : type_(t), seed_(s) {}

  template <typename MemberType, size_t N>
  void Apply(const ktl::array<MemberType, N>& in, ktl::array<MemberType, N>& out) const {
    uint64_t prng = seed_;
    switch (type_) {
      case DistroSpec::Type::ASCENDING:
        for (size_t i = 0; i < N; ++i) {
          out[i] = in[i];
        }
        break;

      case DistroSpec::Type::DESCENDING:
        for (size_t i = 0; i < N; ++i) {
          out[i] = in[in.size() - i - 1];
        }
        break;

      case DistroSpec::Type::RANDOM:
        for (auto& item : out) {
          item = in[rand_r(&prng) % N];
        }
        break;

      // Create a range of values from [0, N) + offset, but shuffle the order of
      // those values in the set.
      case DistroSpec::Type::SHUFFLE:
        // Start by filling our shuffle order array with a illegal sentinel
        // value (N will do the job just fine), then foreach i in the range [0,
        // N) pick a random position in the output to put i, and linearly probe
        // until we find the first unused position in order to shuffle.
        ktl::array<size_t, N> order;
        for (size_t i = 0; i < N; ++i) {
          order[i] = N;
        }

        for (size_t i = 0; i < N; ++i) {
          size_t pos = (rand_r(&prng) % N);
          while (order[pos] != N) {
            pos = (pos + 1) % N;
          }
          order[pos] = i;
        }

        // Finally, produce our output from our input, permuting using the
        // shuffle order.
        for (size_t i = 0; i < N; ++i) {
          out[i] = in[order[i]];
        }
        break;
    }
  }

 private:
  const Type type_;
  const uint64_t seed_;
};

struct ExpectedEffectiveProfile {
  struct {
    SchedDiscipline discipline{SchedDiscipline::Fair};
    SchedWeight fair_weight{0};
    SchedDeadlineParams deadline;
  } base;

  SchedulerState::InheritedProfileValues ipvs;
};
}  // namespace

namespace unittest {
class ThreadEffectiveProfileObserver {
 public:
  void Observe(const Thread& t) { observed_profile_ = t.SnapshotEffectiveProfile(); }

  bool VerifyExpectedEffectiveProfile(const ExpectedEffectiveProfile& eep) {
    BEGIN_TEST;

    bool expected_fair = (eep.base.discipline == SchedDiscipline::Fair) &&
                         (eep.ipvs.uncapped_utilization == SchedUtilization{0});
    ASSERT_EQ(expected_fair, observed_profile_.IsFair());

    if (observed_profile_.IsFair()) {
      SchedWeight expected = eep.base.fair_weight + eep.ipvs.total_weight;
      EXPECT_EQ(expected.raw_value(), observed_profile_.weight().raw_value());
    } else {
      SchedUtilization effective_utilization = eep.ipvs.uncapped_utilization;
      SchedDuration effective_deadline = eep.ipvs.min_deadline;
      bool effective_critical = eep.ipvs.critical_count > 0;

      if (eep.base.discipline == SchedDiscipline::Deadline) {
        effective_utilization += eep.base.deadline.utilization;
        effective_deadline = ktl::min(effective_deadline, eep.base.deadline.deadline_ns);
      }
      effective_utilization = ktl::min(effective_utilization, SchedUtilization{1});

      SchedDuration effective_capacity = effective_deadline * effective_utilization;
      ASSERT_LE(effective_capacity.raw_value(), effective_deadline.raw_value());

      // Rounding can cause differences between expressions that derive capacity
      // from utilization and deadline and expressions that derive utilization
      // from capacity and deadline. For this reason, use the constructor that
      // takes all three values to ensure consistent test comparisons. The two
      // parameter constructor that derives utilization from the given capacity
      // and deadline may produce a slightly different utilization value than the
      // effective utilization value here.
      SchedDeadlineParams expected{effective_capacity, effective_deadline, effective_utilization};

      EXPECT_EQ(expected.capacity_ns.raw_value(),
                observed_profile_.deadline().capacity_ns.raw_value());
      EXPECT_EQ(expected.deadline_ns.raw_value(),
                observed_profile_.deadline().deadline_ns.raw_value());
      EXPECT_EQ(expected.utilization.raw_value(),
                observed_profile_.deadline().utilization.raw_value());
      EXPECT_EQ(effective_critical, observed_profile_.is_critical());
    }

    END_TEST;
  }

 private:
  SchedulerState::EffectiveProfile observed_profile_;
};
}  // namespace unittest

namespace {

class Profile : public fbl::RefCounted<Profile> {
 public:
  virtual ~Profile() = default;
  virtual void Apply(Thread& thread) = 0;
  virtual void SetExpectedBaseProfile(ExpectedEffectiveProfile& eep) = 0;
  virtual void AccumulateExpectedPressure(ExpectedEffectiveProfile& eep) = 0;
  virtual size_t DebugPrint(char* buf, size_t space) = 0;
  virtual zx_duration_mono_t Period() const = 0;

 protected:
  Profile() = default;
};

class FairProfile : public Profile {
 public:
  static fbl::RefPtr<Profile> Create(SchedWeight weight, InheritableProfile inheritable) {
    fbl::AllocChecker ac;
    FairProfile* profile = new (&ac) FairProfile(weight, inheritable);
    if (ac.check()) {
      return fbl::AdoptRef(profile);
    }
    return nullptr;
  }

  void Apply(Thread& thread) override {
    thread.SetBaseProfile(
        SchedulerState::BaseProfile{weight_, (inheritable_ == InheritableProfile::Yes)});
  }

  void SetExpectedBaseProfile(ExpectedEffectiveProfile& eep) override {
    eep.base.discipline = SchedDiscipline::Fair;
    eep.base.fair_weight = weight_;
    eep.ipvs = SchedulerState::InheritedProfileValues{};
  }

  void AccumulateExpectedPressure(ExpectedEffectiveProfile& eep) override {
    if (inheritable_ == InheritableProfile::Yes) {
      eep.ipvs.total_weight += weight_;
    }
  }

  size_t DebugPrint(char* buf, size_t space) override {
    return snprintf(buf, space, "[weight %ld]", weight_.raw_value());
  }

  zx_duration_mono_t Period() const override { return zx_duration_mono_t{0}; }

 private:
  FairProfile(SchedWeight weight, InheritableProfile inheritable)
      : weight_(weight), inheritable_(inheritable) {
    ASSERT(static_cast<uint64_t>(weight_.raw_value()) != 0xFFFFFFFFFFFF0000);
  }

  const SchedWeight weight_;
  const InheritableProfile inheritable_;
};

class DeadlineProfile : public Profile {
 public:
  static fbl::RefPtr<Profile> Create(zx_duration_mono_t capacity, zx_duration_mono_t deadline,
                                     bool critical = false) {
    fbl::AllocChecker ac;
    DeadlineProfile* profile =
        new (&ac) DeadlineProfile(SchedDuration{capacity}, SchedDuration{deadline}, critical);
    if (ac.check()) {
      return fbl::AdoptRef(profile);
    }
    return nullptr;
  }

  void Apply(Thread& thread) override {
    thread.SetBaseProfile(SchedulerState::BaseProfile{sched_params_, critical_});
  }

  void SetExpectedBaseProfile(ExpectedEffectiveProfile& eep) override {
    eep.base.discipline = SchedDiscipline::Deadline;
    eep.base.deadline = sched_params_;
    eep.ipvs = SchedulerState::InheritedProfileValues{};
  }

  void AccumulateExpectedPressure(ExpectedEffectiveProfile& eep) override {
    eep.ipvs.uncapped_utilization += sched_params_.utilization;
    eep.ipvs.min_deadline = ktl::min(eep.ipvs.min_deadline, sched_params_.deadline_ns);
    if (critical_) {
      eep.ipvs.critical_count++;
    }
  }

  size_t DebugPrint(char* buf, size_t space) override {
    return snprintf(buf, space, "[capacity %ld deadline %ld utilization %ld critical %d]",
                    sched_params_.capacity_ns.raw_value(), sched_params_.deadline_ns.raw_value(),
                    sched_params_.utilization.raw_value(), critical_);
  }

  zx_duration_mono_t Period() const override { return sched_params_.deadline_ns.raw_value(); }

  const SchedDeadlineParams& sched_params() const { return sched_params_; }

 private:
  DeadlineProfile(SchedDuration capacity, SchedDuration deadline, bool critical)
      : sched_params_(capacity, deadline), critical_(critical) {
    DEBUG_ASSERT(capacity <= deadline);
  }

  const SchedDeadlineParams sched_params_;
  const bool critical_;
};

// Helper wrapper for an owned wait queue which manages grabbing and releasing
// the locks or disabling preemption at appropriate times for us.  Mostly, this
// is just about saving some typing.
class LockedOwnedWaitQueue : public OwnedWaitQueue {
 public:
  constexpr LockedOwnedWaitQueue() = default;
  DISALLOW_COPY_ASSIGN_AND_MOVE(LockedOwnedWaitQueue);

  void ReleaseAllThreads() {
    AnnotatedAutoEagerReschedDisabler eager_resched_disabler;
    OwnedWaitQueue::WakeThreads(ktl::numeric_limits<uint32_t>::max());
  }

  void ReleaseOneThread() {
    AnnotatedAutoEagerReschedDisabler eager_resched_disabler;
    OwnedWaitQueue::WakeThreadAndAssignOwner();
  }

  bool HasInheritedSchedulerStateStorage() const {
    SingleChainLockGuard guard{
        IrqSaveOption, get_lock(),
        CLT_TAG("LockedOwnedWaitQueue::HasInheritedSchedulerStateStorage (pi_tests)")};
    return inherited_scheduler_state_storage() != nullptr;
  }
};

// LoopIterPrinter
// A small RAII style class which helps us to print out where a loop iterator
// is when a test fails and bails out.  Note: loop iterator types must be
// convertible to int64_t.
template <typename T>
class LoopIterPrinter {
 public:
  constexpr LoopIterPrinter(const char* field_name, T iter_val)
      : field_name_(field_name), iter_val_(iter_val) {}

  ~LoopIterPrinter() {
    if (field_name_ == nullptr) {
      return;
    }

    char buffer[256];
    size_t offset = 0;

    offset += snprintf(buffer + offset, ktl::size(buffer) - offset,
                       "Test failed with %s == ", field_name_);

    if constexpr (ktl::is_same_v<T, fbl::RefPtr<Profile>>) {
      offset += iter_val_->DebugPrint(buffer + offset, ktl::size(buffer) - offset);
    } else {
      offset += snprintf(buffer + offset, ktl::size(buffer) - offset, "%ld",
                         static_cast<int64_t>(iter_val_));
    }

    printf("%s\n", buffer);
  }

  DISALLOW_COPY_ASSIGN_AND_MOVE(LoopIterPrinter);

  void cancel() { field_name_ = nullptr; }

 private:
  const char* field_name_;
  T iter_val_;
};

#define PRINT_LOOP_ITER(_var_name) LoopIterPrinter print_##_var_name(#_var_name, _var_name)

// The core test thread object.  We use this object to build various graphs of
// priority inheritance chains, and then evaluate that the effective priorities
// of the threads involved in the graph are what we expect them to be after
// various mutations of the graph have taken place.
class TestThread {
 public:
  enum class State : uint32_t {
    INITIAL,
    CREATED,
    WAITING_TO_START,
    STARTED,
    WAITING_FOR_SHUTDOWN,
    SHUTDOWN,
  };

  enum class Condition : uint32_t {
    BLOCKED,
    STARTED,
    WAITING_FOR_SHUTDOWN,
  };

  // Test threads in the various tests use lambdas in order to store their
  // customized test operations.  In order to allow these lambda's to capture
  // context from their local scope, but not need to use the heap in order to
  // allocate the storage for the scope, we need to know the worst case
  // capture storage requirements across all of these tests.  Armed with this
  // knowledge, we can use a fit::inline_function to pre-allocate storage in
  // the TestThread object for the worst case lambda we will encounter in the
  // test suite.
  //
  // If this grows in the future, this constexpr bound should be updated to
  // match the new worst case storage requirement.
  static constexpr size_t kMaxOpLambdaCaptureStorageBytes = sizeof(void*) * 8;

  TestThread() = default;
  ~TestThread() { Reset(); }

  DISALLOW_COPY_ASSIGN_AND_MOVE(TestThread);

  // Reset the barrier at the start of a test in order to prevent threads from
  // exiting after they have completed their operation..
  static void ResetShutdownBarrier() { allow_shutdown_.Unsignal(); }

  // Clear the barrier and allow shutdown.
  static void ClearShutdownBarrier() { allow_shutdown_.Signal(); }

  static Event& allow_shutdown() { return allow_shutdown_; }

  // Create a thread, settings its entry point and initial profile in
  // the process, but do not start it yet.
  bool Create(fbl::RefPtr<Profile> initial_profile);

  // Start the thread, have it do nothing but wait to be allowed to exit.
  bool DoStall();

  // Start the thread and have it block on an standard wait queue.
  bool BlockOnWaitQueue(WaitQueue* wq, zx::duration relative_timeout = zx::duration::infinite())
      TA_EXCL(chainlock_transaction_token);

  // Start the thread and have it block on an owned wait queue, declaring the
  // specified test thread to be the owner of that queue in the process.
  bool BlockOnOwnedWaitQueue(OwnedWaitQueue* owned_wq, TestThread* owner,
                             zx::duration relative_timeout = zx::duration::infinite());

  // Start the thread and run a custom lambda.
  bool RunOp(fit::inline_function<void(void), kMaxOpLambdaCaptureStorageBytes> op);

  // Reset the thread back to its initial state.  If |explicit_kill| is true,
  // then do not wait for the thread to exit normally if it has been started.
  // Simply send it the kill signal.
  bool Reset(bool explicit_kill = false);

  State state() const { return state_.load(); }
  Profile* initial_profile() const { return initial_profile_.get(); }
  Thread& thread() const {
    DEBUG_ASSERT(thread_ != nullptr);
    return *thread_;
  }

  thread_state tstate() const {
    if (thread_ == nullptr) {
      return thread_state::THREAD_DEATH;
    }

    SingleChainLockGuard guard{IrqSaveOption, thread().get_lock(),
                               CLT_TAG("TestThread::tstate (pi_tests)")};
    return thread().state();
  }

  SchedDuration remaining_time_slice() const {
    SingleChainLockGuard guard{IrqSaveOption, thread().get_lock(),
                               CLT_TAG("TestThread::remaining_time_slice (pi_tests)")};
    const SchedDuration time_elapsed =
        thread().scheduler_state().state() == THREAD_RUNNING
            ? SchedTime{current_mono_time()} - thread().scheduler_state().last_started_running()
            : SchedDuration{0};
    return thread().scheduler_state().remaining_time_slice_ns() - time_elapsed;
  }

  uint64_t activation_count() const {
    SingleChainLockGuard guard{IrqSaveOption, thread().get_lock(),
                               CLT_TAG("TestThread::activation_count (pi_tests)")};
    return thread().scheduler_state().activation_count();
  }

  struct StateSnapshot {
    SchedTime current_time;
    SchedDuration remaining_time_slice_ns;
    SchedTime start_time;
    SchedTime finish_time;
    SchedDuration time_slice_ns;
    uint64_t activation_count;
  };

  StateSnapshot snapshot() const {
    SingleChainLockGuard guard{IrqSaveOption, thread().get_lock(),
                               CLT_TAG("TestThread::snapshot (pi_tests)")};
    const SchedTime current_time{current_mono_time()};
    const bool running = thread().scheduler_state().state() == THREAD_RUNNING;
    const SchedDuration time_elapsed =
        running ? current_time - thread().scheduler_state().last_started_running()
                : SchedDuration{0};
    return {
        current_time,
        thread().scheduler_state().remaining_time_slice_ns() - time_elapsed,
        thread().scheduler_state().start_time(),
        thread().scheduler_state().finish_time(),
        thread().scheduler_state().time_slice_ns(),
        thread().scheduler_state().activation_count(),
    };
  }

  template <Condition condition>
  bool WaitFor();

 private:
  friend class LockedOwnedWaitQueue;

  int ThreadEntry();

  static inline Event allow_shutdown_{};

  Thread* thread_ = nullptr;
  ktl::atomic<State> state_{State::INITIAL};
  fit::inline_function<void(void), kMaxOpLambdaCaptureStorageBytes> op_;
  fbl::RefPtr<Profile> initial_profile_;
};

bool TestThread::Create(fbl::RefPtr<Profile> initial_profile) {
  BEGIN_TEST;

  ASSERT_NULL(thread_);
  ASSERT_NULL(initial_profile_);
  ASSERT_EQ(state(), State::INITIAL);

  initial_profile_ = ktl::move(initial_profile);
  thread_ = Thread::Create(
      "pi_test_thread",
      [](void* ctx) -> int { return static_cast<TestThread*>(ctx)->ThreadEntry(); },
      static_cast<void*>(this), DEFAULT_PRIORITY);

  ASSERT_NONNULL(thread_);
  initial_profile_->Apply(*thread_);

  state_.store(State::CREATED);

  END_TEST;
}

bool TestThread::DoStall() {
  BEGIN_TEST;
  ASSERT_EQ(state(), State::CREATED);
  ASSERT_FALSE(static_cast<bool>(op_));

  op_ = []() {};

  state_.store(State::WAITING_TO_START);
  thread_->Resume();

  ASSERT_TRUE(WaitFor<Condition::BLOCKED>());

  END_TEST;
}

bool TestThread::BlockOnWaitQueue(WaitQueue* wq, zx::duration relative_timeout) {
  BEGIN_TEST;
  ASSERT_EQ(state(), State::CREATED);
  ASSERT_FALSE(static_cast<bool>(op_));

  op_ = [wq, relative_timeout]() TA_EXCL(chainlock_transaction_token) {
    Deadline timeout = (relative_timeout == zx::duration::infinite())
                           ? Deadline::infinite()
                           : Deadline::after_mono(relative_timeout.get());

    const auto do_transaction = [&]() TA_REQ(
                                    chainlock_transaction_token,
                                    preempt_disabled_token) -> ChainLockTransaction::Result<> {
      Thread* const current_thread = Thread::Current::Get();
      if (!AcquireBothOrBackoff(current_thread->get_lock(), wq->get_lock())) {
        return ChainLockTransaction::Action::Backoff;
      }

      ChainLockTransaction::Finalize();
      wq->Block(current_thread, timeout, Interruptible::Yes);
      current_thread->get_lock().Release();
      return ChainLockTransaction::Done;
    };
    ChainLockTransaction::UntilDone(PreemptDisableAndIrqSaveOption,
                                    CLT_TAG("TestThread::BlockOnWaitQueue (pi_tests)"),
                                    do_transaction);
  };

  state_.store(State::WAITING_TO_START);
  thread_->Resume();

  ASSERT_TRUE(WaitFor<Condition::BLOCKED>());

  END_TEST;
}

bool TestThread::BlockOnOwnedWaitQueue(OwnedWaitQueue* owned_wq, TestThread* owner,
                                       zx::duration relative_timeout) {
  BEGIN_TEST;
  ASSERT_EQ(state(), State::CREATED);
  ASSERT_FALSE(static_cast<bool>(op_));

  op_ = [owned_wq, owner_thrd = owner ? owner->thread_ : nullptr, relative_timeout]() {
    AnnotatedAutoEagerReschedDisabler eager_resched_disabler;

    Deadline timeout = (relative_timeout == zx::duration::infinite())
                           ? Deadline::infinite()
                           : Deadline::after_mono(relative_timeout.get());

    owned_wq->BlockAndAssignOwner(timeout, owner_thrd, ResourceOwnership::Normal,
                                  Interruptible::Yes);
  };

  state_.store(State::WAITING_TO_START);
  thread_->Resume();

  ASSERT_TRUE(WaitFor<Condition::BLOCKED>());

  END_TEST;
}

bool TestThread::RunOp(fit::inline_function<void(void), kMaxOpLambdaCaptureStorageBytes> op) {
  BEGIN_TEST;
  ASSERT_EQ(state(), State::CREATED);
  ASSERT_FALSE(static_cast<bool>(op_));

  op_ = ktl::move(op);

  state_.store(State::WAITING_TO_START);
  thread_->Resume();

  ASSERT_TRUE(WaitFor<Condition::STARTED>());

  END_TEST;
}

bool TestThread::Reset(bool explicit_kill) {
  BEGIN_TEST;

  // If we are explicitly killing the thread as part of the test, then we
  // should not expect the shutdown barrier to be cleared.
  if (!explicit_kill) {
    EXPECT_TRUE(allow_shutdown_.is_signaled());
  }

  switch (state()) {
    case State::INITIAL:
      break;
    case State::CREATED:
      // Created but not started?  thread_forget seems to be the proper way to
      // cleanup a thread which was never started.
      ASSERT(thread_ != nullptr);
      thread_->Forget();
      thread_ = nullptr;
      break;

    case State::WAITING_TO_START:
    case State::STARTED:
    case State::WAITING_FOR_SHUTDOWN:
    case State::SHUTDOWN:
      // If we are explicitly killing the thread, send it the kill signal now.
      if (explicit_kill) {
        thread_->Kill();
      }

      // The thread should be on its way to termination as we speak.  Attempt to
      // join it with a relatively short timeout.  If this fails, print a
      // warning and try again with an infinite timeout.  Why try with a short
      // timeout and then an infinite timeout?  We might be running in an
      // emulated or virtualized environment and things may take a lot longer
      // that they otherwise would.  By timing out quickly and printing an
      // warning, we can hopefully make it easier for a developer to figure out
      // what's going on in the case where the second join hangs forever.
      constexpr zx_duration_mono_t timeout = ZX_MSEC(500);
      ASSERT(thread_ != nullptr);
      int ret_code;
      const Deadline join_deadline = Deadline::after_mono(timeout);
      zx_status_t res = thread_->Join(&ret_code, join_deadline.when());
      if (res == ZX_ERR_TIMED_OUT) {
        printf("Timed out while joining thread %p, retrying with infinite timeout\n", thread_);
        res = thread_->Join(&ret_code, ZX_TIME_INFINITE);
      }
      if (res != ZX_OK) {
        panic("join of thread %p failed with %d\n", thread_, res);
      }
      thread_ = nullptr;
  }

  state_.store(State::INITIAL);
  op_ = nullptr;
  initial_profile_ = nullptr;
  ASSERT_NULL(thread_);

  END_TEST;
}

int TestThread::ThreadEntry() {
  if (!static_cast<bool>(op_) || (state() != State::WAITING_TO_START)) {
    return -1;
  }

  state_.store(State::STARTED);
  op_();
  state_.store(State::WAITING_FOR_SHUTDOWN);
  // Do not block on the allow shutdown event if we have received the kill
  // signal.  Zircon events are non-interruptible.
  if ((thread_->signals() & THREAD_SIGNAL_KILL) == 0) {
    allow_shutdown_.Wait();
  }

  state_.store(State::SHUTDOWN);
  op_ = nullptr;

  return 0;
}

template <TestThread::Condition condition>
bool TestThread::WaitFor() {
  BEGIN_TEST;

  constexpr zx_duration_mono_t timeout = ZX_SEC(10);
  constexpr zx_duration_mono_t poll_interval = ZX_USEC(100);
  zx_instant_mono_t deadline = current_mono_time() + timeout;

  while (true) {
    if constexpr (condition == Condition::BLOCKED) {
      thread_state cur_state = tstate();

      if (cur_state == THREAD_BLOCKED) {
        break;
      }

      if (cur_state != THREAD_RUNNING) {
        ASSERT_EQ(THREAD_READY, cur_state);
      }
    } else if constexpr (condition == Condition::STARTED) {
      if (state() == State::STARTED) {
        break;
      }
    } else {
      static_assert(condition == Condition::WAITING_FOR_SHUTDOWN);
      if (state() == State::WAITING_FOR_SHUTDOWN) {
        break;
      }
    }

    zx_instant_mono_t now = current_mono_time();
    ASSERT_LT(now, deadline);
    Thread::Current::SleepRelative(poll_interval);
  }

  END_TEST;
}

bool pi_test_basic() {
  BEGIN_TEST;

  AutoProfileBooster pboost;
  enum class ReleaseMethod { WAKE = 0, TIMEOUT, KILL };
  constexpr ReleaseMethod REL_METHODS[] = {ReleaseMethod::WAKE, ReleaseMethod::TIMEOUT,
                                           ReleaseMethod::KILL};
  constexpr zx::duration TIMEOUT_RELEASE_DURATION = zx::msec(10);
  constexpr uint32_t RETRY_LIMIT = 100;

  // create the array of profiles we will use during the test, then verify that
  // all of them were successfully allocated before proceeding.
  const ktl::array profiles = {
      FairProfile::Create(TEST_DEFAULT_WEIGHT, InheritableProfile::Yes),
      FairProfile::Create(TEST_DEFAULT_WEIGHT + TEST_EPSILON_WEIGHT, InheritableProfile::Yes),
      FairProfile::Create(TEST_DEFAULT_WEIGHT - TEST_EPSILON_WEIGHT, InheritableProfile::Yes),
      FairProfile::Create(TEST_DEFAULT_WEIGHT, InheritableProfile::No),
      FairProfile::Create(TEST_DEFAULT_WEIGHT + TEST_EPSILON_WEIGHT, InheritableProfile::No),
      FairProfile::Create(TEST_DEFAULT_WEIGHT - TEST_EPSILON_WEIGHT, InheritableProfile::No),
      DeadlineProfile::Create(ZX_MSEC(2), ZX_MSEC(5)),
      DeadlineProfile::Create(ZX_USEC(200), ZX_MSEC(1)),
  };

  for (auto& profile : profiles) {
    ASSERT_NONNULL(profile);
  }

  // Test every combination of profiles in a test where one thread waits while
  // another thread blocks behind it, applying profile pressure.  Validate that
  // the receiving thread has the proper effective profile after receiving
  // pressure, and that the effective profile relaxes back to the initial
  // profile after the thread applying pressure ceases to do so for each of the
  // various release methods.
  for (auto& blocking_profile : profiles) {
    for (auto& pressure_profile : profiles) {
      for (auto rel_method : REL_METHODS) {
        PRINT_LOOP_ITER(blocking_profile);
        PRINT_LOOP_ITER(pressure_profile);
        PRINT_LOOP_ITER(rel_method);

        uint32_t retry_count = 0;
        bool retry_test;
        do {
          retry_test = false;

          LockedOwnedWaitQueue owq;
          TestThread pressure_thread;
          TestThread blocking_thread;
          ExpectedEffectiveProfile expected_profile;
          unittest::ThreadEffectiveProfileObserver observer;

          auto cleanup = fit::defer([&]() {
            TestThread::ClearShutdownBarrier();
            owq.ReleaseAllThreads();
            pressure_thread.Reset();
            blocking_thread.Reset();
          });

          // Make sure that our default barriers have been reset to their proper
          // initial states.
          TestThread::ResetShutdownBarrier();

          // Create 2 threads, each with the appropriate profile.
          ASSERT_TRUE(blocking_thread.Create(blocking_profile));
          ASSERT_TRUE(pressure_thread.Create(pressure_profile));

          // Start the first thread, wait for it to block, and verify that it's
          // profile is correct (it should not be changed).
          ASSERT_TRUE(blocking_thread.DoStall());
          blocking_profile->SetExpectedBaseProfile(expected_profile);
          observer.Observe(blocking_thread.thread());
          ASSERT_TRUE(observer.VerifyExpectedEffectiveProfile(expected_profile));

          // Start the second thread, and have it block on the owned wait queue,
          // and declare the blocking thread to be the owner of the queue at the
          // same time.  Then check to be sure that the effective priority of the
          // blocking thread matches what we expect to see.
          zx::duration relative_timeout = (rel_method == ReleaseMethod::TIMEOUT)
                                              ? TIMEOUT_RELEASE_DURATION
                                              : zx::duration::infinite();
          ASSERT_TRUE(
              pressure_thread.BlockOnOwnedWaitQueue(&owq, &blocking_thread, relative_timeout));

          // Observe the effective profile of the blocking thread, then observe
          // the state of the thread applying pressure.  If this is the TIMEOUT
          // test, the thread *must* still be blocked on |owq| (not timed out yet)
          // in order for the test to be considered valid.  If the thread managed
          // to unblock before we could observe its effective priority, just try
          // again.
          observer.Observe(blocking_thread.thread());
          if (rel_method == ReleaseMethod::TIMEOUT) {
            retry_test = (pressure_thread.tstate() != thread_state::THREAD_BLOCKED) ||
                         (pressure_thread.state() == TestThread::State::WAITING_FOR_SHUTDOWN);
          }

          // Only verify this if we managed to observe the blocked thread's
          // effective profile while the pressure thread was still applying
          // pressure.
          if (!retry_test) {
            pressure_profile->AccumulateExpectedPressure(expected_profile);
            ASSERT_TRUE(observer.VerifyExpectedEffectiveProfile(expected_profile));
          }

          // Finally, release the thread from the owned wait queue based on
          // the release method we are testing.  We will either explicitly
          // wake it up, let it time out, or kill the thread outright.
          //
          // Then, verify that the profile drops back down to what it
          // was initially.
          switch (rel_method) {
            case ReleaseMethod::WAKE:
              owq.ReleaseAllThreads();
              break;

            case ReleaseMethod::TIMEOUT:
              // Wait until the pressure thread times out and has exited.
              ASSERT_TRUE(pressure_thread.WaitFor<TestThread::Condition::WAITING_FOR_SHUTDOWN>());
              break;

            case ReleaseMethod::KILL:
              pressure_thread.Reset(true);
              break;
          }

          blocking_profile->SetExpectedBaseProfile(expected_profile);
          observer.Observe(blocking_thread.thread());
          ASSERT_TRUE(observer.VerifyExpectedEffectiveProfile(expected_profile));
        } while (retry_test && (++retry_count < RETRY_LIMIT));

        ASSERT_FALSE(retry_test, "Failed timeout race too many times!");

        print_blocking_profile.cancel();
        print_pressure_profile.cancel();
        print_rel_method.cancel();
      }
    }
  }

  END_TEST;
}

bool pi_test_changing_priority() {
  BEGIN_TEST;

  AutoProfileBooster pboost;
  LockedOwnedWaitQueue owq;
  TestThread pressure_thread;
  TestThread blocking_thread;

  auto cleanup = fit::defer([&]() {
    TestThread::ClearShutdownBarrier();
    owq.ReleaseAllThreads();
    pressure_thread.Reset();
    blocking_thread.Reset();
  });

  const ktl::array profiles = {
      FairProfile::Create(TEST_DEFAULT_WEIGHT, InheritableProfile::Yes),
      FairProfile::Create(TEST_DEFAULT_WEIGHT + TEST_EPSILON_WEIGHT, InheritableProfile::Yes),
      FairProfile::Create(TEST_DEFAULT_WEIGHT - TEST_EPSILON_WEIGHT, InheritableProfile::Yes),
      FairProfile::Create(TEST_LOWEST_WEIGHT, InheritableProfile::Yes),
      FairProfile::Create(TEST_HIGHEST_WEIGHT, InheritableProfile::Yes),
      FairProfile::Create(TEST_DEFAULT_WEIGHT, InheritableProfile::No),
      FairProfile::Create(TEST_DEFAULT_WEIGHT + TEST_EPSILON_WEIGHT, InheritableProfile::No),
      FairProfile::Create(TEST_DEFAULT_WEIGHT - TEST_EPSILON_WEIGHT, InheritableProfile::No),
      FairProfile::Create(TEST_LOWEST_WEIGHT, InheritableProfile::No),
      FairProfile::Create(TEST_HIGHEST_WEIGHT, InheritableProfile::No),
      DeadlineProfile::Create(ZX_MSEC(2), ZX_MSEC(5)),
      DeadlineProfile::Create(ZX_USEC(200), ZX_MSEC(1)),
  };

  for (auto& profile : profiles) {
    ASSERT_NONNULL(profile);
  }

  ExpectedEffectiveProfile expected_profile;
  unittest::ThreadEffectiveProfileObserver observer;

  // Make sure that our default barriers have been reset to their proper
  // initial states.
  TestThread::ResetShutdownBarrier();

  // Create our threads.
  ASSERT_TRUE(blocking_thread.Create(profiles[0]));
  ASSERT_TRUE(pressure_thread.Create(profiles[0]));

  // Start the first thread, wait for it to block, and verify that it's
  // profile is correct (it should not be changed).
  ASSERT_TRUE(blocking_thread.DoStall());
  blocking_thread.initial_profile()->SetExpectedBaseProfile(expected_profile);
  observer.Observe(blocking_thread.thread());
  ASSERT_TRUE(observer.VerifyExpectedEffectiveProfile(expected_profile));

  // Block the second thread behind the first.
  ASSERT_TRUE(pressure_thread.BlockOnOwnedWaitQueue(&owq, &blocking_thread));
  pressure_thread.initial_profile()->AccumulateExpectedPressure(expected_profile);
  observer.Observe(blocking_thread.thread());
  ASSERT_TRUE(observer.VerifyExpectedEffectiveProfile(expected_profile));

  // Changing the pressure thread's profile to a number of different profiles,
  // verifying that the pressure felt by the blocking thread changes
  // appropriately as we do.
  for (const auto& profile : profiles) {
    PRINT_LOOP_ITER(profile);

    profile->Apply(pressure_thread.thread());
    observer.Observe(blocking_thread.thread());

    blocking_thread.initial_profile()->SetExpectedBaseProfile(expected_profile);
    profile->AccumulateExpectedPressure(expected_profile);
    ASSERT_TRUE(observer.VerifyExpectedEffectiveProfile(expected_profile));

    print_profile.cancel();
  }

  // Release the pressure thread, validate that the priority is what we
  // started with and we are done.
  owq.ReleaseAllThreads();
  blocking_thread.initial_profile()->SetExpectedBaseProfile(expected_profile);
  observer.Observe(blocking_thread.thread());
  ASSERT_TRUE(observer.VerifyExpectedEffectiveProfile(expected_profile));

  END_TEST;
}

// A simple smoke test to make sure that we can change a thread's base priority
// when it is blocked in a normal wait queue (instead of an owned wait queue)
bool pi_test_changing_priority_in_wait_queue() {
  BEGIN_TEST;

  AutoProfileBooster pboost;
  WaitQueue wq;
  TestThread thread;

  auto cleanup = fit::defer([&]() {
    TestThread::ClearShutdownBarrier();
    wq.WakeAll(ZX_OK);
    thread.Reset();
  });

  const ktl::array profiles = {
      FairProfile::Create(TEST_DEFAULT_WEIGHT, InheritableProfile::Yes),
      FairProfile::Create(TEST_DEFAULT_WEIGHT + TEST_EPSILON_WEIGHT, InheritableProfile::Yes),
      FairProfile::Create(TEST_DEFAULT_WEIGHT - TEST_EPSILON_WEIGHT, InheritableProfile::Yes),
      FairProfile::Create(TEST_LOWEST_WEIGHT, InheritableProfile::Yes),
      FairProfile::Create(TEST_HIGHEST_WEIGHT, InheritableProfile::Yes),
      DeadlineProfile::Create(ZX_MSEC(2), ZX_MSEC(5)),
      DeadlineProfile::Create(ZX_USEC(200), ZX_MSEC(1)),
  };

  for (auto& profile : profiles) {
    ASSERT_NONNULL(profile);
  }

  ExpectedEffectiveProfile expected_profile;
  unittest::ThreadEffectiveProfileObserver observer;

  // Make sure that our default barriers have been reset to their proper
  // initial states.
  TestThread::ResetShutdownBarrier();

  // Create our thread and have it block in our wait queue.
  ASSERT_TRUE(thread.Create(profiles[0]));
  ASSERT_TRUE(thread.BlockOnWaitQueue(&wq));
  thread.initial_profile()->SetExpectedBaseProfile(expected_profile);
  observer.Observe(thread.thread());
  ASSERT_TRUE(observer.VerifyExpectedEffectiveProfile(expected_profile));

  // Changing the thread's base profile while it is blocked in the queue.  Its
  // effective profile should always match the base profile we set.
  for (const auto& profile : profiles) {
    PRINT_LOOP_ITER(profile);

    profile->Apply(thread.thread());
    profile->SetExpectedBaseProfile(expected_profile);
    observer.Observe(thread.thread());
    ASSERT_TRUE(observer.VerifyExpectedEffectiveProfile(expected_profile));

    print_profile.cancel();
  }

  END_TEST;
}

bool pi_test_chain() {
  BEGIN_TEST;

  enum class ReleaseOrder : uint64_t { ASCENDING = 0, DESCENDING };
  struct Link {
    LockedOwnedWaitQueue queue;
    bool active = false;
  };

  const ktl::array profile_deck{
      FairProfile::Create(TEST_DEFAULT_WEIGHT, InheritableProfile::Yes),
      FairProfile::Create(TEST_LOWEST_WEIGHT, InheritableProfile::Yes),
      FairProfile::Create(TEST_HIGHEST_WEIGHT, InheritableProfile::Yes),
      FairProfile::Create(TEST_DEFAULT_WEIGHT, InheritableProfile::No),
      FairProfile::Create(TEST_LOWEST_WEIGHT, InheritableProfile::No),
      FairProfile::Create(TEST_HIGHEST_WEIGHT, InheritableProfile::No),
      DeadlineProfile::Create(ZX_MSEC(2), ZX_MSEC(5)),
      DeadlineProfile::Create(ZX_USEC(200), ZX_MSEC(1)),
  };
  constexpr size_t CHAIN_LEN = profile_deck.size();

  const ktl::array PRIORITY_GENERATORS = {
      DistroSpec{DistroSpec::Type::ASCENDING},
      DistroSpec{DistroSpec::Type::DESCENDING},
      DistroSpec{DistroSpec::Type::SHUFFLE, 0xbd6f3bfe33d51c8e},
      DistroSpec{DistroSpec::Type::SHUFFLE, 0x857ce1aa3209ecc7},
  };

  const ktl::array RELEASE_ORDERS = {
      DistroSpec{DistroSpec::Type::ASCENDING},
      DistroSpec{DistroSpec::Type::DESCENDING},
      DistroSpec{DistroSpec::Type::SHUFFLE, 0xac8d4a8ed016caf0},
      DistroSpec{DistroSpec::Type::SHUFFLE, 0xb51e76ca5cf20875},
  };

  ktl::array<TestThread, CHAIN_LEN> threads;
  ktl::array<Link, CHAIN_LEN - 1> links;
  ktl::array<size_t, CHAIN_LEN - 1> release_deck;

  for (auto& profile : profile_deck) {
    ASSERT_NONNULL(profile);
  }

  for (size_t i = 0; i < ktl::size(release_deck); ++i) {
    release_deck[i] = i;
  }

  AutoProfileBooster pboost;
  for (uint32_t pgen_ndx = 0; pgen_ndx < ktl::size(PRIORITY_GENERATORS); ++pgen_ndx) {
    PRINT_LOOP_ITER(pgen_ndx);

    // Generate the profile order to use for this pass.
    ktl::array<fbl::RefPtr<Profile>, CHAIN_LEN> profiles;
    PRIORITY_GENERATORS[pgen_ndx].Apply(profile_deck, profiles);

    for (uint32_t ro_ndx = 0; ro_ndx < ktl::size(RELEASE_ORDERS); ++ro_ndx) {
      PRINT_LOOP_ITER(ro_ndx);

      // Generate the order in which we will release the links for this
      // pass
      decltype(release_deck) release_ordering;
      RELEASE_ORDERS[ro_ndx].Apply(release_deck, release_ordering);

      auto cleanup = fit::defer([&]() {
        TestThread::ClearShutdownBarrier();
        for (auto& l : links) {
          l.queue.ReleaseAllThreads();
        }
        for (auto& t : threads) {
          t.Reset();
        }
      });

      // Lambda used to validate the effective profiles of the threads currently
      // involved in the chain.
      auto ValidatePriorities = [&]() -> bool {
        BEGIN_TEST;

        for (size_t tndx = ktl::size(threads); tndx-- > 0;) {
          PRINT_LOOP_ITER(tndx);

          // All threads should either be created, started or waiting for
          // shutdown.  If they are merely created, they have no effective
          // priority to evaluate at the moment, so just skip them.
          const auto& t = threads[tndx];
          const TestThread::State cur_state = t.state();
          if (cur_state == TestThread::State::CREATED) {
            print_tndx.cancel();
            continue;
          }

          if (cur_state != TestThread::State::WAITING_FOR_SHUTDOWN) {
            ASSERT_EQ(TestThread::State::STARTED, cur_state);
          }

          // The effective profile of this thread should be its base profile,
          // plus all of the profile pressure received from the actively linked
          // thread.
          ExpectedEffectiveProfile expected_profile;
          threads[tndx].initial_profile()->SetExpectedBaseProfile(expected_profile);
          for (size_t i = tndx; (i < ktl::size(links)) && (links[i].active); ++i) {
            ASSERT_LT(i + 1, ktl::size(threads));
            threads[i + 1].initial_profile()->AccumulateExpectedPressure(expected_profile);
          }

          unittest::ThreadEffectiveProfileObserver observer;
          observer.Observe(threads[tndx].thread());
          ASSERT_TRUE(observer.VerifyExpectedEffectiveProfile(expected_profile));

          print_tndx.cancel();
        }

        END_TEST;
      };

      // Make sure that our default barriers have been reset to their proper
      // initial states.
      TestThread::ResetShutdownBarrier();

      // Create our threads.
      static_assert(ktl::size(threads) == ktl::size(profiles));
      for (uint32_t tndx = 0; tndx < ktl::size(threads); ++tndx) {
        PRINT_LOOP_ITER(tndx);
        ASSERT_TRUE(threads[tndx].Create(profiles[tndx]));
        print_tndx.cancel();
      }

      // Start the head of the chain, wait for it to block, then verify that its
      // profile is correct (it should not be changed).
      auto& chain_head = threads[0];
      ASSERT_TRUE(chain_head.DoStall());
      ASSERT_TRUE(ValidatePriorities());

      // Start each of the threads in the chain one at a time.  Make sure that the
      // pressure of the threads in the chain is properly transmitted each time.
      for (uint32_t tndx = 1; tndx < ktl::size(threads); ++tndx) {
        PRINT_LOOP_ITER(tndx);

        auto& link = links[tndx - 1];
        ASSERT_TRUE(threads[tndx].BlockOnOwnedWaitQueue(&link.queue, &threads[tndx - 1]));
        link.active = true;
        ASSERT_TRUE(ValidatePriorities());

        print_tndx.cancel();
      }

      // Tear down the chain according to the release ordering for this
      // pass.  Make sure that the priority properly relaxes for each of
      // the threads as we do so.
      for (auto link_ndx : release_ordering) {
        PRINT_LOOP_ITER(link_ndx);

        ASSERT_LT(link_ndx, ktl::size(links));
        auto& link = links[link_ndx];
        link.queue.ReleaseAllThreads();
        link.active = false;
        ASSERT_TRUE(ValidatePriorities());

        print_link_ndx.cancel();
      }

      print_ro_ndx.cancel();
    }

    print_pgen_ndx.cancel();
  }

  END_TEST;
}

bool pi_test_multi_waiter() {
  BEGIN_TEST;
  AutoProfileBooster pboost;

  const ktl::array profile_deck{
      FairProfile::Create(TEST_DEFAULT_WEIGHT, InheritableProfile::Yes),
      FairProfile::Create(TEST_LOWEST_WEIGHT, InheritableProfile::Yes),
      FairProfile::Create(TEST_HIGHEST_WEIGHT, InheritableProfile::Yes),
      FairProfile::Create(TEST_DEFAULT_WEIGHT, InheritableProfile::No),
      FairProfile::Create(TEST_LOWEST_WEIGHT, InheritableProfile::No),
      FairProfile::Create(TEST_HIGHEST_WEIGHT, InheritableProfile::No),
      DeadlineProfile::Create(ZX_MSEC(2), ZX_MSEC(5)),
      DeadlineProfile::Create(ZX_USEC(200), ZX_MSEC(1)),
  };
  constexpr size_t WAITER_CNT = profile_deck.size();

  const ktl::array PRIORITY_GENERATORS = {
      DistroSpec{DistroSpec::Type::ASCENDING},
      DistroSpec{DistroSpec::Type::DESCENDING},
      DistroSpec{DistroSpec::Type::RANDOM, 0x87251211471cb789},
      DistroSpec{DistroSpec::Type::SHUFFLE, 0x857ce1aa3209ecc7},
  };

  LockedOwnedWaitQueue blocking_queue;
  TestThread blocking_thread;
  struct Waiter {
    TestThread thread;
    bool started = false;
    bool is_waiting = false;

    void Reset() {
      thread.Reset();
      started = false;
      is_waiting = false;
    }
  };
  ktl::array<Waiter, WAITER_CNT> waiters;

  for (const auto& bt_profile : profile_deck) {
    PRINT_LOOP_ITER(bt_profile);

    for (uint32_t pgen_ndx = 0; pgen_ndx < ktl::size(PRIORITY_GENERATORS); ++pgen_ndx) {
      PRINT_LOOP_ITER(pgen_ndx);

      // At the end of the tests, success or failure, be sure to clean up.
      auto cleanup = fit::defer([&]() {
        TestThread::ClearShutdownBarrier();
        blocking_queue.ReleaseAllThreads();
        blocking_thread.Reset();
        for (auto& w : waiters) {
          w.Reset();
        }
      });

      // Make sure that our barriers have been reset.
      TestThread::ResetShutdownBarrier();

      // Select the profiles to apply to the waiter threads during this pass.
      ktl::array<fbl::RefPtr<Profile>, WAITER_CNT> profiles;
      PRIORITY_GENERATORS[pgen_ndx].Apply(profile_deck, profiles);

      // Create all of the threads.
      ASSERT_TRUE(blocking_thread.Create(bt_profile));
      for (uint32_t waiter_ndx = 0; waiter_ndx < ktl::size(waiters); ++waiter_ndx) {
        PRINT_LOOP_ITER(waiter_ndx);

        static_assert(ktl::size(waiters) == ktl::size(profiles));
        ASSERT_TRUE(waiters[waiter_ndx].thread.Create(profiles[waiter_ndx]));

        print_waiter_ndx.cancel();
      }

      // Define a small lambda we will use to validate the expected priorities of
      // each of our threads.
      TestThread* current_owner = &blocking_thread;
      auto ValidatePriorities = [&]() -> bool {
        BEGIN_TEST;

        ExpectedEffectiveProfile expected_profile;
        unittest::ThreadEffectiveProfileObserver observer;

        // The expected profile for the current owner of the OWQ should be its
        // base profile, plus all of the pressure from each of the waiting
        // threads.
        ASSERT_NONNULL(current_owner);
        current_owner->initial_profile()->SetExpectedBaseProfile(expected_profile);
        for (const auto& waiter : waiters) {
          if (waiter.is_waiting) {
            waiter.thread.initial_profile()->AccumulateExpectedPressure(expected_profile);
          }
        }

        observer.Observe(current_owner->thread());
        ASSERT_TRUE(observer.VerifyExpectedEffectiveProfile(expected_profile));

        // Every waiter thread which has started (waiting or not) should be
        // running with its initial profile, unless it happens to currently be
        // the owner of the OWQ.
        for (size_t waiter_ndx = 0; waiter_ndx < ktl::size(waiters); ++waiter_ndx) {
          PRINT_LOOP_ITER(waiter_ndx);

          const Waiter& waiter = waiters[waiter_ndx];
          if (waiter.started && (&waiter.thread != current_owner)) {
            waiter.thread.initial_profile()->SetExpectedBaseProfile(expected_profile);
            observer.Observe(waiter.thread.thread());
            ASSERT_TRUE(observer.VerifyExpectedEffectiveProfile(expected_profile));
          }

          print_waiter_ndx.cancel();
        }

        END_TEST;
      };

      // Start the blocking thread.
      ASSERT_TRUE(blocking_thread.DoStall());
      ASSERT_TRUE(ValidatePriorities());

      // Start each of the threads and have them block on the blocking_queue,
      // declaring blocking_thread to be the owner as they go.  Verify all of
      // the threads' effective profiles as we go.
      for (uint32_t waiter_ndx = 0; waiter_ndx < ktl::size(waiters); ++waiter_ndx) {
        PRINT_LOOP_ITER(waiter_ndx);

        auto& w = waiters[waiter_ndx];
        ASSERT_TRUE(w.thread.BlockOnOwnedWaitQueue(&blocking_queue, current_owner));
        w.started = true;
        w.is_waiting = true;
        ASSERT_TRUE(ValidatePriorities());

        print_waiter_ndx.cancel();
      }

      // Now wake the threads, one at a time, assigning ownership to the thread
      // which was woken each time.  Note that we should not be assuming which
      // thread is going to be woken.  We will need to request that a thread be
      // woken, then figure out after the fact which one was.
      for (uint32_t tndx = 0; tndx < ktl::size(waiters); ++tndx) {
        PRINT_LOOP_ITER(tndx);

        blocking_queue.ReleaseOneThread();

        TestThread* new_owner = nullptr;
        zx_instant_mono_t deadline = current_mono_time() + ZX_SEC(10);
        while (current_mono_time() < deadline) {
          for (auto& w : waiters) {
            // If the waiter's is_waiting flag is set, but the thread has
            // reached the WAITING_FOR_SHUTDOWN state, then we know that
            // this was a thread which was just woken.
            if (w.is_waiting && (w.thread.state() == TestThread::State::WAITING_FOR_SHUTDOWN)) {
              new_owner = &w.thread;
              w.is_waiting = false;
              break;
            }
          }

          if (new_owner != nullptr) {
            break;
          }

          Thread::Current::SleepRelative(ZX_USEC(100));
        }

        // Sanity checks.  Make sure that the new owner exists, and is not the
        // same as the old owner.  Also make sure that none of the other threads
        // have been released but have not been recognized yet.
        ASSERT_NONNULL(new_owner);
        ASSERT_NE(new_owner, current_owner);
        for (auto& w : waiters) {
          if (w.is_waiting) {
            ASSERT_EQ(TestThread::State::STARTED, w.thread.state());
          } else {
            ASSERT_EQ(TestThread::State::WAITING_FOR_SHUTDOWN, w.thread.state());
          }
        }
        current_owner = new_owner;

        // Validate our profiles.
        ASSERT_TRUE(ValidatePriorities());

        print_tndx.cancel();
      }

      print_pgen_ndx.cancel();
    }
    print_bt_profile.cancel();
  }

  END_TEST;
}

bool pi_test_multi_owned_queues() {
  BEGIN_TEST;
  AutoProfileBooster pboost;

  const ktl::array profile_deck{
      FairProfile::Create(TEST_DEFAULT_WEIGHT, InheritableProfile::Yes),
      FairProfile::Create(TEST_LOWEST_WEIGHT, InheritableProfile::Yes),
      FairProfile::Create(TEST_HIGHEST_WEIGHT, InheritableProfile::Yes),
      FairProfile::Create(TEST_DEFAULT_WEIGHT, InheritableProfile::No),
      FairProfile::Create(TEST_LOWEST_WEIGHT, InheritableProfile::No),
      FairProfile::Create(TEST_HIGHEST_WEIGHT, InheritableProfile::No),
      DeadlineProfile::Create(ZX_MSEC(2), ZX_MSEC(5)),
      DeadlineProfile::Create(ZX_USEC(200), ZX_MSEC(1)),
  };
  constexpr size_t QUEUE_CNT = profile_deck.size();

  struct Waiter {
    TestThread thread;
    LockedOwnedWaitQueue queue;
    bool is_started = false;
    bool is_waiting = false;

    void Reset() {
      queue.ReleaseAllThreads();
      thread.Reset();
      is_started = false;
      is_waiting = false;
    }
  };

  TestThread blocking_thread;
  ktl::array<Waiter, QUEUE_CNT> queues;

  const ktl::array PRIORITY_GENERATORS = {
      DistroSpec{DistroSpec::Type::ASCENDING},
      DistroSpec{DistroSpec::Type::DESCENDING},
      DistroSpec{DistroSpec::Type::RANDOM, 0xb89e3b7442b95a1c},
      DistroSpec{DistroSpec::Type::SHUFFLE, 0x06ec82d4ade8efba},
  };

  for (const auto& bt_profile : profile_deck) {
    PRINT_LOOP_ITER(bt_profile);

    for (uint32_t pgen_ndx = 0; pgen_ndx < ktl::size(PRIORITY_GENERATORS); ++pgen_ndx) {
      PRINT_LOOP_ITER(pgen_ndx);

      // At the end of the tests, success or failure, be sure to clean up.
      auto cleanup = fit::defer([&]() {
        TestThread::ClearShutdownBarrier();
        blocking_thread.Reset();
        for (auto& q : queues) {
          q.Reset();
        }
      });

      // Make sure that our barriers have been reset.
      TestThread::ResetShutdownBarrier();

      // Select the profiles to apply to the waiter threads during this pass.
      ktl::array<fbl::RefPtr<Profile>, QUEUE_CNT> profiles;
      PRIORITY_GENERATORS[pgen_ndx].Apply(profile_deck, profiles);

      // Create all of the threads.
      ASSERT_TRUE(blocking_thread.Create(bt_profile));
      for (uint32_t queue_ndx = 0; queue_ndx < ktl::size(queues); ++queue_ndx) {
        PRINT_LOOP_ITER(queue_ndx);
        ASSERT_TRUE(queues[queue_ndx].thread.Create(profiles[queue_ndx]));
        print_queue_ndx.cancel();
      }

      // Define a small lambda we will use to validate the expected priorities of
      // each of our threads.
      auto ValidatePriorities = [&]() -> bool {
        BEGIN_TEST;

        ExpectedEffectiveProfile expected_profile;
        unittest::ThreadEffectiveProfileObserver observer;

        // Each of the started queue threads (waiting or not) should simply have their
        // base profile.  Verify this.
        for (uint32_t queue_ndx = 0; queue_ndx < ktl::size(queues); ++queue_ndx) {
          PRINT_LOOP_ITER(queue_ndx);
          const auto& q = queues[queue_ndx];

          if (q.is_started) {
            q.thread.initial_profile()->SetExpectedBaseProfile(expected_profile);
            observer.Observe(q.thread.thread());
            ASSERT_TRUE(observer.VerifyExpectedEffectiveProfile(expected_profile));
          }

          print_queue_ndx.cancel();
        }

        // The effective profile of the blocking_thread should be the
        // combination of its base profile, in addition to all of the currently
        // blocked threads whose OWQs are owned by the blocking thread.
        blocking_thread.initial_profile()->SetExpectedBaseProfile(expected_profile);
        for (const auto& q : queues) {
          if (q.is_waiting) {
            q.thread.initial_profile()->AccumulateExpectedPressure(expected_profile);
          }
        }

        observer.Observe(blocking_thread.thread());
        ASSERT_TRUE(observer.VerifyExpectedEffectiveProfile(expected_profile));

        END_TEST;
      };

      // Start the blocking thread.
      ASSERT_TRUE(blocking_thread.DoStall());
      ASSERT_TRUE(ValidatePriorities());

      // Start each of the threads and have them block on their associated
      // queue, declaring blocking_thread to be the owner of their queue
      // as they go.  Validate priorities at each step.
      for (uint32_t queue_ndx = 0; queue_ndx < ktl::size(queues); ++queue_ndx) {
        PRINT_LOOP_ITER(queue_ndx);

        auto& q = queues[queue_ndx];
        ASSERT_TRUE(q.thread.BlockOnOwnedWaitQueue(&q.queue, &blocking_thread));
        q.is_started = true;
        q.is_waiting = true;
        ASSERT_TRUE(ValidatePriorities());

        print_queue_ndx.cancel();
      }

      // Now wake the threads, one at a time, verifying priorities as we
      // go.
      for (uint32_t queue_ndx = 0; queue_ndx < ktl::size(queues); ++queue_ndx) {
        PRINT_LOOP_ITER(queue_ndx);

        auto& q = queues[queue_ndx];
        q.queue.ReleaseOneThread();
        q.is_waiting = false;
        ASSERT_TRUE(ValidatePriorities());

        print_queue_ndx.cancel();
      }

      print_pgen_ndx.cancel();
    }
    print_bt_profile.cancel();
  }

  END_TEST;
}

template <InheritableProfile kInheritableProfiles>
bool pi_test_cycle() {
  BEGIN_TEST;
  AutoProfileBooster pboost;

  // Deliberately create a cycle and make sure that we don't hang or otherwise
  // exhibit bad behavior.
  struct Link {
    TestThread thread;
    LockedOwnedWaitQueue link;
    fbl::RefPtr<Profile> profile;
  };

  static constexpr size_t CYCLE_LEN = 4;
  ktl::array<Link, CYCLE_LEN> nodes;

  // At the end of the tests, success or failure, be sure to clean up.
  auto cleanup = fit::defer([&]() {
    TestThread::ClearShutdownBarrier();
    for (auto& n : nodes) {
      n.link.ReleaseAllThreads();
    }
    for (auto& n : nodes) {
      n.thread.Reset();
    }
  });

  // Create each of the profiles and assign them to each of the threads.
  for (uint32_t tndx = 0; tndx < ktl::size(nodes); ++tndx) {
    PRINT_LOOP_ITER(tndx);

    SchedWeight tgt_weight = TEST_LOWEST_WEIGHT + (tndx * TEST_EPSILON_WEIGHT);
    ASSERT_LE(tgt_weight.raw_value(), TEST_HIGHEST_WEIGHT.raw_value());
    nodes[tndx].profile = FairProfile::Create(tgt_weight, kInheritableProfiles);
    ASSERT_NONNULL(nodes[tndx].profile);
    ASSERT_TRUE(nodes[tndx].thread.Create(nodes[tndx].profile));

    print_tndx.cancel();
  }

  // Let each thread run, blocking it on its own link and declaring the next
  // thread in the list to be the owner of the link.  When we hit the last
  // thread, we attempt to form a cycle.
  //
  // As of today, the OwnedWaitQueue code will refuse to create the cycle and
  // will "fix" the problem by allowing the thread to block, but declaring the
  // owner of the OWQ the thread is blocking in to be no one.  Our threads are
  // in ascending priority order, so we should not see any changes to the
  // effective priority of any of the threads.
  //
  // Eventually, however, the Block operation will completely fail instead of
  // allowing the cycle to come into existence.  It will not change the owner,
  // nor will it block the thread in question. Instead, it will return an error.
  // This test will need to be updated when that day arrives.
  for (size_t tndx = 0; tndx < ktl::size(nodes); ++tndx) {
    PRINT_LOOP_ITER(tndx);

    TestThread& owner_thread = nodes[(tndx + 1) % ktl::size(nodes)].thread;
    LockedOwnedWaitQueue& link = nodes[tndx].link;
    ASSERT_TRUE(nodes[tndx].thread.BlockOnOwnedWaitQueue(&link, &owner_thread));

    for (size_t validation_ndx = 0; validation_ndx <= tndx; ++validation_ndx) {
      PRINT_LOOP_ITER(validation_ndx);

      // The profile of each link in the chain should be the combination of its
      // base profile and all of the threads in the chain which are blocked
      // behind it.
      ExpectedEffectiveProfile expected_profile;
      nodes[validation_ndx].thread.initial_profile()->SetExpectedBaseProfile(expected_profile);
      for (size_t i = 1; i <= validation_ndx; ++i) {
        TestThread& thread = nodes[validation_ndx - i].thread;
        thread.initial_profile()->AccumulateExpectedPressure(expected_profile);
      }

      unittest::ThreadEffectiveProfileObserver observer;
      observer.Observe(nodes[validation_ndx].thread.thread());
      ASSERT_TRUE(observer.VerifyExpectedEffectiveProfile(expected_profile));

      // Every OWQ in the test vector should be owned by the thread after it in
      // the sequence, except for the last OWQ.  We tried to assign it to be
      // owned by the first thread when we tried to create the cycle, but the
      // implementation should have refused and left the queue with now owner.
      auto ObserveOwner = [](OwnedWaitQueue& queue) -> const Thread* {
        SingleChainLockGuard guard{IrqSaveOption, queue.get_lock(), CLT_TAG("pi_test_cycle")};
        return queue.owner();
      };

      const Thread* const expected_owner =
          (tndx < (nodes.size() - 1)) ? &(nodes[tndx + 1].thread.thread()) : nullptr;
      const Thread* const observed_owner = ObserveOwner(nodes[tndx].link);
      ASSERT_EQ(expected_owner, observed_owner);

      print_validation_ndx.cancel();
    }

    print_tndx.cancel();
  }

  END_TEST;
}

bool bug_42182770_regression() {
  BEGIN_TEST;

  // Set up and test a situation which mimics the one encountered in during bug 42182770.
  // Basically, we want to:
  //
  // 1) Block a thread A which has a fair profile in an owned wait queue W.  Be
  //    sure to test both inheritable and non-inheritable profiles.
  // 2) Observe that the start time, finish time, and time slice of W (the
  //    "dynamic" IPVs) remain zero (the initial defaults)  A has blocked in W,
  //    and it was the first thread to block, but its profile is
  //    non-inheritable, therefore the dynamic parameters remain undefined.
  // 3) Block a thread B, which has an inheritable deadline profile in W.
  // 4) Observe that W's IPVs have taken on B's dynamic scheduler profile values.
  // 5) Unblock B from W.  Observe that W still has IPV storage allocated to it,
  //    but that the static parameters (total weight, uncapped utilization, min
  //    deadline) have returned to their default values.  If extra scheduler
  //    invariant validation is turned on, the static parameters should have
  //    returned to their defaults as well, otherwise they should remain
  //    unchanged.
  // 6) Unblock A from W.  After this operation, W should no longer have any
  //    inherited profile value storage allocated to it as it no longer has any
  //    waiting threads.

  constexpr ktl::array kInheritableOptions = {InheritableProfile::No, InheritableProfile::Yes};
  for (const InheritableProfile allow_inherit : kInheritableOptions) {
    // Create the queue, profiles, and threads we will need to run the test.
    fbl::AllocChecker ac;
    ktl::unique_ptr<LockedOwnedWaitQueue> owq = ktl::make_unique<LockedOwnedWaitQueue>(&ac);
    ASSERT_TRUE(ac.check());

    fbl::RefPtr<Profile> fair_profile = FairProfile::Create(TEST_DEFAULT_WEIGHT, allow_inherit);
    fbl::RefPtr<Profile> deadline_profile = DeadlineProfile::Create(ZX_MSEC(2), ZX_MSEC(5));
    ASSERT_NONNULL(fair_profile);
    ASSERT_NONNULL(deadline_profile);

    const SchedWeight inheritable_weight =
        (allow_inherit == InheritableProfile::Yes) ? TEST_DEFAULT_WEIGHT : SchedWeight{0};

    TestThread fair_thread, deadline_thread;
    ASSERT_TRUE(fair_thread.Create(fair_profile));
    ASSERT_TRUE(deadline_thread.Create(deadline_profile));

    // Make sure that our default barriers have been reset to their proper initial
    // states, and that we will properly release threads from their blocked state
    // if anything goes wrong during the test and we bail out.
    TestThread::ResetShutdownBarrier();
    auto cleanup = fit::defer([&]() {
      TestThread::ClearShutdownBarrier();
      owq->ReleaseAllThreads();
      fair_thread.Reset();
      deadline_thread.Reset();
    });

    // Verify that the OWQ has no inherited profile storage allocated to it.  This
    // should always be the case when there are no waiters.
    EXPECT_FALSE(owq->HasInheritedSchedulerStateStorage());

    // Now, block the fair thread on the queue, and verify the IPV state.  There
    // should be storage allocated at this point in time, but because the
    // blocked thread's profile is not inheritable, there should be no inherited
    // utilization in the queue's values.
    ASSERT_TRUE(fair_thread.BlockOnOwnedWaitQueue(owq.get(), nullptr));
    {
      SingleChainLockGuard guard{IrqSaveOption, owq->get_lock(),
                                 CLT_TAG("pi_tests::bug_42182770_regression #1")};
      ASSERT_NONNULL(owq->inherited_scheduler_state_storage());

      const SchedulerState::WaitQueueInheritedSchedulerState& iss =
          *owq->const_inherited_scheduler_state_storage();
      EXPECT_EQ(iss.ipvs.total_weight.raw_value(), inheritable_weight.raw_value());
      EXPECT_EQ(iss.ipvs.uncapped_utilization.raw_value(), SchedUtilization{0}.raw_value());
      EXPECT_EQ(iss.ipvs.min_deadline.raw_value(), SchedDuration::Max().raw_value());
      EXPECT_EQ(iss.start_time.raw_value(), SchedTime{0}.raw_value());
      EXPECT_EQ(iss.finish_time.raw_value(), SchedTime{0}.raw_value());
      EXPECT_EQ(iss.time_slice_ns.raw_value(), SchedDuration{0}.raw_value());
    }

    // Next, block the deadline thread.  After this, the queue should have
    // inherited the blocked thread's utilization, as well as its dynamic
    // parameters since it is the only "consequential" thread blocked in the
    // queue.
    ASSERT_TRUE(deadline_thread.BlockOnOwnedWaitQueue(owq.get(), nullptr));
    {
      SingleChainLockGuard guard{IrqSaveOption, owq->get_lock(),
                                 CLT_TAG("pi_tests::bug_42182770_regression #2")};
      ASSERT_NONNULL(owq->const_inherited_scheduler_state_storage());

      const SchedDeadlineParams& params =
          static_cast<DeadlineProfile*>(deadline_profile.get())->sched_params();
      const SchedulerState::WaitQueueInheritedSchedulerState& iss =
          *owq->const_inherited_scheduler_state_storage();
      EXPECT_EQ(iss.ipvs.total_weight.raw_value(), inheritable_weight.raw_value());
      EXPECT_EQ(iss.ipvs.uncapped_utilization.raw_value(), params.utilization.raw_value());
      EXPECT_EQ(iss.ipvs.min_deadline.raw_value(), params.deadline_ns.raw_value());
      EXPECT_NE(iss.start_time.raw_value(), SchedTime{0}.raw_value());
      EXPECT_NE(iss.finish_time.raw_value(), SchedTime{0}.raw_value());
      EXPECT_NE(iss.time_slice_ns.raw_value(), SchedDuration{0}.raw_value());
    }

    // Wake the deadline thread from the owned wait queue.  We cannot 100% control
    // which thread the will be woken if we simply use WakeOne as the choice of
    // which thread to wake is part of the scheduler's logic, and not something we
    // can guarantee over time.
    //
    // Instead, we can force the thread we want to wake by sending it either a
    // suspend, or kill signal (in this case, we are going to send it the kill
    // signal).
    //
    // Once the thread has woken and exited, observe that the OWQ's IPVs have gone
    // back to no inherited weight or utilization.  If we have extra scheduler
    // invariant validation turned on, we should see the dynamic parameters get
    // reset as well (even though they are not normally reset as they are now
    // considered to be undefined.
    ASSERT_TRUE(deadline_thread.Reset(true));
    {
      SingleChainLockGuard guard{IrqSaveOption, owq->get_lock(),
                                 CLT_TAG("pi_tests::bug_42182770_regression #3")};
      ASSERT_NONNULL(owq->const_inherited_scheduler_state_storage());

      const SchedulerState::WaitQueueInheritedSchedulerState& iss =
          *owq->const_inherited_scheduler_state_storage();
      EXPECT_EQ(iss.ipvs.total_weight.raw_value(), inheritable_weight.raw_value());
      EXPECT_EQ(iss.ipvs.uncapped_utilization.raw_value(), SchedUtilization{0}.raw_value());
      EXPECT_EQ(iss.ipvs.min_deadline.raw_value(), SchedDuration::Max().raw_value());
      if constexpr (kSchedulerExtraInvariantValidation) {
        EXPECT_EQ(iss.start_time.raw_value(), SchedTime{0}.raw_value());
        EXPECT_EQ(iss.finish_time.raw_value(), SchedTime{0}.raw_value());
        EXPECT_EQ(iss.time_slice_ns.raw_value(), SchedDuration{0}.raw_value());
      }
    }

    // Finally, release the fair thread, confirm that the IPV storage no longer
    // exists in the OWQ, and shutdown the test.
    TestThread::ClearShutdownBarrier();
    owq->ReleaseAllThreads();
    ASSERT_TRUE(fair_thread.Reset());
    ASSERT_FALSE(owq->HasInheritedSchedulerStateStorage());
  }

  END_TEST;
}

bool pi_test_critical_active() {
  BEGIN_TEST;

  AutoProfileBooster pboost;

  const fbl::RefPtr<Profile> fair_profile =
      FairProfile::Create(TEST_DEFAULT_WEIGHT, InheritableProfile::Yes);
  const fbl::RefPtr<Profile> critical_profile =
      DeadlineProfile::Create(ZX_MSEC(2), ZX_MSEC(5), true);  // true for critical

  ASSERT_NONNULL(fair_profile);
  ASSERT_NONNULL(critical_profile);

  LockedOwnedWaitQueue owq;
  TestThread active_thread;
  TestThread pressure_thread;

  auto cleanup = fit::defer([&]() {
    TestThread::ClearShutdownBarrier();
    owq.ReleaseAllThreads();
    pressure_thread.Reset();
    active_thread.Reset();
  });

  TestThread::ResetShutdownBarrier();

  ASSERT_TRUE(active_thread.Create(fair_profile));
  ASSERT_TRUE(pressure_thread.Create(critical_profile));

  // Run the active thread in a soft-yield loop
  ktl::atomic<bool> stop_loop = false;
  ASSERT_TRUE(active_thread.RunOp([&stop_loop]() {
    while (!stop_loop.load()) {
      Thread::Current::Yield();
    }
  }));

  // Have the pressure thread block on the owq owned by active_thread.
  ASSERT_TRUE(pressure_thread.BlockOnOwnedWaitQueue(&owq, &active_thread));

  // Verify that active_thread inherited criticality.
  unittest::ThreadEffectiveProfileObserver observer;
  observer.Observe(active_thread.thread());
  ExpectedEffectiveProfile expected_profile;
  fair_profile->SetExpectedBaseProfile(expected_profile);
  critical_profile->AccumulateExpectedPressure(expected_profile);
  ASSERT_TRUE(observer.VerifyExpectedEffectiveProfile(expected_profile));

  // Release the pressure
  owq.ReleaseAllThreads();
  ASSERT_TRUE(pressure_thread.WaitFor<TestThread::Condition::WAITING_FOR_SHUTDOWN>());

  // Stop the active loop
  stop_loop = true;
  ASSERT_TRUE(active_thread.WaitFor<TestThread::Condition::WAITING_FOR_SHUTDOWN>());

  END_TEST;
}

template <size_t NumPressureThreads, typename VerifyFn>
  requires(ktl::invocable<VerifyFn, TestThread&, const ktl::array<TestThread, NumPressureThreads>&>)
bool pi_test_deadline_join_split_helper(const fbl::RefPtr<Profile>& active_profile,
                                        const fbl::RefPtr<Profile>& pressure_profile,
                                        zx_duration_mono_t spin_duration,
                                        zx_duration_mono_t retry_duration, VerifyFn verify_fn) {
  bool all_ok = true;

  const size_t kRetryWarningCount = 10000;
  size_t retry_count = 0;
  bool retry_test;

  const auto owq_count = [](LockedOwnedWaitQueue& owq) {
    SingleChainLockGuard guard{
        IrqSaveOption,
        owq.get_lock(),
        CLT_TAG("pi_tests::bandwidth_inheritance_poll"),
    };
    return owq.Count();
  };

  do {
    retry_test = false;
    ktl::atomic<bool> activation_count_changed{false};

    LockedOwnedWaitQueue owq1;
    LockedOwnedWaitQueue owq2;
    TestThread active_thread;
    ktl::array<TestThread, NumPressureThreads> pressure_threads;

    const auto print_state = [&active_thread, &pressure_threads](const char* tag) {
      const auto active_snapshot = active_thread.snapshot();
      printf("\n\t%-10s\t%-20s  %14s  %14s  %14s  %14s  %6s  %14s\n", tag, "THREAD", "REM_TS",
             "SLICE", "START", "FINISH", "ACT", "CURRENT");
      printf("\t%-10s\t%-20s  %14" PRId64 "  %14" PRId64 "  %14" PRId64 "  %14" PRId64 "  %6" PRIu64
             "  %14" PRId64 "\n",
             "", "active_thread", active_snapshot.remaining_time_slice_ns.raw_value(),
             active_snapshot.time_slice_ns.raw_value(), active_snapshot.start_time.raw_value(),
             active_snapshot.finish_time.raw_value(), active_snapshot.activation_count,
             active_snapshot.current_time.raw_value());
      for (size_t i = 0; i < NumPressureThreads; ++i) {
        const auto pressure_snapshot = pressure_threads[i].snapshot();
        char name[32];
        snprintf(name, sizeof(name), "pressure_thread%zu", i + 1);
        printf("\t%-10s\t%-20s  %14" PRId64 "  %14" PRId64 "  %14" PRId64 "  %14" PRId64
               "  %6" PRIu64 "  %14" PRId64 "\n",
               "", name, pressure_snapshot.remaining_time_slice_ns.raw_value(),
               pressure_snapshot.time_slice_ns.raw_value(),
               pressure_snapshot.start_time.raw_value(), pressure_snapshot.finish_time.raw_value(),
               pressure_snapshot.activation_count, pressure_snapshot.current_time.raw_value());
      }
    };

    auto cleanup = fit::defer([&]() {
      TestThread::ClearShutdownBarrier();
      owq1.ReleaseAllThreads();
      owq2.ReleaseAllThreads();
      for (auto& t : pressure_threads) {
        t.Reset();
      }
      active_thread.Reset();
    });

    TestThread::ResetShutdownBarrier();

    ASSERT_TRUE(active_thread.Create(active_profile));
    for (TestThread& t : pressure_threads) {
      ASSERT_TRUE(t.Create(pressure_profile));
    }

    // All of the threads have been created, bound to their respective profiles,
    // and are waiting to run their assigned operations. Pause the test for a
    // duration sufficient for all of the thread's periods to have expired, such
    // that each thread will receive a new activation. This makes it easier to
    // ensure the initial conditions are deterministic.
    const zx_duration_mono_t max_period =
        ktl::max(active_profile->Period(), pressure_profile->Period());
    Thread::Current::SleepRelative(max_period);

    // Sample the time after the threads have been created and their initial
    // activations have expired, but before the active thread starts executing.
    // This will be our baseline for measuring elapsed time.
    const zx_instant_mono_t start_time = current_mono_time();

    // Have the active thread block on owq1, then have the pressure threads block
    // on owq1.
    ASSERT_TRUE(active_thread.RunOp([&owq1, &owq2, &owq_count, &print_state, spin_duration,
                                     &activation_count_changed, &active_thread]() {
      print_state("start active");

      // Wait for all pressure threads to block on owq1.
      while (owq_count(owq1) != NumPressureThreads) {
        Thread::Current::SleepRelative(ZX_MSEC(1));
      }

      print_state("pre-spin");
      const uint64_t pre_spin_activation_count = active_thread.activation_count();

      // Spin for a measurable duration (i.e. the expected consumed capacity) to
      // ensure that the active thread has accumulated a significant portion of
      // its capacity before splitting from owq1.
      const zx_instant_mono_t spin_until = current_mono_time() + spin_duration;
      while (current_mono_time() < spin_until) {
        // Spin to consume time slice.
      }

      const uint64_t post_spin_activation_count = active_thread.activation_count();
      if (post_spin_activation_count != pre_spin_activation_count) {
        activation_count_changed.store(true);
      }

      print_state("post-spin");

      // Unblock the pressure threads, returning unused capacity.
      owq1.ReleaseAllThreads();

      print_state("post-release");

      // Block on owq2 to freeze state for observation.
      AnnotatedAutoEagerReschedDisabler eager_resched_disabler;
      owq2.BlockAndAssignOwner(Deadline::infinite(), nullptr, ResourceOwnership::Normal,
                               Interruptible::Yes);
    }));

    // Custom operation for pressure threads: block on owq1, then owq2.
    auto pressure_op = [&owq1, &owq2, &active_thread, &print_state]() {
      print_state("start pressure");

      AnnotatedAutoEagerReschedDisabler eager_resched_disabler;
      owq1.BlockAndAssignOwner(Deadline::infinite(), &active_thread.thread(),
                               ResourceOwnership::Normal, Interruptible::Yes);
      owq2.BlockAndAssignOwner(Deadline::infinite(), nullptr, ResourceOwnership::Normal,
                               Interruptible::Yes);
    };

    for (auto& t : pressure_threads) {
      ASSERT_TRUE(t.RunOp(pressure_op));
    }

    // Wait for all threads to block on owq2.
    while (owq_count(owq2) != NumPressureThreads + 1) {
      Thread::Current::SleepRelative(ZX_MSEC(5));
    }

    // Sample the time after all threads block on owq2. This will be our
    // end time for measuring elapsed time.
    const zx_instant_mono_t end_time = current_mono_time();

    print_state("end");

    // If the total elapsed time is too long, or the activation count changed,
    // assume VM time loss occurred and retry.
    if (end_time - start_time > retry_duration || activation_count_changed.load()) {
      retry_test = true;
    } else {
      all_ok = verify_fn(active_thread, pressure_threads) && all_ok;
    }

    owq2.ReleaseAllThreads();
    for (auto& t : pressure_threads) {
      ASSERT_TRUE(t.template WaitFor<TestThread::Condition::WAITING_FOR_SHUTDOWN>());
    }

    ++retry_count;
    EXPECT_NE(retry_count, kRetryWarningCount, "Reached retry limit due to suspected time loss");
  } while (retry_test);

  return all_ok;
}

// Tests bandwidth distribution when a deadline thread blocks as upstream node
// behind a fair target thread. This results in a deadline-to-fair PI graph join
// and a subsequent deadline-from-fair PI graph split.
bool pi_test_deadline_to_fair_join_split() {
  BEGIN_TEST;

  const zx_duration_mono_t period = ZX_SEC(1);
  const zx_duration_mono_t capacity = period;
  const zx_duration_mono_t spin_duration = capacity / 4;
  const zx_duration_mono_t max_expected_remaining_capacity = capacity - spin_duration;
  const zx_duration_mono_t retry_duration = period * 3;

  const fbl::RefPtr<Profile> fair_profile =
      FairProfile::Create(TEST_DEFAULT_WEIGHT, InheritableProfile::Yes);
  const fbl::RefPtr<Profile> deadline_profile = DeadlineProfile::Create(capacity, period);

  ASSERT_NONNULL(fair_profile);
  ASSERT_NONNULL(deadline_profile);

  auto verify_fn = [&all_ok, max_expected_remaining_capacity](
                       const TestThread& active_thread,
                       const ktl::array<TestThread, 1>& pressure_threads) -> bool {
    // Verify that pressure_thread's parameters were updated.
    const SchedDuration remaining_time_slice = pressure_threads[0].remaining_time_slice();

    // If the active thread consumed approximately the expected capacity, the
    // remaining time slice should be greater than 0ns and less than the
    // expected remaining capacity (accounting for scheduling overhead).
    EXPECT_GT(remaining_time_slice.raw_value(), 0);
    EXPECT_LT(remaining_time_slice.raw_value(), max_expected_remaining_capacity);
    return all_ok;
  };

  ASSERT_TRUE(pi_test_deadline_join_split_helper<1>(fair_profile, deadline_profile, spin_duration,
                                                    retry_duration, verify_fn));

  END_TEST;
}

// Tests bandwidth distribution when a deadline thread blocks as upstream node
// behind another deadline target thread. This results in a deadline-to-deadline
// PI graph join and a subsequent deadline-from-deadline PI graph split.
bool pi_test_deadline_to_deadline_join_split() {
  BEGIN_TEST;

  const zx_duration_mono_t period = ZX_SEC(1);
  const zx_duration_mono_t capacity = period / 2;
  const zx_duration_mono_t spin_duration = capacity;
  const zx_duration_mono_t max_expected_remaining_capacity = (period - spin_duration) / 2;
  const zx_duration_mono_t retry_duration = period * 3;

  const fbl::RefPtr<Profile> deadline_profile = DeadlineProfile::Create(capacity, period);

  ASSERT_NONNULL(deadline_profile);

  auto verify_fn = [&all_ok, max_expected_remaining_capacity](
                       const TestThread& active_thread,
                       const ktl::array<TestThread, 1>& pressure_threads) -> bool {
    // Verify that the active and pressure thread's parameters were updated.
    const SchedDuration active_remaining_time_slice = active_thread.remaining_time_slice();
    const SchedDuration pressure_remaining_time_slice = pressure_threads[0].remaining_time_slice();

    // If the active thread consumed approximately the expected capacity, the
    // remaining time slice should be greater than 0ns and less than the
    // expected remaining capacity (accounting for scheduling overhead).
    EXPECT_GT(active_remaining_time_slice.raw_value(), 0);
    EXPECT_LT(active_remaining_time_slice.raw_value(), max_expected_remaining_capacity);
    EXPECT_GT(pressure_remaining_time_slice.raw_value(), 0);
    EXPECT_LT(pressure_remaining_time_slice.raw_value(), max_expected_remaining_capacity);
    return all_ok;
  };

  ASSERT_TRUE(pi_test_deadline_join_split_helper<1>(deadline_profile, deadline_profile,
                                                    spin_duration, retry_duration, verify_fn));

  END_TEST;
}

// Tests bandwidth distribution when a deadline thread blocks as upstream node
// behind another deadline target thread. This results in a deadline-to-deadline
// PI graph join and a subsequent deadline-from-deadline PI graph split, where the
// active thread has a smaller capacity than the pressure thread.
bool pi_test_deadline_to_deadline_join_split_uneven() {
  BEGIN_TEST;

  const zx_duration_mono_t period = ZX_SEC(1);
  const zx_duration_mono_t active_capacity = period / 4;
  const zx_duration_mono_t pressure_capacity = period * 3 / 4;
  const zx_duration_mono_t spin_duration = period / 2;
  const zx_duration_mono_t max_expected_active_remaining_capacity = (period - spin_duration) / 4;
  const zx_duration_mono_t max_expected_pressure_remaining_capacity =
      (period - spin_duration) * 3 / 4;
  const zx_duration_mono_t retry_duration = period * 3;

  const fbl::RefPtr<Profile> active_profile = DeadlineProfile::Create(active_capacity, period);
  const fbl::RefPtr<Profile> pressure_profile = DeadlineProfile::Create(pressure_capacity, period);

  ASSERT_NONNULL(active_profile);
  ASSERT_NONNULL(pressure_profile);

  auto verify_fn = [&all_ok, max_expected_active_remaining_capacity,
                    max_expected_pressure_remaining_capacity](
                       const TestThread& active_thread,
                       const ktl::array<TestThread, 1>& pressure_threads) -> bool {
    // Verify that the active and pressure thread's parameters were updated.
    const SchedDuration active_remaining_time_slice = active_thread.remaining_time_slice();
    const SchedDuration pressure_remaining_time_slice = pressure_threads[0].remaining_time_slice();

    // If the active thread consumed approximately the expected capacity, the
    // remaining time slice should be greater than 0ns and less than the
    // expected remaining capacity (accounting for scheduling overhead).
    EXPECT_GT(active_remaining_time_slice.raw_value(), 0);
    EXPECT_LT(active_remaining_time_slice.raw_value(), max_expected_active_remaining_capacity);
    EXPECT_GT(pressure_remaining_time_slice.raw_value(), 0);
    EXPECT_LT(pressure_remaining_time_slice.raw_value(), max_expected_pressure_remaining_capacity);
    return all_ok;
  };

  ASSERT_TRUE(pi_test_deadline_join_split_helper<1>(active_profile, pressure_profile, spin_duration,
                                                    retry_duration, verify_fn));

  END_TEST;
}

// Tests bandwidth distribution when multiple deadline threads block as upstream
// nodes behind a deadline thread. This results in multiple deadline-to-deadline
// PI graph joins and subsequent deadline-from-deadline PI graph splits, with
// multiple threads blocking on the same waitqueue.
bool pi_test_deadline_to_deadline_join_split_multiple_waiters() {
  BEGIN_TEST;

  const zx_duration_mono_t period = ZX_SEC(1);
  const zx_duration_mono_t active_capacity = period / 2;
  const zx_duration_mono_t pressure_capacity = period / 4;
  const zx_duration_mono_t spin_duration = period / 2;
  const zx_duration_mono_t max_expected_active_remaining_capacity = (period - spin_duration) / 2;
  const zx_duration_mono_t max_expected_pressure_remaining_capacity = (period - spin_duration) / 4;
  const zx_duration_mono_t retry_duration = period * 3;

  const fbl::RefPtr<Profile> active_profile = DeadlineProfile::Create(active_capacity, period);
  const fbl::RefPtr<Profile> pressure_profile = DeadlineProfile::Create(pressure_capacity, period);

  ASSERT_NONNULL(active_profile);
  ASSERT_NONNULL(pressure_profile);

  auto verify_fn = [&all_ok, max_expected_active_remaining_capacity,
                    max_expected_pressure_remaining_capacity](
                       const TestThread& active_thread,
                       const ktl::array<TestThread, 2>& pressure_threads) -> bool {
    // Verify that the active and pressure thread's parameters were updated.
    const SchedDuration active_remaining_time_slice = active_thread.remaining_time_slice();
    const SchedDuration pressure1_remaining_time_slice = pressure_threads[0].remaining_time_slice();
    const SchedDuration pressure2_remaining_time_slice = pressure_threads[1].remaining_time_slice();

    // If the active thread consumed approximately the expected capacity, the
    // remaining time slice should be greater than 0ns and less than the
    // expected remaining capacity (accounting for scheduling overhead).
    EXPECT_GT(active_remaining_time_slice.raw_value(), 0);
    EXPECT_LT(active_remaining_time_slice.raw_value(), max_expected_active_remaining_capacity);
    EXPECT_GT(pressure1_remaining_time_slice.raw_value(), 0);
    EXPECT_LT(pressure1_remaining_time_slice.raw_value(), max_expected_pressure_remaining_capacity);
    EXPECT_GT(pressure2_remaining_time_slice.raw_value(), 0);
    EXPECT_LT(pressure2_remaining_time_slice.raw_value(), max_expected_pressure_remaining_capacity);
    return all_ok;
  };

  ASSERT_TRUE(pi_test_deadline_join_split_helper<2>(active_profile, pressure_profile, spin_duration,
                                                    retry_duration, verify_fn));

  END_TEST;
}

}  // namespace

UNITTEST_START_TESTCASE(pi_tests)
UNITTEST("basic", pi_test_basic)
UNITTEST("critical active", pi_test_critical_active)
UNITTEST("changing priority", pi_test_changing_priority)
UNITTEST("changing priority in WaitQueue", pi_test_changing_priority_in_wait_queue)
UNITTEST("chains", pi_test_chain)
UNITTEST("multiple waiters", pi_test_multi_waiter)
UNITTEST("multiple owned queues", pi_test_multi_owned_queues)
UNITTEST("cycles (inheritable)", pi_test_cycle<InheritableProfile::Yes>)
UNITTEST("cycles (non-inheritable)", pi_test_cycle<InheritableProfile::No>)
UNITTEST("b/42182770 regression test", bug_42182770_regression)
UNITTEST("deadline to fair join and split", pi_test_deadline_to_fair_join_split)
UNITTEST("deadline to deadline join and split", pi_test_deadline_to_deadline_join_split)
UNITTEST("deadline to deadline join and split uneven",
         pi_test_deadline_to_deadline_join_split_uneven)
UNITTEST("deadline to deadline join and split multiple waiters",
         pi_test_deadline_to_deadline_join_split_multiple_waiters)
UNITTEST_END_TESTCASE(pi_tests, "pi", "Priority inheritance tests for OwnedWaitQueues")
