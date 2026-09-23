// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_CONCURRENT_CHAINLOCK_TRANSACTION_H_
#define LIB_CONCURRENT_CHAINLOCK_TRANSACTION_H_

#include <lib/concurrent/chainlock_transaction_common.h>
#include <zircon/assert.h>
#include <zircon/compiler.h>
#include <zircon/time.h>

#include <atomic>
#include <type_traits>

//
// # General Chain Lock Transaction Facility
//
// Chain lock transactions are a layer on top of chain locks that track the state necessary for
// entities (i.e. CPUs) to acquire a series of one or more chain locks and correctly handle
// arbitration with other entities contending for the same locks. The base transaction facility
// provides the main hooks required by the general chain lock implementation, and has compile-time
// enabled state for debugging and performance tracing to help diagnose issues with chain lock
// usage.
//

namespace concurrent {

// A trait type that all derived classes of ChainLockTransaction must implement.
//
// Concretely, all derived classes must provide a nested definition for the CallsiteInfo type, which
// should provide debugging/tracing information that identifies the code location of the
// transaction.
//
// Naively, one would expect this to be a nested type within the Derived class of
// ChainLockTransaction. However, this causes compilation failures, as CallsiteInfo is a parameter
// for several of the methods in the base class, and making it a nested type results in an
// incomplete definition error when inheriting from the base class. Thus, we need a separate traits
// type that works around this problem.
template <typename T>
struct CallsiteTraits;

// CRTP type that provides the basic environment-independent framework for chainlock transactions.
//
// Environment-specific chain lock transaction types should subclass the ChainLockTransaction CRTP
// type and must provide the following definitions to adapt to the host environment:
//
// 1. A nested StateSaver template type to handle environment-specific state save, restore, and
//    relax operations.
//
//    Example:
//
//    enum class StateType { A = 0, B = 1 };
//
//    template <StateType state_type>
//    struct StateSaver;
//
//    template <>
//    struct StateSaver<StateType::A> {
//      StateSaver() : state_{SaveStateTypeA()} {}
//      ~StateSaver() { RestoreStateTypeA(state_); }
//
//      void Relax(ChainLockTransaction& transaction) {
//        transaction.Unregister();
//        RestoreStateTypeA(state_); // Environment-specific.
//        transaction.WaitForConflictResolution(); // Environment-specific.
//        state_ = SaveStateTypeA(); // Environment-specific.
//        transaction.Register();
//      }
//
//      StateTypeA state_;
//    };
//
// TODO(eieio): Complete explanation of derived type requirements.
//
template <typename Derived, bool DebugAccountingEnabledFlag = false,
          bool ContentionTracingEnabledFlag = false>
class ChainLockTransaction : public ChainLockTransactionCommon {
  struct AnyResult {
    template <typename T>
    AnyResult(Result<T>) {}
    AnyResult(DoneType) {}
  };
  template <typename Callable>
  using EnableIfTransactionHandler = std::enable_if_t<std::is_invocable_r_v<AnyResult, Callable>>;
  using CallsiteInfo = CallsiteTraits<Derived>::CallsiteInfo;

  template <typename T>
  static Result<T> ToResult(Result<T> result) {
    return result;
  }
  static Result<> ToResult(DoneType) { return DoneType{}; }

 public:
  static constexpr bool DebugAccountingEnabled = DebugAccountingEnabledFlag;
  static constexpr bool ContentionTracingEnabled = ContentionTracingEnabledFlag;

  template <auto option>
  struct Option {};

