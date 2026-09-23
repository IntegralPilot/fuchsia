// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_CONCURRENT_CHAINLOCK_H_
#define LIB_CONCURRENT_CHAINLOCK_H_

#include <inttypes.h>
#include <lib/concurrent/chainlock_transaction_common.h>
#include <zircon/assert.h>
#include <zircon/compiler.h>

#include <atomic>
#include <cstddef>

// # General Chain Lock Facility
//
// Chain locks are a type of spin lock designed for use cases where multiple CPUs need to acquire
// various sets of locks in different orders, such as in dynamic object graphs where nodes have
// lock-synchronized states and references to other nodes. Chain locks provide an arbitration
// mechanism that is particularly useful in operations that require locking multiple nodes
// simultaneously, such as propagating state changes or altering the graph's connectivity.
// Operations initiated from different nodes in the graph may require locks on some of the same
// nodes, but in different orders. If ordinary spin locks were used, such lock order variations
// could lead to deadlocks.
//
// Similar to other spin locks, chain locks use atomic variables for lock state and memory ordering,
// and require disabling interrupts to guarantee IRQ-safe critical sections. However, chain locks
// differ from spin locks in that they don't use CPU IDs or other static values to indicate the
// locked state and identify the owner. Instead, the locked value is derived from a global
// generation counter that provides a priority invariant used in arbitration. An operation that
// requires one or more chain locks is performed in a transaction that carries a unique generation
// count. Within a transaction, a CPU can attempt to lock multiple chain locks using the same
// generation count, giving rise to the name "chain lock."
//
// The generation count provides a fair arbitration mechanism when multiple CPUs attempt to acquire
// the same locks in potentially different orders. During arbitration, the transaction with the
// lowest generation count has the highest priority. If a CPU encounters contention and has a
// transaction with a higher priority than the lock's current owner, it simply spins until the lock
// is released. However, if the transaction has a lower priority, the CPU must back off by releasing
// all held chain locks and restarting the locking sequence. This is because the CPU with the higher
// priority transaction may be waiting to acquire one or more of these locks. Once a transaction is
// complete its generation count is retired and never reused, ensuring that all transactions
// eventually make progress, even under heavy contention.
//

namespace concurrent {

// ChainLock<Transaction> provides the environment-independent chain lock interface and protocol.
// The Transaction template type specifies the envrionment-specific transaction implementation,
// which provides the active transaction and debug/tracing hook interfaces.
//
// The Transaction type must provide the following methods, which do not need to be public but must
// be accessible to ChainLock<Transaction>:
// - static Transaction* Transaction::Active()
// - static Transaction* Transaction::Yield()
// - static void Transaction::Accounting::OnAcquire(Transaction*)
// - static void Transaction::Accounting::OnRelease(Transaction*)
// - static void Transaction::Accounting::OnContention(Transaction*)
// - static bool Transaction::Contention::RecordBackoffOrRetry(Token, atd::atomic<Token>&,
//                                                             Transaction::ContentionState&)
// - TransactionBase::Token Transaction::token()
//
// ChainLockTransaction<Transaction> is a nearly complete transaction implementation that
// environment-specific transaction types may derive from that includes additional interfaces and
// affordances.
//
template <typename Transaction>
class __TA_CAPABILITY("mutex") ChainLock {
 public:
  using Token = ChainLockTransactionCommon::Token;

  ChainLock() = default;

  ~ChainLock() {
    ZX_DEBUG_ASSERT_MSG(const Token lock_token = state_.load(std::memory_order_relaxed);
                        lock_token == kUnlockedToken, "Destroying locked ChainLock: token=%" PRIu64,
                        lock_token.value());
  }

  ChainLock(const ChainLock&) = delete;
  ChainLock(ChainLock&&) = delete;
  ChainLock& operator=(const ChainLock&) = delete;
  ChainLock& operator=(ChainLock&&) = delete;

  enum class Result {
    // Lock successfully acquired.
    Ok,

    // Lock is held by a lower priority transaction. Returned only by TryAcquire. Other acquire
    // methods spin internally and do not return this result.
    Retry,

    // Lock is held by a higher priority transaction, requiring the current transaction to follow
    // the back off protocol.
    Backoff,

