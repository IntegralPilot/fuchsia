// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_SUSPEND_WAKEUP_TIMER_INCLUDE_LIB_SUSPEND_WAKEUP_TIMER_H_
#define ZIRCON_KERNEL_LIB_SUSPEND_WAKEUP_TIMER_INCLUDE_LIB_SUSPEND_WAKEUP_TIMER_H_

#include <lib/relaxed_atomic.h>
#include <stdint.h>
#include <zircon/time.h>

#include <kernel/spinlock.h>
#include <ktl/unique_ptr.h>
#include <ktl/utility.h>

// # SuspendWakeupTimer
//
// ## Summary
//
// A class which helps to abstract the specific timer HW used to wake the system
// from a suspended state.  Currently meant to be used only by the
// IdlePowerThread.
//
// ## Creation
// Users of the SuspendWakeupTimer start by creating a timer instance using the
// static Create factory function.  During creation, a Callback instance
// must be passed.
//
// The Callback is a fit::inline_function with enough storage for exactly one
// pointer, allowing it to easily target either a static or instanced method of
// a class.  It takes two parameters, now and resume_at, both timestamps on
// the boot timeline which inform the callback owner of when the interrupt
// fired, as well as what the originally requested resume time was.
//
// The lifecycle of the SuspendWakeupTimer is controlled by a unique_ptr
// returned from the Create function.  Create must not be called earlier
// than LK_INIT_LEVEL_PLATFORM to allow platform specific wakeup timer
// hardware to be detected and initialized.  Callbacks will always take place on
// the BOOT_CPU and with interrupts off.
//
// ## Usage
//
// Usage of the SuspendWakeupTimer involves three methods called in accordance
// with a protocol which must be followed to ensure correct behavior.  The
// methods are:
//
// + SetResumeDeadline
// + CancelTimer
// + EnsureStarted
//
// At the start of a suspend operation, if the operation has a resume deadline,
// SetResumeDeadline must be called to configure the desired deadline before
// instructing the BOOT_CPU's IdlePowerThread to transition to the suspended
// state.  Once a resume deadline has been configured, it is illegal to call
// SetResumeDeadline again without first explicitly canceling the timer via
// CancelTimer.
//
// After the resume deadline has been configured, and as the suspend coordinator
// thread unwinds from the suspend operation for any reason (resume timer fired,
// some other wake source fired, failure to enter suspend due to an unrelated
// error), CancelTimer *must* be called in order to reset the timer, and
// guarantee that there are no timer callbacks in flight.  CancelTimer is
// idempotent.  Unlike SetResumeDeadline, it may be called any number of times
// in a row.  Once CancelTimer has been called, it is guaranteed that there are
// no longer any callbacks in flight, however it does *not* guarantee that the
// user won the race to cancel a timer which had been set up, but may not have
// fired yet.
//
// Finally, in the BOOT_CPU's IdlePowerThread itself, when the system is
// supposed to be in the suspend state, it *must* call EnsureStarted to make
// certain that the hardware is properly set up to deliver an interrupt which
// will wake the system at an appropriate time.  This call *must* be made from
// the BOOT_CPU and interrupts *must* be disabled when this happens.  Similar to
// CancelTimer, EnsureStarted is idempotent and may multiple times without a
// problem.  If there is a configured deadline and the timer is not yet started,
// it will be.
//
// So, the summary of the usage protocol is as follows.
//
// 1) At the start of a suspend operation call SetResumeDeadline to configure
//    the time to wake up and resume the system.
// 2) In the BOOT_CPU's IdlePowerThread itself, just before entering a suspended
//    state and with interrupts still disabled, call EnsureStarted to make sure
//    that the timer is running if it needs to be.
// 3) After control is returned to the coordinator thread, always call
//    CancelTimer as we unwind.
//
// ## Restrictions
//
// SuspendWakeupTimer callbacks will take place at hard IRQ time.  Operations
// which may need to block (such as holding a Mutex) are not allowed.  That
// said, no spinlocks are held during the callback itself.  It is safe for the
// callback to call methods on the SuspendWakeupTimer instance itself, however
// it is anticipated that there is little to no reason to do so.  Keep in mind,
// however, that it is illegal to call SetResumeDeadline a second time without
// having canceled the timer first.  If a callback _wanted_ to re-program its
// deadline during the operation itself, `CancelTimer` must still be called
// first to ensure that the timer is ready to be set up again.
//
class SuspendWakeupTimer {
 public:
  using Callback =
      fit::inline_function<void(zx_instant_boot_t now, zx_instant_boot_t resume_at), sizeof(void*)>;
  virtual ~SuspendWakeupTimer() = default;

  static ktl::unique_ptr<SuspendWakeupTimer> Create(Callback callback);

