// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_KERNEL_LIB_KCONCURRENT_INCLUDE_LIB_KCONCURRENT_CHAINLOCK_H_
#define ZIRCON_KERNEL_LIB_KCONCURRENT_INCLUDE_LIB_KCONCURRENT_CHAINLOCK_H_

#include <lib/arch/intrin.h>
#include <lib/concurrent/chainlock.h>
#include <lib/concurrent/chainlock_guard.h>
#include <lib/concurrent/chainlock_transaction.h>
#include <lib/concurrent/chainlock_transaction_common.h>
#include <lib/concurrent/chainlockable.h>
#include <lib/fxt/interned_string.h>
#include <lib/fxt/string_ref.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <zircon/compiler.h>
#include <zircon/time.h>

#include <kernel/cpu.h>
#include <kernel/spin_tracing_config.h>
#include <ktl/atomic.h>

// Forward declaration.
struct percpu;

// Produce trace records documenting the CLT contention overhead if we have lock
// contention tracing enabled.
static constexpr bool kCltTracingEnabled = kSchedulerLockSpinTracingEnabled;
static constexpr bool kCltDebugChecksEnabled = LK_DEBUGLEVEL >= 2;

class ChainLockTransaction;

template <>
struct concurrent::CallsiteTraits<ChainLockTransaction> {
  // Stores callsite info for the current transaction used in debugging and tracing.
  struct CallsiteInfo {
    const fxt::InternedString* label{nullptr};
    uint32_t line_number{0};
  };
};

// Returns a CallsiteInfo instance for the given string literal label at the current source
// location.
#define CLT_TAG(label)                                                                 \
  []() constexpr -> ::concurrent::CallsiteTraits<ChainLockTransaction>::CallsiteInfo { \
    using fxt::operator""_intern;                                                      \
    return {&label##_intern, __LINE__};                                                \
  }()

using ::concurrent::chainlock_transaction_token;

// ChainLockTransaction derives from concurrent::ChainLockTransaction to adapt the environment-
// independent libconcurrent chain lock library to the kernel environment by providing the following
// methods and types required by the base CRTP type:
//
//   static void SetActive(ChainLockTransaction*);
//   static void ChainLockTransaction* Active();
//   static Token AllocateReservedToken();
//   static zx_instant_mono_ticks_t GetCurrentTicks();
//   static void OnFinalized(zx_instant_mono_t, concurrent::CallsiteInfo);
//   template <StateOptions> struct StateSaver;
//
// The above are declared in this file and defined in lib/kconcurrent/chainlock_transaction.h to
// avoid circular dependencies with kernel/thread.h, which uses the types declared here and also
// provides some of the facilities used by the methods and types listed above.
//
class ChainLockTransaction
    : public ::concurrent::ChainLockTransaction<ChainLockTransaction, kCltDebugChecksEnabled,
                                                kCltTracingEnabled> {
 public:
  ~ChainLockTransaction() TA_REL(chainlock_transaction_token);

  // Returns the active chain lock transaction for the current CPU or nullptr.
  static ChainLockTransaction* Active();

  // Performs an architecture-specific CPU relax operation.
  static void Yield() { arch::Yield(); }

  // Specifies which IRQ save and preempt disable options are required over the transaction.
  enum class StateOptions {
    NoIrqSave,
    IrqSave,
    PreemptDisable,
    EagerReschedDisable,
  };

  // Saves and restores the IRQ and preempt disable state corresponding to the given option.
  // Although this is mainly used internally by concurrent::ChainLockTransaction::UntilDone, there
  // are some instances where bare transactions need to be created. Define this publicly to support
  // those use cases.
  template <StateOptions>
  struct StateSaver;

  // State used by concurrent::ChainLock to store contended lock state for backoff mitigation.
  using ContentionState = ktl::atomic<cpu_mask_t>;
  using CallsiteInfo = ::concurrent::CallsiteTraits<ChainLockTransaction>::CallsiteInfo;

 private:
  using Base = ::concurrent::ChainLockTransaction<ChainLockTransaction, kCltDebugChecksEnabled,
                                                  kCltTracingEnabled>;

  template <StateOptions>
  friend class SingleChainLockGuard;
  friend Base;

  explicit ChainLockTransaction(CallsiteInfo callsite_info) TA_ACQ(chainlock_transaction_token);

  ChainLockTransaction(CallsiteInfo callsite_info, uint32_t locks_held, bool finalized);

  // Updates kcounters to track backoff rates.
  void OnBackoff();

  // Handle backoff mitigation.
  bool RecordBackoffOrRetry(Token observed_token, ktl::atomic<Token>& lock_state,
                            ContentionState& contention_state);

  // Carefully sequence the operations required to release the chainlock and notify backed-off
  // contenders.
  template <typename Callable>
  void ReleaseAndNotify(ContentionState& contention_state, Callable&& do_release);

  // Waits for the observed contention to be resolved on the given CPU that is latched before
  // IRQ/preemption state is restored during relax.
  void WaitForConflictResolution(percpu* percpu);

  // Sets the active transaction for the current CPU.
  static void SetActive(ChainLockTransaction*);

  // Returns a reserved token corresponding to the current CPU.
  static Token AllocateReservedToken();

  // Gets the current monotonic ticks.
  static zx_instant_mono_ticks_t GetCurrentTicks();

  // Emits a trace event for the given contention information.
  static void OnFinalized(zx_instant_mono_t contention_start_ticks, CallsiteInfo callsite_info);

  static void UpdateContentionCounters(zx_duration_mono_ticks_t contention_ticks);

  uint64_t conflict_id_{0};
};

// Constants for each StateOption that are passed to ChainLockTransaction::UntilDone(option,
// callsite, transaction_lambda).
constexpr ChainLockTransaction::Option<ChainLockTransaction::StateOptions::NoIrqSave>
    NoIrqSaveOption{};
constexpr ChainLockTransaction::Option<ChainLockTransaction::StateOptions::IrqSave> IrqSaveOption{};
constexpr ChainLockTransaction::Option<ChainLockTransaction::StateOptions::PreemptDisable>
    PreemptDisableAndIrqSaveOption{};
constexpr ChainLockTransaction::Option<ChainLockTransaction::StateOptions::EagerReschedDisable>
    EagerReschedDisableAndIrqSaveOption{};

// Aliases of each libconcurrent chain lock type that depends on the concrete ChainLockTransaction
// declared above to adapt to the kernel environment.
using ChainLock = ::concurrent::ChainLock<ChainLockTransaction>;
using ChainLockable = ::concurrent::ChainLockable<ChainLockTransaction>;
using ChainLockGuard = ::concurrent::ChainLockGuard<ChainLockTransaction>;

#endif  // ZIRCON_KERNEL_LIB_KCONCURRENT_INCLUDE_LIB_KCONCURRENT_CHAINLOCK_H_
