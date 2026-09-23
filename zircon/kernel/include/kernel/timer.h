// Copyright 2016 The Fuchsia Authors
// Copyright (c) 2008-2009 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_INCLUDE_KERNEL_TIMER_H_
#define ZIRCON_KERNEL_INCLUDE_KERNEL_TIMER_H_

#include <lib/kconcurrent/chainlock.h>
#include <string-file.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#include <fbl/canary.h>
#include <fbl/intrusive_double_list.h>
#include <kernel/deadline.h>
#include <kernel/spinlock.h>
#include <ktl/atomic.h>

// Rules for Timers:
// - Timer callbacks occur from interrupt context.
// - Timers may be programmed or canceled from interrupt or thread context.
// - Timers may be canceled or reprogrammed from within their callback.
// - Setting and canceling timers is not thread safe and cannot be done concurrently.
// - Timer::cancel() may spin waiting for a pending timer to complete on another cpu.
// - Timers aren't permanently bound to a particular CPU and may fire on a CPU other
//   than the one on which they were set.

// Timers may be removed from an arbitrary TimerQueue, so their list
// node requires the AllowRemoveFromContainer option.
class Timer : public fbl::DoublyLinkedListable<Timer*, fbl::NodeOptions::AllowRemoveFromContainer> {
 public:
  using Callback = void (*)(Timer*, zx_time_t now, void* arg);

  // Timers need a constexpr constructor, as it is valid to construct them in static storage.
  // TODO(https://fxbug.dev/328306129): The default value for the clock_id parameter should be
  // removed, thus forcing users of the Timer class to explicitly declare the clock they wish
  // to use.
  constexpr explicit Timer(zx_clock_t clock_id = ZX_CLOCK_MONOTONIC) : clock_id_(clock_id) {}

  // We ensure that timers are not on a list or an active cpu when destroyed.
  ~Timer();

  // Timers are not moved or copied.
  Timer(const Timer&) = delete;
  Timer(Timer&&) = delete;
  Timer& operator=(const Timer&) = delete;
  Timer& operator=(Timer&&) = delete;

  // Set up a timer that executes once
  //
  // This function specifies a callback function to be run after a specified
  // deadline passes. The function will be called one time.
  //
  // deadline: specifies when the timer should be executed
  // callback: the function to call when the timer expires
  // arg: the argument to pass to the callback
  //
  // The timer function is declared as:
  //   void callback(Timer *, zx_time_t now, void *arg) { ... }
  void Set(const Deadline& deadline, Callback callback, void* arg);

  // Cancel a pending timer
  //
  // Returns true if the timer was canceled before it was
  // scheduled in a cpu and false otherwise or if the timer
  // was not scheduled at all.
  //
  bool Cancel();

  // Equivalent to Set with no slack
  // The deadline parameter should be interpreted differently depending on the clock_id_ field.
  // If clock_id_ is set to ZX_CLOCK_MONOTONIC, deadline is a zx_instant_mono_t.
  // If clock_id_ is set to ZX_CLOCK_BOOT, deadline is a zx_instant_boot_t.
  void SetOneshot(zx_time_t deadline, Callback callback, void* arg) {
    Set(Deadline::no_slack(deadline), callback, arg);
  }

  // Special helper routine to simultaneously try to acquire a spinlock and
  // check for timer cancel, which is needed in a few special cases. Returns
  // ZX_OK if spinlock was acquired, ZX_ERR_TIMED_OUT if timer was canceled.
  zx_status_t TrylockOrCancel(MonitoredSpinLock* lock) TA_TRY_ACQ(false, lock);
  zx_status_t TrylockOrCancel(ChainLock& lock) TA_REQ(chainlock_transaction_token)
      TA_TRY_ACQ(false, lock);

  // Private accessors for timer tests.
  zx_duration_t slack_for_test() const { return slack_; }

  // This function returns a zx_instant_mono_t if the expected_clock_id is ZX_CLOCK_MONOTONIC and a
  // zx_instant_boot_t if the expected_clock_id is ZX_CLOCK_BOOT.
  zx_time_t scheduled_time_for_test(zx_clock_t expected_clock_id) const {
    DEBUG_ASSERT(clock_id_ == expected_clock_id);
    return scheduled_time_;
  }