    // Lock is already held by the current transaction and would otherwise deadlock, requiring the
    // current transaction to follow the cycle detected protocol. May be fatal in some environments.
    Cycle,
  };

  // Acquires the lock without backoff, spinning until acquired. This method may be used for the
  // first chain lock acquired in a transaction as an optimization, since backoff is unnecessary
  // when no other chain locks are held.
  //
  // Asserts if a cycle is detected or if any chain locks are currently held.
  void AcquireFirstInChain() __TA_REQUIRES(chainlock_transaction_token) __TA_ACQUIRE() {
    Transaction* transaction = Transaction::Active();
    transaction->AssertNotFinalized();
    transaction->AssertNumLocksHeld(0);

    const Token token = transaction->token();
    while (true) {
      const AttemptResult result = AttemptAcquireCommon(token);
      if (result == Result::Ok) {
        Transaction::Accounting::OnAcquire(transaction);
        return;
      }

      Transaction::Accounting::OnContention(transaction);

      ZX_DEBUG_ASSERT(result != Result::Cycle);
      Transaction::Yield();
    }
  }

  // Acquires the lock, spinning until acquired or returning failure because the caller should
  // backoff due to contention with a higher priority transaction. Returns true if the lock was
  // acquired or false if the current transaction should back off. Asserts if a cycle is detected.
  [[nodiscard]] bool AcquireOrBackoff() __TA_REQUIRES(chainlock_transaction_token)
      __TA_TRY_ACQUIRE(true) {
    Transaction* transaction = Transaction::Active();
    transaction->AssertNotFinalized();

    const Token token = transaction->token();
    while (true) {
      const AttemptResult result = AttemptAcquireCommon(token);
      if (result == Result::Ok) {
        Transaction::Accounting::OnAcquire(transaction);
        return true;
      }

      Transaction::Accounting::OnContention(transaction);

      // If recording the backoff fails, the transaction owning the lock changed, which could be
      // either higher or lower priority than the current transaction. In that case, retry the
      // acquisition instead of returning false and unwinding the transaction.
      if (result == Result::Backoff &&
          Transaction::Contention::RecordBackoffOrRetry(transaction, result.observed_token, state_,
                                                        contention_state_)) {
        return false;
      }

      ZX_DEBUG_ASSERT(result != Result::Cycle);
      Transaction::Yield();
    }
  }

  // Acquires the lock, spinning until acquired or returning failure and backoff or cycle as an out
  // parameter.  Returns true if the lock was acquired or false if acquiring the lock failed.
  [[nodiscard]] bool AcquireOrResult(Result& result_out) __TA_REQUIRES(chainlock_transaction_token)
      __TA_TRY_ACQUIRE(true) {
    Transaction* transaction = Transaction::Active();
    transaction->AssertNotFinalized();

    const Token token = transaction->token();
    while (true) {
      AttemptResult result = AttemptAcquireCommon(token);
      if (result == Result::Ok) {
        result_out = result.lock_result;
        Transaction::Accounting::OnAcquire(transaction);
        return true;
      }

      Transaction::Accounting::OnContention(transaction);

      // If recording the backoff fails, the transaction owning the lock changed, which could be
      // either higher or lower priority than the current transaction. In that case, retry the
      // acquisition instead of failing and returning the result.
      if (result == Result::Backoff &&
          !Transaction::Contention::RecordBackoffOrRetry(transaction, result.observed_token, state_,
                                                         contention_state_)) {
        result.lock_result = Result::Retry;
      }

      if (result != Result::Retry) {
        result_out = result.lock_result;
        return false;
      }

      Transaction::Yield();
    }
  }

  enum class AllowFinalized : bool {
    No,
    Yes,
  };
  enum class RecordBackoff : bool {
    No,
    Yes,
  };

