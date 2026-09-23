// Copyright 2018 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_INCLUDE_KERNEL_BRWLOCK_H_
#define ZIRCON_KERNEL_INCLUDE_KERNEL_BRWLOCK_H_

#include <assert.h>
#include <debug.h>
#include <endian.h>
#include <lib/fxt/interned_string.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <stdint.h>

#include <fbl/canary.h>
#include <kernel/lock_trace.h>
#include <kernel/lock_validation_guard.h>
#include <kernel/owned_wait_queue.h>
#include <kernel/scheduler.h>
#include <kernel/spin_tracing_storage.h>
#include <kernel/thread.h>
#include <kernel/wait.h>
#include <ktl/atomic.h>

namespace internal {

enum class BrwLockEnablePi : bool {
  No = false,
  Yes = true,
};

template <BrwLockEnablePi PI>
struct BrwLockWaitQueueType;

template <>
struct BrwLockWaitQueueType<BrwLockEnablePi::Yes> {
  using Type = OwnedWaitQueue;
};

template <>
struct BrwLockWaitQueueType<BrwLockEnablePi::No> {
  using Type = WaitQueue;
};

template <BrwLockEnablePi PI>
struct BrwLockState;

template <>
struct alignas(16) BrwLockState<BrwLockEnablePi::Yes> {
  constexpr BrwLockState(uint64_t state, Thread* writer) : state_(state), writer_(writer) {}

  explicit constexpr BrwLockState(uint64_t state) : BrwLockState(state, nullptr) {}

  uint64_t state_;
  Thread* writer_;
};

static_assert(sizeof(BrwLockState<BrwLockEnablePi::Yes>) == 16,
              "PI BrwLockState expected to be exactly 16 bytes");
static_assert(BYTE_ORDER == LITTLE_ENDIAN, "PI BrwLockState assumptions little endian ordering");

template <>
struct BrwLockState<BrwLockEnablePi::No> {
  explicit constexpr BrwLockState(uint64_t state) : state_(state) {}

  constexpr BrwLockState(uint64_t state, Thread* writer) : BrwLockState(state) {}