 private:
  // TimerQueues can directly manipulate the state of their enqueued Timers.
  friend class TimerQueue;

  // TimerSyncVar is a small helper class used to help us follow some of the
  // trickier rules when it comes to synchronization and timer operations.  Most
  // of timer and timer queue state is protected by the global TimerLock, which
  // guarantees ordering.  There are some examples which are a bit more
  // complicated.  Consider the cancel operation.
  //
  // When a timer needs to fire, the global lock is held and the timer's
  // deadline is evaluated.  If the deadline has arrived, the TimerTick code
  // will remove the timer from the appropriate pending list, and then indicate
  // that there is a callback-in-flight by setting the `active_cpu_` atomic to
  // the current CPU value.  Then it drops the lock and performs the callback.
  // Both dropping the lock and recording which cpu is handling the callback are
  // important here because the callback can choose to call into the timer's
  // methods during the callback.  Calling "set" is a good example of something
  // someone might want to do from the timer callback.
  //
  // Now consider another CPU attempting to cancel a timer T when there is a
  // callback already in flight.  Cancel is supposed to return a boolean:
  //
  // 1) True means that the cancel succeeded.  The timer was canceled before any
  //    callback was made, and now there is a guarantee that no callback will be
  //    made and that the timer's owner is free to destroy the Timer.
  // 2) False means that the cancel failed.  When control is returned to the
  //    caller, the callback is guaranteed to have finished.
  //
  // So, when cancel runs, it grabs the lock and checks to see if the callback
  // has already started.  If it has, then it need to drop the lock (so the
  // TimerTick code can reacquire the lock and mark the callback done via
  // active_cpu_), _and wait for the callback to finish_.  It does this by
  // spinning on the `active_cpu_` variable, read using atomic loads.
  //
  // It is important, however, that when the callback has finished, that the
  // cancel CPU is guaranteed that not only is the callback done, but that all
  // of the side effects of the callback are visible to the cancel CPU.  So, it
  // needs to load with acquire, and writes of the state (who always hold the
  // lock) need to store with release.  Anyone else reading the variable from
  // _inside_ of the lock, however, do not need to load with acquire.  The lock
  // is already satisfying any ordering requirements.
  //
  // So: TimerSyncVar lets us control access to the two variables (cancel_ and
  // active_cpu_) we have which are sometimes are called upon to provide
  // synchronization guarantees without relying on the guarantees provided by
  // the lock.  It enforces the following rules using the static analyzer.
  //
  // 1) Whenever a SyncVar is written, it is always stored with release semantics,
  //    and also demands that we hold the lock.
  // 2) Whenever a SyncVar is read from inside of the lock, it is loaded with
  //    relaxed semantics.  The fact that #1 happened with the lock held
  //    provides our synchronization guarantees.
  // 3) Whenever a SyncVar is read from outside of the lock, it is read using
  //    acquire semantics, which should syncronize-with the release-store
  //    performed in #1.
  //
  // Wrapping this all up in a class provides a good place for annotations, in
  // addition to guaranteeing that no one can make a mistake by directly
  // accessing the actual atomic with the wrong semantics.
  template <typename T>
  class TimerSyncVar {
   public:
    // No default constructor.  Explicitly initialize your PODs folks!
    explicit constexpr TimerSyncVar(T val) : val_{val} {}

    // No copy, no move.
    TimerSyncVar(const TimerSyncVar&) = delete;
    TimerSyncVar(TimerSyncVar&&) = delete;
    TimerSyncVar& operator=(const TimerSyncVar&) = delete;
    TimerSyncVar& operator=(TimerSyncVar&&) = delete;

    T load_locked() TA_REQ(TimerLock::Get()) { return val_.load(ktl::memory_order_relaxed); }
    T load_unlocked() TA_EXCL(TimerLock::Get()) { return val_.load(ktl::memory_order_acquire); }
    void store_locked(T val) TA_REQ(TimerLock::Get()) {
      val_.store(val, ktl::memory_order_release);
    }