  // Attempts to acquire the lock. Returns true if the lock was acquired or false if acquiring the
  // lock failed. Asserts if a cycle is detected.
  [[nodiscard]] bool TryAcquire(AllowFinalized allow_finalized = AllowFinalized::No,
                                RecordBackoff record_backoff = RecordBackoff::Yes)
      __TA_REQUIRES(chainlock_transaction_token) __TA_TRY_ACQUIRE(true) {
    Transaction* transaction = Transaction::Active();
    if (allow_finalized == AllowFinalized::No) {
      transaction->AssertNotFinalized();
    }

    const Token token = transaction->token();
    const AttemptResult result = AttemptAcquireCommon(token);
    if (result == Result::Ok) {
      Transaction::Accounting::OnAcquire(transaction);
      return true;
    }

    Transaction::Accounting::OnContention(transaction);

    if (result == Result::Backoff && record_backoff == RecordBackoff::Yes) {
      Transaction::Contention::RecordBackoffOrRetry(transaction, result.observed_token, state_,
                                                    contention_state_);
    }

    ZX_DEBUG_ASSERT(result != Result::Cycle);
    return false;
  }

  // Releases the lock.
  void Release() __TA_REQUIRES(chainlock_transaction_token) __TA_RELEASE() {
    Transaction* transaction = Transaction::Active();
    ZX_DEBUG_ASSERT(const Token token = transaction->token();
                    token == state_.load(std::memory_order_relaxed));

    Transaction::Accounting::OnRelease(transaction);

    // The environment-specific contention backoff strategy may require a careful order of
    // operations for notifying backed-off contenders of the release and the actual lock release
    // operation to ensure that BOTH of the following properties hold:
    //
    // 1. Races between backing off and releasing the lock do not result in missed notifications.
    // 2. The contention state of the lock is not accessed after the lock is released to avoid
    //    use-after-free hazards.
    //
    // The following protocol ensures these properties hold:
    //
    // 1. The lock token is set to the maximum token value to temporarily prevent further updates to
    //    the contention state by new contenders, since the maximum token value is the lowest
    //    possible priority for a chain lock transaction.
    // 2. The lock contention state is sampled to determine which backed-off contenders to notify
    //    after the lock is released.
    // 3. The lock is released, allowing contenders to attempt to acquire the lock.
    // 4. Contenders that backed off prior to the lock release are notified.
    //
    // The environment-specific implementation of ReleaseAndNotify is responsible for ensuring that
    // steps 2 through 4 are implemented correctly, using the provided lambda to implement step 3.
    //
    // TODO(johngro): Look into relaxing the memory ordering being used here.  I
    // _think_ that we can perform this store to the lock's state using relaxed
    // semantics, but I'm not going to take that chance without having first built
    // a model and run it through CDSChecker.
    //
    state_.store(kMaxToken, std::memory_order_release);
    Transaction::Contention::ReleaseAndNotify(transaction, contention_state_, [this] {
      state_.store(kUnlockedToken, std::memory_order_release);
    });
  }

  // Asserts that the lock is held by the current transaction.
  void AssertHeld() const __TA_REQUIRES(chainlock_transaction_token) __TA_ASSERT() {
    Transaction* transaction = Transaction::Active();
    const Token token = transaction->token();
    ZX_DEBUG_ASSERT_MSG(Token lock_token = state_.load(std::memory_order_relaxed);
                        lock_token == token,
                        "Failed to assert ChainLock is held (lock token 0x%lx, our token 0x%lx).",
                        lock_token.value(), token.value());
  }

  // Asserts that the lock is held by the current transaction and must be released by the
  // appropriately annotated scope.
  void AssertAcquired() const __TA_REQUIRES(chainlock_transaction_token) __TA_ACQUIRE() {
    AssertHeld();
  }