  // UntilDone provides a common sequence for executing a chain lock transaction and the required
  // backoff arbitration logic.
  //
  // The caller is responsble for providing three arguments:
  // 1. An instance of Option<constant>: Used to pass a compile-time constant to the environment-
  //    provided StateSaver<constant> type, indicating the relevant states to save, such as IRQ
  //    enable flags, preemption flags, etc..., and restore after each transaction attempt.
  // 2. An instance of CallsiteTraits<Derived>::CallsiteInfo: Used for debugging and tracing to
  //    identify the code location of the transaction.
  // 3. A callable implementing a single transaction attempt: This can be a function, lambda, or
  //    functor with an appropriate signature and static annotations.
  //
  // The callable should be invocable matching one of the following signatures:
  // a. ChainLockTransactionCommon::Result<>() : A transaction that has a return type void.
  // c. ChainLockTransactionCommon::Result<T>(): A transaction that has user defined return type T.
  // b. ChainLockTransactionCommon::DoneType() : A transaction that has an implied return type void.
  //
  // The overall return type of UntilDone is either T or void, depending on signature of Callable,
  // as described above.  Note that Result<>, Result<T>, and DoneType are transitively inherited
  // from ChainLockTransactionCommon through this ChainLockTransaction CRTP type and may be referred
  // to using the environment-specific transaction type.
  //
  // A basic transaction is structured as follows:
  //
  // 1. Define a callable that will be executed until the transaction is complete. In this example,
  // the callable will return zx_status_t when the transaction is complete, which will become the
  // return value of UntilDone.
  //
  // auto do_transaction = [&]() __TA_REQUIRES(chainlock_transaction_token, <env-specific tokens>)
  //                                                  -> ChainLockTransaction::Result<zx_status_t> {
  //   if (!object->get_lock().AcquireOrBackoff()) {
  //     return ChainLockTransaction::Action::Backoff; // Execute the backoff protocol and re-enter.
  //   }
  //
  //   if (restart_transaciton_condition) {
  //     object->get_lock().Release();
  //     return ChainLockTransaction::Action::Restart; // Restart the transaction and re-enter.
  //   }
  //
  //   if (!LockedOperationOnObject(object)) {
  //     object->get_lock().Release();
  //     return ZX_ERR_BAD_STATE;  // Complete the transaction, returning the error to the caller.
  //   }
  //
  //   object->get_lock().Release();
  //   return ZX_OK;  // Complete the transaction, returning success to the caller.
  // };
  //
  // In cases where the return user return type is void, the callable can return
  // ChainLockTransaction::Done to complete the transaction.
  //
  // 2. Execute the transaction using the given options and callable. In this example, the constant
  // kStateOptionA is a value of type Option<auto option> with an environment-specific option value
  // to pass to the state saver. The callable is invoked cyclically, with intervening backoffs,
  // until a terminal value (of a user defined return type in this case) is returned.
  //
  //  return ChainLockTransaction::UntilDone(kStateOptionA, CLT_CALLSITE("foo"), do_transaction);
  //
  template <auto option, typename Callable, typename = EnableIfTransactionHandler<Callable>>
  static auto UntilDone(Option<option>, CallsiteInfo callsite_info, Callable&& callable) {
    // Instantiate the state saver type with the supplied option to take care of state save,
    // restore, and relax operations.
    typename Derived::template StateSaver<option> state_saver{};

    // Instantiate the derived transaction type to store the transaction state. This will register
    // the transaction as the active transaction.
    Derived transaction{callsite_info};

    // Deduce a CV qualified reference to the passed-in callable, since it may be called more than
    // once if the transaction must back off or restart.
    decltype(auto) callable_reference = std::forward<Callable>(callable);

    // Execute the callable until it returns a terminal value indicating the transaction is
    // complete.
    while (true) {
      // Execute the transaction and get the returned result.
      Result result = ToResult(callable_reference());

      // If the result is not an action it is a terminal value. Complete the transaction and return
      // the user defined value, if any.
      if (!result.is_action()) {
        if (transaction.state() == State::Started) {
          transaction.Finalize();
        }
        transaction.SetState(State::Complete);
        return result.take_value();
      }

      // If the result is an action, take the either back off or restart the transaction.
      switch (result.action()) {
        case Action::Backoff:
          ZX_DEBUG_ASSERT(transaction.state() == State::Started);
          transaction.AssertNumLocksHeld(0);
          transaction.AssertNotFinalized();
          ++transaction.backoff_count_;
          transaction.OnBackoff();  // Notify after updating backoff_count_.
          state_saver.Relax(transaction);
          break;

        case Action::Restart:
          ZX_DEBUG_ASSERT(transaction.state() == State::Finalized);
          transaction.Restart(callsite_info);
          break;
      }
    }
  }

  uint64_t backoff_count() const { return backoff_count_; }

  // Debug asserts for various transaction states.
  void AssertAtLeastOneLockHeld() const {
    if constexpr (DebugAccountingEnabled) {
      ZX_ASSERT(accounting_.locks_held() > 0);
    }
  }
  void AssertAtMostOneLockHeld() const {
    if constexpr (DebugAccountingEnabled) {
      ZX_ASSERT(accounting_.locks_held() <= 1);
    }
  }
  void AssertNumLocksHeld(uint32_t expected) const {
    if constexpr (DebugAccountingEnabled) {
      ZX_ASSERT_MSG(expected == accounting_.locks_held(), "expected=%u, actual=%u", expected,
                    accounting_.locks_held());
    }
  }
  void AssertFinalized() const {
    if constexpr (DebugAccountingEnabled) {
      ZX_ASSERT(accounting_.finalized() == true);
    }
  }
  void AssertNotFinalized() const {
    if constexpr (DebugAccountingEnabled) {
      ZX_ASSERT(accounting_.finalized() == false);
    }
  }