   private:
    ktl::atomic<T> val_;
  };

  static constexpr uint32_t kMagic = fbl::magic("timr");
  uint32_t magic_ = kMagic;

  // This field should be interpreted differently depending on the clock_id_ field.
  // If clock_id_ is set to ZX_CLOCK_MONOTONIC, this is a zx_instant_mono_t.
  // If clock_id_ is set to ZX_CLOCK_BOOT, this is a zx_instant_boot_t.
  zx_time_t scheduled_time_ = 0;

  // Stores the applied slack adjustment from the ideal scheduled_time.
  zx_duration_t slack_ = 0;
  Callback callback_ = nullptr;
  void* arg_ = nullptr;

  // The clock this timer is set on.
  const zx_clock_t clock_id_;

  // INVALID_CPU, if inactive.
  TimerSyncVar<cpu_num_t> active_cpu_{INVALID_CPU};

  // true if cancel is pending
  TimerSyncVar<bool> cancel_{false};

  // Note that we need to manually name the timer_lock because it is one of and
  // extremely small number of "wrapped" locks in the system, and cannot easily
  // use lockdep in order to generate its name.
  //
  // The vast majority of locks in the system are directly instrumented using
  // lockdep. This causes the instance of the actual lock to become a member of a
  // generated lock dep class, which is used to access the underlying lock.  In
  // these cases, lockdep itself controls the construction sequencing of the lock,
  // allowing it to configure the internal lock's metadata immediately after
  // construction has completed.
  //
  // Wrapped locks, OTOH, are a bit different.  The lock instance is declared
  // outside of the lockdep generated class, which holds a reference to the lock
  // instead of encapsulating the lock itself.  This leads to a global ctor race:
  // Who is constructed first, the lock itself, or the lock wrapper who holds a
  // pointer/reference to the lock?
  //
  // In situations like this, it is not safe for lock wrapper to be interacting
  // with the lock whose reference it holds, since that lock may not have been
  // constructed yet.
  //
  // So instead, we simply manually name the lock, and the lockdep wrapper never
  // makes any attempt to name the lock because of the ordering issues.
  //
  static MonitoredSpinLock timer_lock __CPU_ALIGN_EXCLUSIVE;
  DECLARE_SINGLETON_LOCK_WRAPPER(TimerLock, timer_lock);
};

// Preemption Timers
//
// Each CPU has a dedicated preemption timer that's managed using specialized
// functions (prefixed with timer_preempt_).
//
// Preemption timers are different from general timers. Preemption timers:
//
// - are reset frequently by the scheduler so performance is important
// - should not be migrated off their CPU when the CPU is shutdown
//
// Note: A preemption timer may fire even after it has been canceled.
class TimerQueue {
 public:
  // Set/reset/cancel the preemption timer.
  //
  // When the preemption timer fires, Scheduler::TimerTick is called. Set the
  // deadline to ZX_TIME_INFINITE to cancel the preemption timer.
  // Scheduler::TimerTick may be called spuriously after cancellation.
  void PreemptReset(zx_instant_mono_t deadline);

  // Returns true if the preemption deadline is set and will definitely fire in
  // the future. A false value does not definitively mean the preempt timer will
  // not fire, as a spurious expiration is allowed.
  bool PreemptArmed() const { return preempt_timer_deadline_ != ZX_TIME_INFINITE; }

  // Internal routines used when bringing cpus online/offline

  // Moves |source|'s timers (except its preemption timer) to this TimerQueue.
  void TransitionOffCpu(TimerQueue& source);

  // Prints the contents of all timer queues into |buf| of length |len| and null
  // terminates |buf|.
  static void PrintTimerQueues(char* buf, size_t len);

  // This is called periodically by timer_tick(), which itself is invoked
  // periodically by some hardware timer.
  void Tick(cpu_num_t cpu);