  // A no-op version of assert held.  USE WITH EXTREME CAUTION.  This tells the static analyzer that
  // we are holding the lock, but makes no actual runtime effort to verify this.
  //
  // There are very few places where it is OK to use this method, but here are two examples.
  //
  // Example 1: We just verified that we have the lock.
  // ```
  // if (const ChainLock::Result res = my_obj.lock_.Acquire(token);
  //     res != ChainLock::Result::Ok) {
  //   return SomeError;
  // }
  // my_obj.lock_MarkHeld();
  // ```
  // It is OK to skip the actual assert operations here because we just checked to make sure that
  // the operation succeeded.  There is not much point in wasting cycles to do any more.
  //
  // Example 2: The analyzer is confused about which lock is which because of some tricky code, like
  //            a dynamic downcast.
  // ```
  // Obj* GetObj(BaseClass* base) TA_REQ(base->lock_) {
  //   DerivedClass* derived = DowncastToDerived(base);
  //
  //   if (derived) {
  //    derived->lock_.MarkHeld();
  //    derived->protected_obj_;
  //   }
  //
  //   return nullptr;
  // }
  // ```
  //
  // In this case, we have a base class with a lock, and a derived class with a member which is
  // protected by the base class's lock.  We are also operating in an environment where RTTI is
  // disabled, so we cannot use dynamic_cast.  Instead, we need to write our own dynamic downcast
  // function using some internal mechanism to ensure that we can do so safely.
  //
  // We are attempting to fetch the protected member from the derived class, but only if exists,
  // which is may not if the base class pointer we have does not actually point to an instance of
  // the derived class.
  //
  // We have already statically required that the base class instance be locked, but when we perform
  // our downcast, the static analyzer is not smart enough to understand that base->lock_ is the
  // same thing as derived->lock_, we we use the no-op form of AssertHeld to satisfy it instead.
  void MarkHeld() const __TA_REQUIRES(chainlock_transaction_token) __TA_ASSERT() {}
  void MarkNeedsRelease() const __TA_REQUIRES(chainlock_transaction_token) __TA_ACQUIRE() {}

  // Complementary function to MarkNeedsRelease to assure static analysis that the lock is released.
  void MarkReleased() const __TA_REQUIRES(chainlock_transaction_token) __TA_RELEASE() {}

  // Conditionally marks the lock as held if held by the current transaction.
  [[nodiscard]] bool MarkNeedsReleaseIfHeld() const __TA_REQUIRES(chainlock_transaction_token)
      __TA_TRY_ACQUIRE(true) {
    return is_held();
  }

  // Returns true if the lock is held by the current transaction.
  bool is_held() const __TA_REQUIRES(chainlock_transaction_token) {
    Transaction* transaction = Transaction::Active();

    const Token token = transaction->token();
    return state_.load(std::memory_order_relaxed) == token;
  }

  // Returns true if the lock is unlocked.
  bool is_unlocked() const { return state_.load(std::memory_order_relaxed) == kUnlockedToken; }

  // Syncs the held token value of this lock with the current transaction token. Used for saving and
  // restoring state in special circumstances that use reserved tokens.
  void SyncToken() __TA_REQUIRES(chainlock_transaction_token) {
    Transaction* transaction = Transaction::Active();
    const Token token = transaction->token();
    state_.store(token, std::memory_order_release);
  }

  Token state_for_testing() const { return state_.load(std::memory_order_relaxed); }

 private:
  struct AttemptResult {
    Result lock_result;
    Token observed_token;

    constexpr bool operator==(Result other_lock_result) const {
      return lock_result == other_lock_result;
    }
    constexpr bool operator!=(Result other_lock_result) const {
      return lock_result != other_lock_result;
    }
  };

  AttemptResult AttemptAcquireCommon(Token token) {
    Token expected{kUnlockedToken};
    if (state_.compare_exchange_strong(expected, token, std::memory_order_acquire,
                                       std::memory_order_acquire)) {
      return {Result::Ok, expected};
    }
    if (expected > token) {
      return {Result::Retry, expected};
    }
    if (expected < token) {
      return {Result::Backoff, expected};
    }
    return {Result::Cycle, expected};
  }

  static constexpr Token kUnlockedToken{ChainLockTransactionCommon::kUnlockedTokenValue};
  static constexpr Token kMaxToken{ChainLockTransactionCommon::kMaxTokenValue};

  static_assert(std::atomic<Token>::is_always_lock_free);

  std::atomic<Token> state_{kUnlockedToken};
  typename Transaction::ContentionState contention_state_{};
};

// Utility to simplify expressions where two chain locks need to be acquired with backoff. Returns
// true if both locks where acquired or false if the current transaction needs to back off.
template <typename Transaction>
inline bool AcquireBothOrBackoff(ChainLock<Transaction>& first, ChainLock<Transaction>& second)
    __TA_REQUIRES(chainlock_transaction_token) __TA_TRY_ACQUIRE(true, first, second) {
  if (first.AcquireOrBackoff()) {
    if (second.AcquireOrBackoff()) {
      return true;
    }
    first.Release();
  }
  return false;
}

}  // namespace concurrent

#endif  // LIB_CONCURRENT_CHAINLOCK_H_
