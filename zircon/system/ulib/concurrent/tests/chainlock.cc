// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/concurrent/capability_token.h>
#include <lib/concurrent/chainlock.h>
#include <lib/concurrent/chainlock_guard.h>
#include <lib/concurrent/chainlock_transaction.h>
#include <lib/concurrent/chainlock_transaction_common.h>
#include <lib/concurrent/chainlockable.h>
#include <zircon/assert.h>
#include <zircon/compiler.h>
#include <zircon/time.h>

#include <atomic>
#include <chrono>
#include <condition_variable>
#include <functional>
#include <memory>
#include <mutex>
#include <thread>

#include <zxtest/zxtest.h>

// Provide an empty implementation of the CallsiteTraits that will allow the tests to compile
// but doesn't actually store any debugging/tracing data.
namespace {
class ChainLockTransaction;
}

template <>
struct concurrent::CallsiteTraits<ChainLockTransaction> {
  struct CallsiteInfo {};
};

namespace {

using namespace std::chrono_literals;
using ::concurrent::chainlock_transaction_token;

constexpr bool DebugEnabled = true;
constexpr bool TracingEnabled = true;

class ChainLockTransaction
    : public ::concurrent::ChainLockTransaction<ChainLockTransaction, DebugEnabled,
                                                TracingEnabled> {
 public:
  static ChainLockTransaction* Active() { return active_transaction_; }
  static zx_instant_mono_ticks_t GetCurrentTicks() {
    return std::chrono::steady_clock::now().time_since_epoch().count();
  }
  static void Yield() {}

  using ContentionState = std::atomic<uint64_t>;
  using CallsiteInfo = concurrent::CallsiteTraits<ChainLockTransaction>::CallsiteInfo;

  enum class Options {
    None,
    A,
    B,
  };

  static constexpr Option<Options::None> OptionNone{};
  static constexpr Option<ChainLockTransaction::Options::A> OptionA{};
  static constexpr Option<ChainLockTransaction::Options::B> OptionB{};

  [[maybe_unused]] static constexpr concurrent::CapabilityToken<> option_a_cap;
  [[maybe_unused]] static constexpr concurrent::CapabilityToken<> option_b_cap;

  static Options ActiveOption() { return active_option_; }

  ~ChainLockTransaction() __TA_RELEASE(chainlock_transaction_token) = default;

  static zx_instant_mono_ticks_t last_contention_start_ticks() {
    return last_contention_start_ticks_;
  }
  static CallsiteInfo last_callsite_info() { return last_callsite_info_; }

 private:
  using Base =
      ::concurrent::ChainLockTransaction<ChainLockTransaction, DebugEnabled, TracingEnabled>;

  template <Options>
  friend class SingletonChainLockGuard;
  friend Base;

  explicit ChainLockTransaction(CallsiteInfo callsite_info)
      __TA_ACQUIRE(chainlock_transaction_token)
      : Base{callsite_info} {}

  ChainLockTransaction(CallsiteInfo callsite_info, uint32_t locks_held, bool finalized)
      : Base{callsite_info, locks_held, finalized} {}

  void OnBackoff() {}
  bool RecordBackoffOrRetry(Token observed_token, std::atomic<Token>& lock_state,
                            ContentionState& contention_state) {
    return true;
  }

  template <typename Callable>
  void ReleaseAndNotify(ContentionState& contention_state, Callable&& do_release) {
    std::invoke(do_release);
  }

  static void SetActive(ChainLockTransaction* transaction) { active_transaction_ = transaction; }

  static Token AllocateReservedToken() {
    const size_t reserved_index = reserved_token_allocator_.fetch_add(1);
    ZX_ASSERT(reserved_index <= concurrent::ChainLockTransactionCommon::kReservedTokenValues);
    return ReservedToken(reserved_index);
  }

  static void OnFinalized(zx_instant_mono_t contention_start_ticks, CallsiteInfo callsite_info) {
    last_contention_start_ticks_ = contention_start_ticks;
    last_callsite_info_ = callsite_info;
  }

  template <Options>
  struct StateSaver {
    StateSaver() {}
    ~StateSaver() {}

    void Relax(ChainLockTransaction&) { std::this_thread::yield(); }
  };

  template <>
  struct StateSaver<Options::A> {
    StateSaver() __TA_ACQUIRE(option_a_cap) { active_option_ = Options::A; }
    ~StateSaver() __TA_RELEASE(option_a_cap) { active_option_ = Options::None; }

    void Relax(ChainLockTransaction&) { std::this_thread::yield(); }
  };

  template <>
  struct StateSaver<Options::B> {
    StateSaver() __TA_ACQUIRE(option_b_cap) { active_option_ = Options::B; }
    ~StateSaver() __TA_RELEASE(option_b_cap) { active_option_ = Options::None; }

    void Relax(ChainLockTransaction&) { std::this_thread::yield(); }
  };

  inline static thread_local zx_instant_mono_t last_contention_start_ticks_{0};
  inline static thread_local CallsiteInfo last_callsite_info_;

  inline static std::atomic<size_t> reserved_token_allocator_{0};

  inline static thread_local Options active_option_{Options::None};
  inline static thread_local ChainLockTransaction* active_transaction_{nullptr};
};

using ChainLock = ::concurrent::ChainLock<ChainLockTransaction>;
using ChainLockable = ::concurrent::ChainLockable<ChainLockTransaction>;
using ChainLockGuard = ::concurrent::ChainLockGuard<ChainLockTransaction>;

#define CLT_TAG(tag) \
  ChainLockTransaction::CallsiteInfo {}

template <typename State>
class HelperThread {
 public:
  template <typename Callable>
  explicit HelperThread(Callable&& callable)
      : helper_thread_{std::forward<Callable>(callable), Helper{this}} {}