  uint64_t state_;
};

template <BrwLockEnablePi enable_pi>
struct BlockOpLockDetails;

template <>
struct BlockOpLockDetails<BrwLockEnablePi::Yes> : public OwnedWaitQueue::BAAOLockingDetails {
  explicit BlockOpLockDetails(const OwnedWaitQueue::BAAOLockingDetails& owq_details)
      : OwnedWaitQueue::BAAOLockingDetails{owq_details} {}
};

template <>
struct BlockOpLockDetails<BrwLockEnablePi::No> {};

static_assert(sizeof(BrwLockState<BrwLockEnablePi::No>) == 8,
              "Non PI BrwLockState expected to be exactly 8 bytes");

// Blocking (i.e. non spinning) reader-writer lock. Readers and writers are
// ordered by priority (i.e. their wait_queue release order) and otherwise
// readers and writers are treated equally and will fall back to FIFO ordering
// at some priority.
// The lock optionally respects priority inheritance. Not supporting PI is more
// efficient as the current active writer does not have to be tracked. Enabling PI
// creates an additional restriction that readers must not take any additional
// locks or otherwise block whilst holding the read lock.
template <BrwLockEnablePi PI>
class TA_CAP("mutex") BrwLock
    : public spin_tracing::LockNameStorage<spin_tracing::LockType::kRwLock,
                                           kSchedulerLockSpinTracingEnabled |
                                               kLockNameTracingEnabled> {
 public:
  BrwLock() = default;
  explicit BrwLock(const fxt::InternedString& name_stringref)
      : LockNameStorage(name_stringref.id()) {}

  ~BrwLock();

  void ReadAcquire() TA_ACQ_SHARED() {
    DEBUG_ASSERT(!arch_blocking_disallowed());
    canary_.Assert();
    if constexpr (PI == BrwLockEnablePi::Yes) {
      // As readers are not recorded and do not receive boosting from blocking
      // writers they must not block or otherwise cease to run, otherwise
      // our PI will be violated.
      Thread::Current::preemption_state().PreemptDisable();
    }
    // Attempt the optimistic grab
    uint64_t prev =
        ktl::atomic_ref(state_.state_).fetch_add(kBrwLockReader, ktl::memory_order_acquire);
    // See if there are only readers
    if (unlikely((prev & kBrwLockReaderMask) != prev)) {
      ContendedReadAcquire();
    }
  }

  void WriteAcquire() TA_ACQ() {
    DEBUG_ASSERT(!arch_blocking_disallowed());
    canary_.Assert();
    // When acquiring the write lock we require there be no-one else using
    // the lock.
    CommonWriteAcquire(kBrwLockUnlocked, [this] { ContendedWriteAcquire(); });
  }

  void WriteRelease() TA_REL();

  void ReadRelease() TA_REL_SHARED() {
    canary_.Assert();
    uint64_t prev =
        ktl::atomic_ref(state_.state_).fetch_sub(kBrwLockReader, ktl::memory_order_release);
    if (unlikely((prev & kBrwLockReaderMask) == 1 && (prev & kBrwLockWaiterMask) != 0)) {
      LOCK_TRACE_DURATION("ContendedReadRelease", ("name", class_name_ref()));
      // there are no readers but still some waiters, becomes our job to wake them up
      ReleaseWakeup();
    }
    if constexpr (PI == BrwLockEnablePi::Yes) {
      Thread::Current::preemption_state().PreemptReenable();
    }
  }
  // Note: ReadUpgrade() is intentionally omitted because it is unused across the codebase
  // outside of tests and may be reconsidered when a concrete use case arises.

  // suppress default constructors
  DISALLOW_COPY_ASSIGN_AND_MOVE(BrwLock);

  // Tag structs needed for linking BrwLock acquisition options to the different
  // policy structures. See LOCK_DEP_POLICY_OPTION usage below.
  struct Reader {};
  struct Writer {};

  struct ReaderPolicy {
    struct State {};
    // This will be seen by Guard to know to generate shared acquisitions for thread analysis.
    struct Shared {};

    using ValidationGuard = LockValidationGuard;

    static void PreValidate(BrwLock*, State*) {}
    static bool Acquire(BrwLock* lock, State*) TA_ACQ_SHARED(lock) {
      lock->ReadAcquire();
      return true;
    }
    static void Release(BrwLock* lock, State*) TA_REL_SHARED(lock) { lock->ReadRelease(); }
  };

  struct WriterPolicy {
    struct State {};

    using ValidationGuard = LockValidationGuard;

    static void PreValidate(BrwLock*, State*) {}
    static bool Acquire(BrwLock* lock, State*) TA_ACQ(lock) {
      lock->WriteAcquire();
      return true;
    }
    static void Release(BrwLock* lock, State*) TA_REL(lock) { lock->WriteRelease(); }
  };

 private:
  static constexpr uint64_t kBrwLockUnlocked = 0;
  // We count readers in the low part of the state
  static constexpr uint64_t kBrwLockReader = 1;
  static constexpr uint64_t kBrwLockReaderMask = 0xFFFFFFFF;
  // We count waiters in all but the MSB of the state
  static constexpr uint64_t kBrwLockWaiter = 1ul << 32;
  static constexpr uint64_t kBrwLockWaiterMask = 0x7FFFFFFF00000000;
  // Writer is in the MSB
  static constexpr uint64_t kBrwLockWriter = 1ul << 63;

  static constexpr bool StateHasReaders(uint64_t state) {
    return (state & kBrwLockReaderMask) != 0;
  }
  static constexpr bool StateHasWriter(uint64_t state) { return (state & kBrwLockWriter) != 0; }
  static constexpr bool StateHasWaiters(uint64_t state) {
    return (state & kBrwLockWaiterMask) != 0;
  }
  static constexpr bool StateHasExclusiveReader(uint64_t state) {
    return (state & ~kBrwLockWaiterMask) == kBrwLockReader;
  }
  static constexpr uint32_t StateReaderCount(uint64_t state) {
    return static_cast<uint32_t>(state & kBrwLockReaderMask);
  }

  void ContendedReadAcquire() TA_EXCL(chainlock_transaction_token);
  void ContendedWriteAcquire() TA_EXCL(chainlock_transaction_token);
  void ReleaseWakeup() TA_EXCL(chainlock_transaction_token);

  ktl::optional<BlockOpLockDetails<PI>> LockForBlock()
      TA_REQ(chainlock_transaction_token, wait_.get_lock());

  void Block(Thread* const current_thread, const BlockOpLockDetails<PI>& lock_details, bool write)
      TA_REQ(chainlock_transaction_token) TA_REL(wait_.get_lock(), current_thread->get_lock());

  // TryWake requires that there be an active ChainLockTransaction in progress,
  // and will finalize that transaction if (and only if) the wake operation
  // succeeds.
  ktl::optional<ResourceOwnership> TryWake()
      TA_REQ(chainlock_transaction_token, wait_.get_lock(), preempt_disabled_token);

  struct AcquireResult {
    const bool success;
    const uint64_t state;

    explicit operator bool() const { return success; }
  };

  AcquireResult AtomicWriteAcquire(uint64_t expected_state_bits, Thread* current_thread) {
    // Clang considers a type "always lock-free" when compare_exchange
    // operations work on that type.  GCC also requires that it believe that
    // atomic load operations are also actually atomic, which isn't the case of
    // 16-byte quantities on machines with 8-byte words.  But we only care that
    // compare_exchange be atomic, not that 16-byte atomic loads be available.
#ifdef __clang__
    constexpr bool kLockFree = decltype(ktl::atomic_ref(state_))::is_always_lock_free;
#elif defined(__aarch64__) || defined(__x86_64__)
    constexpr bool kLockFree = true;
#else
    constexpr bool kLockFree = false;
#endif

    // To prevent a race between setting the kBrwLocKWriter bit and the writer_
    // we perform a 16-byte compare and swap of both values. This ensures that
    // Block can never fail to see a writer_. Other possibilities are:
    //
    //   * Disable interrupts: This would be correct, but disabling interrupts
    //     is more expensive than a 16-byte CAS.
    //
    //   * thread_preempt_disable: Cheaper than disabling interrupts but is
    //     **INCORRECT** as when preemption happens we must take the
    //     thread_lock to proceed, but Block must hold the thread lock until it
    //     observes that writer_ has been set, thus resulting in deadlock.
    if constexpr (kLockFree) {
      BrwLockState<PI> current_state(expected_state_bits, nullptr);
      const BrwLockState<PI> new_state(kBrwLockWriter, current_thread);
      ktl::atomic_ref state(state_);
      const bool success = state.compare_exchange_strong(
          current_state, new_state, ktl::memory_order_acquire, ktl::memory_order_relaxed);
      return {success, current_state.state_};
    } else {
      PANIC_UNIMPLEMENTED;
    }
  }

  template <typename F>
  void CommonWriteAcquire(uint64_t expected_state_bits, F contended)
      TA_ACQ(this) TA_NO_THREAD_SAFETY_ANALYSIS {
    Thread* current_thread = Thread::Current::Get();
    if (unlikely(!AtomicWriteAcquire(expected_state_bits, current_thread))) {
      contended();
      if constexpr (PI == BrwLockEnablePi::Yes) {
        DEBUG_ASSERT(ktl::atomic_ref(state_.writer_).load(ktl::memory_order_relaxed) ==
                     current_thread);
      }
    }
  }

  fbl::Canary<fbl::magic("RWLK")> canary_;
  BrwLockState<PI> state_{kBrwLockUnlocked};
  typename BrwLockWaitQueueType<PI>::Type wait_;
};

// Must declare policy options whilst in the internal namespace for ADL resolution to work.
using BrwLockPi = BrwLock<BrwLockEnablePi::Yes>;

// Configure fbl::Guard<BrwLockPi, BrwLockPi::Writer> write locks through the given policy.
LOCK_DEP_POLICY_OPTION(BrwLockPi, BrwLockPi::Writer, BrwLockPi::WriterPolicy);
// Configure fbl::Guard<BrwLockPi, BrwLockPi::Reader> read locks through the given policy.
LOCK_DEP_POLICY_OPTION(BrwLockPi, BrwLockPi::Reader, BrwLockPi::ReaderPolicy);

using BrwLockNoPi = BrwLock<BrwLockEnablePi::No>;

// Configure fbl::Guard<BrwLockNoPi, BrwLockNoPi::Writer> write locks through the given policy.
LOCK_DEP_POLICY_OPTION(BrwLockNoPi, BrwLockNoPi::Writer, BrwLockNoPi::WriterPolicy);
// Configure fbl::Guard<BrwLockNoPi, BrwLockNoPi::Reader> read locks through the given policy.
LOCK_DEP_POLICY_OPTION(BrwLockNoPi, BrwLockNoPi::Reader, BrwLockNoPi::ReaderPolicy);

}  // namespace internal