  // UpdatePlatformTimer updates the platform's oneshot timer to the minimum of:
  // 1. The scheduled time of the head of the monotonic timer queue.
  // 2. The scheduled time of the head of the boot timer queue.
  // 3. The preemption timer deadline.
  //
  // This can only be called when interrupts are disabled.
  void UpdatePlatformTimer() TA_EXCL(Timer::TimerLock::Get());
  void UpdatePlatformTimerLocked() TA_REQ(Timer::TimerLock::Get());

 private:
  // Timers can directly call Insert and Cancel.
  friend class Timer;

  // Add |timer| to this TimerQueue, possibly coalescing deadlines as well.
  void Insert(Timer* timer, zx_time_t earliest_deadline, zx_time_t latest_deadline);

  // A helper function for Insert that inserts the given timer into the given timer list.
  static void InsertIntoTimerList(fbl::DoublyLinkedList<Timer*>& timer_list, Timer* timer,
                                  zx_time_t earliest_deadline, zx_time_t latest_deadline);

  // A helper function for TransitionOffCpu that moves all timers from the src_list to the
  // dst_list. Returns the Timer at the head of the dst_list if it changed, otherwise returns
  // nullopt.
  static ktl::optional<Timer*> TransitionTimerList(fbl::DoublyLinkedList<Timer*>& src_list,
                                                   fbl::DoublyLinkedList<Timer*>& dst_list);

  // A helper function for PrintTimerQueues that prints all of the timers in the given timer_list
  // into the given buffer. Also takes in the current time, which is either a zx_instant_mono_t or a
  // zx_instant_boot_t depending on the timeline the timer_list is operating on.
  template <typename TimestampType>
  static void PrintTimerList(TimestampType now, fbl::DoublyLinkedList<Timer*>& timer_list,
                             StringFile& buffer);

  // The UpdatePlatformTimer* methods are used to update the platform's oneshot timer to the
  // minimum of the existing deadline (stored in next_timer_deadline_) and the given new_deadline.
  // The two separate variations of this method are provided for convenience, so that callers can
  // provide either a montonic or a boot timestamp depending on the context they're operating in.
  // Using one of these functions is a bit more efficient than calling UpdatePlatformTimer(), as
  // they do not need to check the head of both the boot and monotonic timer queues before updating
  // the platform oneshot timer.
  //
  // These can only be called when interrupts are disabled.
  void UpdatePlatformTimerMono(zx_instant_mono_t new_deadline);
  void UpdatePlatformTimerBoot(zx_instant_boot_t new_deadline);

  // Converts the given monotonic timestamp to the raw ticks value it corresponds to.
  // Returns nullopt if the monotonic clock is paused.
  static ktl::optional<zx_ticks_t> ConvertMonotonicTimeToRawTicks(zx_instant_mono_t mono);

  // Converts the given boot timestamp to the raw ticks value it corresponds to.
  static zx_ticks_t ConvertBootTimeToRawTicks(zx_instant_boot_t boot);

  // This is called by Tick(), and processes all timers with scheduled times less than now.
  // Once it's done, the scheduled time of the timer at the front of the queue is returned.
  template <typename TimestampType>
  static void TickInternal(TimestampType now, cpu_num_t cpu,
                           fbl::DoublyLinkedList<Timer*>* timer_list);

  // Timers on the monotonic timeline are placed in this list.
  fbl::DoublyLinkedList<Timer*> monotonic_timer_list_ TA_GUARDED(Timer::TimerLock::Get());

  // Timers on the boot timeline are placed in this list.
  fbl::DoublyLinkedList<Timer*> boot_timer_list_ TA_GUARDED(Timer::TimerLock::Get());

  // This TimerQueue's preemption deadline. ZX_TIME_INFINITE means not set.
  zx_instant_mono_t preempt_timer_deadline_ = ZX_TIME_INFINITE;

  // This TimerQueue's deadline for its platform timer or ZX_TIME_INFINITE if not set.
  // The deadline is stored in raw platform ticks, as that is the unit used by the
  // platform_set_oneshot_timer API.
  zx_ticks_t next_timer_deadline_ = ZX_TIME_INFINITE;
};

#endif  // ZIRCON_KERNEL_INCLUDE_KERNEL_TIMER_H_