  ~HelperThread() { helper_thread_.join(); }

  HelperThread(const HelperThread&) = delete;
  HelperThread& operator=(const HelperThread&) = delete;

  void SetStateAndNotifyHelper(State state) {
    {
      std::unique_lock guard{lock_};
      state_ = state;
    }
    helper_condition_.notify_one();
  }

  void WaitForHelperState(State state) {
    std::unique_lock guard{lock_};
    if (state_ != state) {
      test_condition_.wait(guard, [&] { return state_ == state; });
    }
  }

  State GetState() const {
    std::unique_lock guard{lock_};
    return state_;
  }

  class Helper {
   public:
    void SetStateAndNotifyTest(State state) {
      {
        std::unique_lock guard{self_.lock_};
        self_.state_ = state;
      }
      self_.test_condition_.notify_one();
    }

    void WaitForTestState(State state) {
      std::unique_lock guard{self_.lock_};
      if (self_.state_ != state) {
        self_.helper_condition_.wait(guard, [&] { return self_.state_ == state; });
      }
    }

    State GetState() const { return self_.GetState(); }

   private:
    friend HelperThread;
    explicit Helper(HelperThread* self) : self_{*self} {}

    HelperThread& self_;
  };

 private:
  std::condition_variable helper_condition_;
  std::condition_variable test_condition_;
  mutable std::mutex lock_;
  State state_{State::Initial};
  std::thread helper_thread_;
};

TEST(ChainLock, TransactionOptions) {
  EXPECT_EQ(ChainLockTransaction::Options::None, ChainLockTransaction::ActiveOption());

  ChainLockTransaction::UntilDone(
      ChainLockTransaction::OptionNone, CLT_TAG("TransactionOptions"),
      []() __TA_EXCLUDES(ChainLockTransaction::option_a_cap,
                         ChainLockTransaction::option_b_cap) -> ChainLockTransaction::Result<> {
        EXPECT_EQ(ChainLockTransaction::Options::None, ChainLockTransaction::ActiveOption());
        return ChainLockTransaction::Done;
      });

  EXPECT_EQ(ChainLockTransaction::Options::None, ChainLockTransaction::ActiveOption());

  ChainLockTransaction::UntilDone(
      ChainLockTransaction::OptionA, CLT_TAG("TransactionOptions"),
      []() __TA_REQUIRES(ChainLockTransaction::option_a_cap)
          __TA_EXCLUDES(ChainLockTransaction::option_b_cap) -> ChainLockTransaction::Result<> {
            EXPECT_EQ(ChainLockTransaction::Options::A, ChainLockTransaction::ActiveOption());
            return ChainLockTransaction::Done;
          });

  EXPECT_EQ(ChainLockTransaction::Options::None, ChainLockTransaction::ActiveOption());

  ChainLockTransaction::UntilDone(
      ChainLockTransaction::OptionB, CLT_TAG("TransactionOptions"),
      []() __TA_REQUIRES(ChainLockTransaction::option_b_cap)
          __TA_EXCLUDES(ChainLockTransaction::option_a_cap) -> ChainLockTransaction::Result<> {
            EXPECT_EQ(ChainLockTransaction::Options::B, ChainLockTransaction::ActiveOption());
            return ChainLockTransaction::Done;
          });

  EXPECT_EQ(ChainLockTransaction::Options::None, ChainLockTransaction::ActiveOption());
}

TEST(ChainLock, TransactionReturnValue) {
  std::unique_ptr<int> value = std::make_unique<int>(42);
  const int* value_pointer = value.get();

  std::unique_ptr<int> return_value = ChainLockTransaction::UntilDone(
      ChainLockTransaction::OptionNone, CLT_TAG("TransactionOptions"),
      [&value]() -> ChainLockTransaction::Result<std::unique_ptr<int>> {
        return std::move(value);
      });

  EXPECT_EQ(return_value.get(), value_pointer);
  EXPECT_EQ(nullptr, value.get());
}

TEST(ChainLock, UncontestedAcquire) {
  ChainLock lock;

  ChainLockTransaction::UntilDone(
      ChainLockTransaction::OptionNone, CLT_TAG("UncontestedAcquire"),
      [&]() __TA_REQUIRES(chainlock_transaction_token) -> ChainLockTransaction::Result<> {
        EXPECT_FALSE(lock.is_held());

        lock.AcquireFirstInChain();

        EXPECT_TRUE(lock.is_held());
        EXPECT_EQ(lock.state_for_testing(), ChainLockTransaction::ActiveRef().token());

        lock.Release();

        EXPECT_FALSE(lock.is_held());
        return ChainLockTransaction::Done;
      });
}

TEST(ChainLock, CyclicAcquire) {
  ChainLock lock;

  // Return the lock without the returns capability annotation so TA won't reject the attempt to
  // acquire the lock more than once in this test.
  const auto get_lock = [&]() -> ChainLock& { return lock; };

  ChainLockTransaction::UntilDone(
      ChainLockTransaction::OptionNone, CLT_TAG("CyclicAcquire"),
      [&]() __TA_REQUIRES(chainlock_transaction_token) -> ChainLockTransaction::Result<> {
        lock.AcquireFirstInChain();

        ChainLock::Result result;
        EXPECT_FALSE(get_lock().AcquireOrResult(result));
        EXPECT_EQ(ChainLock::Result::Cycle, result);

        lock.Release();
        return ChainLockTransaction::Done;
      });
}

TEST(ChainLock, ContentionPriority) {
  ChainLock lock_a;
  ChainLock lock_b;

  enum class State {
    Initial,
    HelperTransacionStarted,
    TestTransactionStarted,
  };

  HelperThread<State> helper_thread{[&](auto helper) {
    ChainLockTransaction::UntilDone(
        ChainLockTransaction::OptionNone, CLT_TAG("ContentionPriority helper"),
        [&]() __TA_REQUIRES(chainlock_transaction_token) -> ChainLockTransaction::Result<> {
          ChainLockGuard guard{lock_a};

          helper.SetStateAndNotifyTest(State::HelperTransacionStarted);
          helper.WaitForTestState(State::TestTransactionStarted);

          // Acquiring lock_b should spin until the lower priority transaction backs off.
          if (const bool acquired = lock_b.AcquireOrBackoff(); acquired) {
            lock_b.Release();
          } else {
            EXPECT_TRUE(acquired);
          }

          return ChainLockTransaction::Done;
        });
  }};

  // Wait for the helper thread to start a transaction so it has higher priority.
  helper_thread.WaitForHelperState(State::HelperTransacionStarted);

  ChainLockTransaction::UntilDone(
      ChainLockTransaction::OptionNone, CLT_TAG("ContentionPriority"),
      [&]() __TA_REQUIRES(chainlock_transaction_token) -> ChainLockTransaction::Result<> {
        ChainLockGuard guard{lock_b};

        helper_thread.SetStateAndNotifyHelper(State::TestTransactionStarted);

        // The helper transaction should be higher priority than this transaction.
        EXPECT_GT(lock_b.state_for_testing(), lock_a.state_for_testing());

        // Let the helper thread spin for a while waiting for lock_b to be released.
        std::this_thread::sleep_for(1s);

        if (!lock_a.AcquireOrBackoff()) {
          return ChainLockTransaction::Action::Backoff;
        }
        lock_a.Release();

        return ChainLockTransaction::Done;
      });
}

// Test TA annotations for cross-referencing types using ChainLockable to avoid incomplete type
// errors.
class TypeB;
class TypeA : public ChainLockable {
 public:
  explicit TypeA(int value) : value_{value} {}