  // Assert that a transaction is active to static analysis.
  static void AssertActive() __TA_ASSERT(chainlock_transaction_token) {
    ZX_DEBUG_ASSERT(nullptr != Derived::Active());
  }

  // Assert that a transaction is active to static analysis without actually checking at runtime for
  // efficiency in trivial cases.
  static void MarkActive() __TA_ASSERT(chainlock_transaction_token) {}

  // Returns a reference to the active transaction. Asserts that a transaction is active.
  static Derived& ActiveRef() __TA_REQUIRES(chainlock_transaction_token) {
    Derived* transaction = Derived::Active();
    ZX_DEBUG_ASSERT(transaction != nullptr);
    return *transaction;
  }

  // Saves the pointer to the current transaction and allocates a reserved token to sync leaf chain
  // locks to during critical sequences.
  static SavedState SaveStateAndUseReservedToken() __TA_REQUIRES(chainlock_transaction_token) {
    ChainLockTransaction* transaction = Derived::Active();
    ZX_DEBUG_ASSERT(transaction != nullptr);
    ZX_DEBUG_ASSERT(!transaction->token().is_reserved());
    SavedState state{transaction, Derived::AllocateReservedToken()};
    return state;
  }

  // Create a save state for a bare transaction used to kick off a new thread.
  SavedState SaveStateInitial() __TA_REQUIRES(chainlock_transaction_token) {
    ZX_DEBUG_ASSERT(Derived::Active() != this);
    return SavedState{this, Derived::AllocateReservedToken()};
  }

  // Restore the active transaction using the given saved state.
  static void RestoreState(SavedState saved_state) {
    Derived* transaction = static_cast<Derived*>(saved_state.transaction());
    ZX_DEBUG_ASSERT(transaction != nullptr);
    ZX_DEBUG_ASSERT(transaction->token().is_reserved());
    transaction->ReplaceToken(saved_state.token());
    Derived::SetActive(transaction);
  }

  // Directly finalize the transaction, ending an active contention tracing duration, if any.
  static void Finalize() __TA_REQUIRES(chainlock_transaction_token) {
    Derived& transaction = ActiveRef();
    transaction.AssertNotFinalized();
    transaction.SetFinalized(true);
  }

  // Restarts this transaction, resetting key state including the callsite info.
  void Restart(CallsiteInfo callsite_info) __TA_REQUIRES(chainlock_transaction_token) {
    AssertFinalized();
    AssertAtMostOneLockHeld();
    ZX_DEBUG_ASSERT(token().is_valid());
    SetFinalized(false);
    if constexpr (ContentionTracingEnabled) {
      tracing_.SetCallsiteInfo(callsite_info);
    }
  }

  // Accounting is a semi-private interface providing ChainLock access to debug accounting hooks.
  // While this type is primarily intended as an interface for ChainLock internal use, several
  // public methods are provided for non-production debug use.
  class Accounting {
   public:
    static uint32_t LocksHeld(const ChainLockTransaction* transaction) {
      static_assert(DebugAccountingEnabled);
      return transaction->accounting_.locks_held();
    }
    static bool IsFinalized(const ChainLockTransaction* transaction) {
      static_assert(DebugAccountingEnabled);
      return transaction->accounting_.finalized();
    }

   private:
    template <typename Transaction>
    friend class ChainLock;

    static void OnAcquire(ChainLockTransaction* transaction)
        __TA_REQUIRES(chainlock_transaction_token) {
      if constexpr (DebugAccountingEnabled) {
        transaction->accounting_.OnAcquire();
      }
    }
    static void OnRelease(ChainLockTransaction* transaction)
        __TA_REQUIRES(chainlock_transaction_token) {
      if constexpr (DebugAccountingEnabled) {
        transaction->accounting_.OnRelease();
      }
    }
    static void OnContention(ChainLockTransaction* transaction)
        __TA_REQUIRES(chainlock_transaction_token) {
      if constexpr (ContentionTracingEnabled) {
        transaction->tracing_.OnContention();
      }
    }
  };

  // Contention is a private interface providing ChainLock access to contention mitigation hooks.
  class Contention {
   private:
    template <typename Transaction>
    friend class ChainLock;

    using ContentionState = typename Derived::ContentionState;

    static bool RecordBackoffOrRetry(ChainLockTransaction* transaction, Token observed_token,
                                     std::atomic<Token>& lock_state,
                                     ContentionState& contention_state)
        __TA_REQUIRES(chainlock_transaction_token) {
      return static_cast<Derived*>(transaction)
          ->RecordBackoffOrRetry(observed_token, lock_state, contention_state);
    }