  // Sets the time at which the SuspendWakeupTimer should call the user's
  // callback in order to wake the system from suspend.  Note that once this has
  // been called and the timer has an assigned deadline, it is illegal to call
  // this method again without first calling CancelTimer.
  void SetResumeDeadline(zx_instant_boot_t resume_at) {
    Guard<SpinLock, IrqSave> timer_guard{&lock_};
    DEBUG_ASSERT(!started_);
    DEBUG_ASSERT(resume_at_ == ZX_TIME_INFINITE);
    resume_at_ = resume_at;
  }

  // Called by the BOOT_CPU's IdlePowerThread every time it is just about to
  // enter a suspended state to ensure that its SuspendWakeupTimer is started
  // and will wake it up at the appropriate time.
  virtual void EnsureStarted() = 0;

  // Called to cancel any pending timer and its callback.  Must be called at
  // least once after every call to SetResumeDeadline before a second call to
  // SetResumeDeadline may be made.  After returning from CancelTimer, the user
  // is guaranteed that there the registered callback is no longer in flight.
  // It has either completed, or was successfully canceled.
  virtual void CancelTimer() = 0;

 protected:
  SuspendWakeupTimer(Callback callback) : callback_(ktl::move(callback)) {}

  SuspendWakeupTimer(const SuspendWakeupTimer&) = delete;
  SuspendWakeupTimer& operator=(const SuspendWakeupTimer&) = delete;
  SuspendWakeupTimer(SuspendWakeupTimer&&) = delete;
  SuspendWakeupTimer& operator=(SuspendWakeupTimer&&) = delete;

  void ResetLocked() TA_REQ(lock_) {
    started_ = false;
    resume_at_ = ZX_TIME_INFINITE;
  }

  void DoCallback(zx_instant_boot_t now) {
    // Don't hold any locks when calling our callback.  Our underlying timers
    // don't hold any locks, and if we don't hold any locks either, it becomes
    // safe for the callback to call its own timer instance's methods while
    // executing.
    //
    // But, why is it safe for us to read `resume_at_` then?  What is to prevent
    // a callback from running on the boot CPU and attempting to read
    // resume_at_, while at the same time someone is calling cancel which is
    // resetting resume_at_ back to ZX_TIME_INFINITE?  Isn't a formal data race?
    //
    // Turns out, the answer is "no", but the reason why is not all that
    // obvious.  There are 4 operations we need to consider, the 3 public
    // interface methods and the callback itself.
    //
    // The 3 public interface methods all hold the timer's spinlock as they read
    // and write.  The mutually exclusive nature of the lock guarantees that
    // there are no formal races, and that the value observed will always be
    // "correct" for the internal state of the timer.  So, this leaves the
    // callback itself, and its interaction with the 3 public methods, to
    // consider.
    //
    // + CancelTimer: Cancel operations will mutate the resume_at_ member of the
    //   timer, but before doing so, they will call "cancel" on their underlying
    //   timer.  All existing low level timer implementations guarantee that,
    //   "after cancel is called, any callback in flight has been successfully
    //   canceled and will not run --or--, the callback has already run to
    //   completion.  So, if the callback is either finished or never will be
    //   run, and then resume_at_ is mutated, there is no possibility that the
    //   value can be written (from the method) while concurrently being read by
    //   the callback.  So, no formal race.
    //
    // + SetResumeDeadline: It is illegal for a user to set a new resume
    //   deadline without first having called CancelTimer.  SetResumeDeadline
    //   will obtain lock_, synchronizing it with any CancelTimer operations,
    //   before checking the value in resume_at_, and will ASSERT if the timer
    //   has not been first canceled.  So, if there is a timer callback in
    //   flight, cancel has not been called, and the system will panic (because
    //   of the protocol violation).  If cancel _has_ been called, then there is
    //   no timer in flight, and a callback cannot become in-fight for the
    //   duration of the Set operation (because holding lock_ prevents
    //   EnsureStarted from being called concurrently).
    //
    // + EnsureStarted: Holds the lock, but never mutates resume_at_, meaning
    //   that there is no possibility of a formal race.
    //
    // Please note: even though there is no chance of a formal race here, we
    // still declare resume_at_ as a RelaxedAtomic.  Why?  Because there is no
    // real penalty to be paid for relaxed atomic access to variables on the
    // architectures we support, and it makes it easier for a reader to know
    // that there is no chance of a formal data race by simply observing the
    // atomic nature of the member, without having to understand the more
    // complex chain of reasoning given above.
    //
    callback_(now, resume_at_);
  }

  DECLARE_SPINLOCK(SuspendWakeupTimer) lock_;
  bool started_ TA_GUARDED(lock_){false};
  RelaxedAtomic<zx_instant_boot_t> resume_at_{ZX_TIME_INFINITE};
  const Callback callback_;
};

#endif  // ZIRCON_KERNEL_LIB_SUSPEND_WAKEUP_TIMER_INCLUDE_LIB_SUSPEND_WAKEUP_TIMER_H_