#ifdef __riscv
// TODO(https://fxbug.dev/42075694) Optimally implement BrwLock
// Workaround for lack of doubleword CAS in the brwlock implementation on RISC-V that is only
// used in the PI path. Simply switch to using the no PI version for this architecture.
using BrwLockPi = ::internal::BrwLockNoPi;
#else
using BrwLockPi = ::internal::BrwLockPi;
#endif

#define DECLARE_BRWLOCK_PI(container_type, ...) \
  LOCK_DEP_INSTRUMENT(container_type, BrwLockPi, ##__VA_ARGS__)
#define DECLARE_SINGLETON_BRWLOCK_PI(name, ...) \
  LOCK_DEP_SINGLETON_LOCK(name, BrwLockPi, ##__VA_ARGS__)

using BrwLockNoPi = internal::BrwLockNoPi;

#define DECLARE_BRWLOCK_NO_PI(container_type, ...) \
  LOCK_DEP_INSTRUMENT(container_type, BrwLockNoPi, ##__VA_ARGS__)
#define DECLARE_SINGLETON_BRWLOCK_NO_PI(name, ...) \
  LOCK_DEP_SINGLETON_LOCK(name, BrwLockNoPi, ##__VA_ARGS__)

#endif  // ZIRCON_KERNEL_INCLUDE_KERNEL_BRWLOCK_H_