    template <typename Callable, typename = std::enable_if_t<std::is_invocable_r_v<void, Callable>>>
    static void ReleaseAndNotify(ChainLockTransaction* transaction,
                                 ContentionState& contention_state, Callable&& do_release)
        __TA_REQUIRES(chainlock_transaction_token) {
      static_cast<Derived*>(transaction)
          ->ReleaseAndNotify(contention_state, std::forward<Callable>(do_release));
    }
  };

  // Escape hatch methods allowing transactions to be instantiated outside the scope of the
  // preferred UntilDone interface.
  static Derived MakeBareTransaction(CallsiteInfo callsite_info)
      __TA_ACQUIRE(chainlock_transaction_token) {
    return Derived{callsite_info};
  }
  static Derived MakeBareTransaction(CallsiteInfo callsite_info, uint32_t lock_held,
                                     bool finalized) {
    return Derived{callsite_info, lock_held, finalized};
  }

  ~ChainLockTransaction() { Unregister(); }

 protected:
  explicit ChainLockTransaction(CallsiteInfo callsite_info) : tracing_{callsite_info} {
    Register();
  }
  ChainLockTransaction(CallsiteInfo info, uint32_t locks_held, bool finalized)
      : accounting_{locks_held, finalized}, tracing_{info} {}

  void Register() {
    ZX_DEBUG_ASSERT(Derived::Active() == nullptr);
    Derived::SetActive(static_cast<Derived*>(this));
  }
  void Unregister() {
    ZX_DEBUG_ASSERT(Derived::Active() == static_cast<Derived*>(this));
    Derived::SetActive(nullptr);
  }

  void SetFinalized(bool finalized) {
    state_ = finalized ? State::Finalized : State::Started;
    if constexpr (DebugAccountingEnabled) {
      accounting_.SetFinalized(finalized);
    }
    if constexpr (ContentionTracingEnabled) {
      if (finalized) {
        tracing_.OnFinalized();
      }
    }
  }

 private:
  // Extra state when contention tracing is enabled.
  class TraceStorageEnabled {
   public:
    TraceStorageEnabled() = default;
    explicit TraceStorageEnabled(CallsiteInfo callsite_info) : callsite_info_(callsite_info) {}
    ~TraceStorageEnabled() = default;

    void OnContention() {
      if (contention_start_ticks_ == kInvalidTicks) {
        contention_start_ticks_ = Derived::GetCurrentTicks();
      }
    }
    void OnFinalized() {
      if (contention_start_ticks_ != kInvalidTicks) {
        Derived::OnFinalized(contention_start_ticks_, callsite_info_);
        contention_start_ticks_ = kInvalidTicks;
      }
    }

    void SetCallsiteInfo(CallsiteInfo callsite_info) { callsite_info_ = callsite_info; }

   private:
    static constexpr zx_instant_mono_ticks_t kInvalidTicks = 0;
    zx_instant_mono_ticks_t contention_start_ticks_{kInvalidTicks};
    CallsiteInfo callsite_info_;
  };
  class TraceStorageDisabled {
   public:
    explicit TraceStorageDisabled(CallsiteInfo) {}
    ~TraceStorageDisabled() = default;
  };
  using TraceStorage =
      std::conditional_t<ContentionTracingEnabled, TraceStorageEnabled, TraceStorageDisabled>;

  // Extra state when debug accounting/validation is enabled.
  class AccountingStorageEnabled {
   public:
    AccountingStorageEnabled() = default;
    AccountingStorageEnabled(uint32_t locks_held, bool finalized)
        : locks_held_{locks_held}, finalized_{finalized} {}

    ~AccountingStorageEnabled() = default;

    uint32_t locks_held() const { return locks_held_; }
    bool finalized() const { return finalized_; }

    void OnAcquire() { ++locks_held_; }
    void OnRelease() { --locks_held_; }
    void SetFinalized(bool finalized) { finalized_ = finalized; }

   private:
    uint32_t locks_held_{0};
    bool finalized_{false};
  };
  class AccountingStorageDisabled {
   public:
    AccountingStorageDisabled() = default;
    AccountingStorageDisabled(uint32_t, bool) {}
    ~AccountingStorageDisabled() = default;
  };
  using AccountingStorage = std::conditional_t<DebugAccountingEnabled, AccountingStorageEnabled,
                                               AccountingStorageDisabled>;

  // The following two members use the no unique address attribute to avoid extra storage overhead
  // when the respective state is disabled.
  [[no_unique_address]] AccountingStorage accounting_;
  [[no_unique_address]] TraceStorage tracing_;
  uint64_t backoff_count_{0};
};

}  // namespace concurrent

#endif  // LIB_CONCURRENT_CHAINLOCK_TRANSACTION_H_