  // ChainLockable::GetLock() may be used to get the lock capability for incomplete types that are
  // eventually defined to derive from ChainLockable.
  bool compare(const TypeB& other) const __TA_REQUIRES(get_lock(), ChainLockable::GetLock(other));

 private:
  friend class TypeB;
  __TA_GUARDED(get_lock()) int value_;
};

class TypeB : public ChainLockable {
 public:
  explicit TypeB(int value) : value_{value} {}

  bool compare(const TypeA& other) const __TA_REQUIRES(get_lock(), other.get_lock()) {
    return value_ == other.value_;
  }

 private:
  friend class TypeA;
  __TA_GUARDED(get_lock()) int value_;
};

// Must be defined here since it depends on the complete definition of TypeB.
inline bool TypeA::compare(const TypeB& other) const { return value_ == other.value_; }

TEST(ChainLock, CrossReferencingTypes) {
  TypeA value_a{0};
  TypeB value_b{0};

  const auto do_transaction =
      [&]() __TA_REQUIRES(chainlock_transaction_token) -> ChainLockTransaction::Result<> {
    ChainLockGuard guard_a{value_a.get_lock()};
    ChainLockGuard guard_b{value_b.get_lock(), ChainLockGuard::Defer};
    if (!guard_b.AcquireOrBackoff()) {
      return ChainLockTransaction::Action::Backoff;
    }
    EXPECT_TRUE(value_a.compare(value_b));
    return ChainLockTransaction::Done;
  };
  ChainLockTransaction::UntilDone(ChainLockTransaction::OptionNone,
                                  CLT_TAG("CrossReferencingTypes"), do_transaction);
}

}  // anonymous namespace
