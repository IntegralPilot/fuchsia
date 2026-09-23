// Copyright 2016 The Fuchsia Authors
// Copyright (c) 2008-2014 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

/**
 * @file
 * @brief  Kernel timer subsystem
 * @defgroup timer Timers
 *
 * The timer subsystem allows functions to be scheduled for later
 * execution.  Each timer object is used to cause one function to
 * be executed at a later time.
 *
 * Timer callback functions are called in interrupt context.
 *
 * @{
 */
#include "kernel/timer.h"

#include <align.h>
#include <assert.h>
#include <debug.h>
#include <inttypes.h>
#include <lib/affine/ratio.h>
#include <lib/arch/intrin.h>
#include <lib/counters.h>
#include <lib/kconcurrent/chainlock_transaction.h>
#include <lib/page/size.h>
#include <lib/zircon-internal/macros.h>
#include <platform.h>
#include <stdlib.h>
#include <trace.h>
#include <zircon/compiler.h>
#include <zircon/errors.h>
#include <zircon/time.h>
#include <zircon/types.h>

#include <cstdio>

#include <kernel/lockdep.h>
#include <kernel/mp.h>
#include <kernel/percpu.h>
#include <kernel/scheduler.h>
#include <kernel/spinlock.h>
#include <kernel/stats.h>
#include <kernel/thread.h>
#include <platform/timer.h>

#define LOCAL_TRACE 0

// Total number of timers set. Always increasing.
KCOUNTER(timer_created_counter, "timer.created")

// Number of timers merged into an existing timer because of slack.
KCOUNTER(timer_coalesced_counter, "timer.coalesced")

// Number of timers that have fired (i.e. callback was invoked).
KCOUNTER(timer_fired_counter, "timer.fired")

// Number of timers that were successfully canceled. Attempts to cancel a timer that is currently
// firing are not counted.
KCOUNTER(timer_canceled_counter, "timer.canceled")

// static
MonitoredSpinLock Timer::timer_lock __CPU_ALIGN_EXCLUSIVE{"timer_lock"_intern};

namespace {

affine::Ratio gTicksToTime;
uint64_t gTicksPerSecond;

}  // anonymous namespace

void timer_set_ticks_to_time_ratio(const affine::Ratio& ticks_to_time) {
  // ASSERT that we are not calling this function twice.  Once set, this ratio
  // may not change.
  DEBUG_ASSERT(gTicksPerSecond == 0);
  DEBUG_ASSERT(ticks_to_time.numerator() != 0);
  DEBUG_ASSERT(ticks_to_time.denominator() != 0);
  gTicksToTime = ticks_to_time;
  gTicksPerSecond = gTicksToTime.Inverse().Scale(ZX_SEC(1));
}

const affine::Ratio& timer_get_ticks_to_time_ratio(void) { return gTicksToTime; }

zx_instant_mono_t current_mono_time(void) { return gTicksToTime.Scale(current_mono_ticks()); }

zx_instant_boot_t current_boot_time(void) { return gTicksToTime.Scale(current_boot_ticks()); }

InstantMonoAndBootTime current_mono_and_boot_time() {
  CurrentTicksObservation observation = timer_current_mono_and_boot_ticks();
  return {.mono_time = gTicksToTime.Scale(observation.mono_now),
          .boot_time = gTicksToTime.Scale(observation.boot_now)};
}

InstantMonoTimeAndBootTicks current_mono_time_and_boot_ticks() {
  CurrentTicksObservation observation = timer_current_mono_and_boot_ticks();
  return {.mono_time = gTicksToTime.Scale(observation.mono_now),
          .boot_ticks = observation.boot_now};
}

zx_ticks_t ticks_per_second(void) { return gTicksPerSecond; }

ktl::optional<zx_ticks_t> TimerQueue::ConvertMonotonicTimeToRawTicks(zx_instant_mono_t mono) {
  // Do not attempt to convert the sentinel value of ZX_TIME_INFINITE.
  if (mono == ZX_TIME_INFINITE) {
    return ZX_TIME_INFINITE;
  }
  const zx_ticks_t deadline_mono_ticks =
      timer_get_ticks_to_time_ratio().Inverse().Scale<affine::Ratio::Round::Up>(mono);
  return timer_convert_mono_to_raw_ticks(deadline_mono_ticks);
}

zx_ticks_t TimerQueue::ConvertBootTimeToRawTicks(zx_instant_boot_t boot) {
  // Do not attempt to convert the sentinel value of ZX_TIME_INFINITE.
  if (boot == ZX_TIME_INFINITE) {
    return ZX_TIME_INFINITE;
  }
  const zx_ticks_t deadline_boot_ticks =
      timer_get_ticks_to_time_ratio().Inverse().Scale<affine::Ratio::Round::Up>(boot);
  return zx_ticks_sub_ticks(deadline_boot_ticks, timer_get_boot_ticks_offset());
}

void TimerQueue::UpdatePlatformTimer() {
  Guard<MonitoredSpinLock, NoIrqSave> guard{Timer::TimerLock::Get(), SOURCE_TAG};
  UpdatePlatformTimerLocked();
}

void TimerQueue::UpdatePlatformTimerLocked() {
  DEBUG_ASSERT(arch_ints_disabled());
  zx_ticks_t timer_deadline = ZX_TIME_INFINITE;

  // The monotonic deadline should be the minimum of the preemption timer and the front of the
  // monotonic timer list.
  zx_instant_mono_t mono_time_deadline = preempt_timer_deadline_;
  if (!monotonic_timer_list_.is_empty()) {
    mono_time_deadline =
        ktl::min(mono_time_deadline, monotonic_timer_list_.front().scheduled_time_);
  }
  const ktl::optional<zx_ticks_t> mono_ticks_deadline =
      ConvertMonotonicTimeToRawTicks(mono_time_deadline);
  if (mono_ticks_deadline.has_value()) {
    timer_deadline = mono_ticks_deadline.value();
  }

  // Check if we have a boot timer with a sooner scheduled time and update the timer deadline
  // accordingly.
  if (!boot_timer_list_.is_empty()) {
    const zx_ticks_t boot_deadline =
        ConvertBootTimeToRawTicks(boot_timer_list_.front().scheduled_time_);
    timer_deadline = ktl::min(timer_deadline, boot_deadline);
  }

  // Update the platform oneshot timer to the resulting timer deadline.
  LTRACEF("rescheduling timer for %" PRIi64 " ticks from UpdatePlatformTimer\n", timer_deadline);
  platform_set_oneshot_timer(timer_deadline);
  next_timer_deadline_ = timer_deadline;
}

void TimerQueue::UpdatePlatformTimerBoot(zx_instant_boot_t new_deadline) {
  DEBUG_ASSERT(arch_ints_disabled());

  // Do not set the platform timer if we were passed an infinite deadline.
  if (new_deadline == ZX_TIME_INFINITE) {
    return;
  }

  const zx_ticks_t deadline_raw_ticks = ConvertBootTimeToRawTicks(new_deadline);
  if (deadline_raw_ticks < next_timer_deadline_) {
    LTRACEF("rescheduling timer for %" PRIi64 " ticks from UpdatePlatformTimerBoot, next: %" PRIi64
            " \n",
            deadline_raw_ticks, next_timer_deadline_);
    platform_set_oneshot_timer(deadline_raw_ticks);
    next_timer_deadline_ = deadline_raw_ticks;
  }
}

void TimerQueue::UpdatePlatformTimerMono(zx_instant_mono_t new_deadline) {
  DEBUG_ASSERT(arch_ints_disabled());

  // Do not set the platform timer if we were passed an infinite deadline.
  if (new_deadline == ZX_TIME_INFINITE) {
    return;
  }

  // Convert from monotonic time to a raw ticks value to set the platform timer to.
  const ktl::optional<zx_ticks_t> deadline_raw_ticks = ConvertMonotonicTimeToRawTicks(new_deadline);
  // Return early if the monotonic clock is paused.
  if (!deadline_raw_ticks.has_value()) {
    return;
  }

  if (deadline_raw_ticks.value() < next_timer_deadline_) {
    LTRACEF("rescheduling timer for %" PRIi64 " ticks from UpdatePlatformTimerMono, next: %" PRIi64
            "\n",
            deadline_raw_ticks.value(), next_timer_deadline_);
    platform_set_oneshot_timer(deadline_raw_ticks.value());
    next_timer_deadline_ = deadline_raw_ticks.value();
  }
}

void TimerQueue::Insert(Timer* timer, zx_time_t earliest_deadline, zx_time_t latest_deadline) {
  DEBUG_ASSERT(arch_ints_disabled());
  LTRACEF("timer %p, cpu %u, scheduled %" PRIi64 "\n", timer, arch_curr_cpu_num(),
          timer->scheduled_time_);
  fbl::DoublyLinkedList<Timer*>& timer_list =
      timer->clock_id_ == ZX_CLOCK_MONOTONIC ? monotonic_timer_list_ : boot_timer_list_;
  InsertIntoTimerList(timer_list, timer, earliest_deadline, latest_deadline);
}

void TimerQueue::InsertIntoTimerList(fbl::DoublyLinkedList<Timer*>& timer_list, Timer* timer,
                                     zx_time_t earliest_deadline, zx_time_t latest_deadline) {
  // For inserting the timer we consider several cases. In general we
  // want to coalesce with the current timer unless we can prove that
  // either that:
  //  1- there is no slack overlap with current timer OR
  //  2- the next timer is a better fit.
  //
  // In diagrams that follow
  // - Let |e| be the current (existing) timer deadline
  // - Let |t| be the deadline of the timer we are inserting
  // - Let |n| be the next timer deadline if any
  // - Let |x| be the end of the list (not a timer)
  // - Let |(| and |)| the earliest_deadline and latest_deadline.

  for (Timer& entry : timer_list) {
    if (entry.scheduled_time_ > latest_deadline) {
      // New timer latest is earlier than the current timer.
      // Just add upfront as is, without slack.
      //
      //   ---------t---)--e-------------------------------> time
      timer->slack_ = 0ll;
      timer_list.insert(entry, timer);
      return;
    }

    if (entry.scheduled_time_ >= timer->scheduled_time_) {
      //  New timer slack overlaps and is to the left (or equal). We
      //  coalesce with current by scheduling late.
      //
      //  --------(----t---e-)----------------------------> time
      timer->slack_ = zx_time_sub_time(entry.scheduled_time_, timer->scheduled_time_);
      timer->scheduled_time_ = entry.scheduled_time_;
      kcounter_add(timer_coalesced_counter, 1);
      timer_list.insert_after(timer_list.make_iterator(entry), timer);
      return;
    }

    if (entry.scheduled_time_ < earliest_deadline) {
      // new timer earliest is later than the current timer. This case
      // is handled in a future iteration.
      //
      //   ----------------e--(---t-----------------------> time
      continue;
    }

    // New timer is to the right of current timer and there is overlap
    // with the current timer, but could the next timer (if any) be
    // a better fit?
    //
    //  -------------(--e---t-----?-------------------> time

    auto iter = timer_list.make_iterator(entry);
    ++iter;
    if (iter != timer_list.end()) {
      const Timer& next = *iter;
      if (next.scheduled_time_ <= timer->scheduled_time_) {
        // The new timer is to the right of the next timer. There is no
        // chance the current timer is a better fit.
        //
        //  -------------(--e---n---t----------------------> time
        continue;
      }

      if (next.scheduled_time_ < latest_deadline) {
        // There is slack overlap with the next timer, and also with the
        // current timer. Which coalescing is a better match?
        //
        //  --------------(-e---t---n-)-----------------------> time
        zx_duration_t delta_entry = zx_time_sub_time(timer->scheduled_time_, entry.scheduled_time_);
        zx_duration_t delta_next = zx_time_sub_time(next.scheduled_time_, timer->scheduled_time_);
        if (delta_next < delta_entry) {
          // New timer is closer to the next timer, handle it in the
          // next iteration.
          continue;
        }
      }
    }

    // Handles the remaining cases, note that there is overlap with
    // the current timer.
    //
    //  1- this is the last timer (next == NULL) or
    //  2- there is no overlap with the next timer, or
    //  3- there is overlap with both current and next but
    //     current is closer.
    //
    //  So we coalesce by scheduling early.
    timer->slack_ = zx_time_sub_time(entry.scheduled_time_, timer->scheduled_time_);
    timer->scheduled_time_ = entry.scheduled_time_;
    kcounter_add(timer_coalesced_counter, 1);
    timer_list.insert_after(timer_list.make_iterator(entry), timer);
    return;
  }

  // Walked off the end of the list and there was no overlap.
  timer->slack_ = 0;
  timer_list.push_back(timer);
}

Timer::~Timer() {
  // Ensure that we are not on any TimerQueue's list.
  ZX_DEBUG_ASSERT(!InContainer());
  // Ensure that we are not active on some cpu.
  ZX_DEBUG_ASSERT(active_cpu_.load_unlocked() == INVALID_CPU);
}

void Timer::Set(const Deadline& deadline, Callback callback, void* arg) {
  LTRACEF("timer %p deadline.when %" PRIi64 " deadline.slack.amount %" PRIi64
          " deadline.slack.mode %u callback %p arg %p\n",
          this, deadline.when(), deadline.slack().amount(), deadline.slack().mode(), callback, arg);

  DEBUG_ASSERT(magic_ == kMagic);
  DEBUG_ASSERT(deadline.slack().mode() <= TIMER_SLACK_LATE);
  DEBUG_ASSERT(deadline.slack().amount() >= 0);

  const zx_time_t latest_deadline = deadline.latest();
  const zx_time_t earliest_deadline = deadline.earliest();

  Guard<MonitoredSpinLock, IrqSave> guard{TimerLock::Get(), SOURCE_TAG};

  if (InContainer()) {
    panic("timer %p already in list\n", this);
  }

  cpu_num_t cpu = arch_curr_cpu_num();
  cpu_num_t active_cpu = active_cpu_.load_locked();

  bool currently_active = (active_cpu == cpu);
  if (unlikely(currently_active)) {
    // The timer is active on our own cpu, we must be inside the callback.
    if (cancel_.load_locked()) {
      return;
    }
  } else if (unlikely(active_cpu != INVALID_CPU)) {
    panic("timer %p currently active on a different cpu %u\n", this, active_cpu);
  }

  // Set up the structure.
  scheduled_time_ = deadline.when();
  callback_ = callback;
  arg_ = arg;
  cancel_.store_locked(false);
  // We don't need to modify active_cpu_ because it is managed by timer_tick().

  LTRACEF("scheduled time %" PRIi64 "\n", scheduled_time_);

  TimerQueue& timer_queue = percpu::Get(cpu).timer_queue;
  timer_queue.Insert(this, earliest_deadline, latest_deadline);

  switch (clock_id_) {
    case ZX_CLOCK_MONOTONIC:
      if (!timer_queue.monotonic_timer_list_.is_empty() &&
          &timer_queue.monotonic_timer_list_.front() == this) {
        timer_queue.UpdatePlatformTimerMono(deadline.when());
      }
      break;
    case ZX_CLOCK_BOOT:
      if (!timer_queue.boot_timer_list_.is_empty() &&
          &timer_queue.boot_timer_list_.front() == this) {
        timer_queue.UpdatePlatformTimerBoot(deadline.when());
      }
      break;
  }
  kcounter_add(timer_created_counter, 1);
}

void TimerQueue::PreemptReset(zx_instant_mono_t deadline) {
  DEBUG_ASSERT(arch_ints_disabled());
  LTRACEF("preempt timer cpu %u deadline %" PRIi64 "\n", arch_curr_cpu_num(), deadline);
  preempt_timer_deadline_ = deadline;
  UpdatePlatformTimerMono(deadline);
}

bool Timer::Cancel() {
  DEBUG_ASSERT(magic_ == kMagic);

  Guard<MonitoredSpinLock, IrqSave> guard{TimerLock::Get(), SOURCE_TAG};

  cpu_num_t cpu = arch_curr_cpu_num();

  // Mark the timer as canceled. This visible state change allows other CPUs
  // (e.g., in TrylockOrCancel) to detect that the timer is being cancelled and
  // abort operations early.
  cancel_.store_locked(true);
  // TODO(https://fxbug.dev/42142666): Consider whether this DeviceMemoryBarrier
  // is required
  arch::DeviceMemoryBarrier();

  // Check if we are attempting to cancel the timer from within its own callback
  // on the current CPU. If so, we cannot wait for the timer to become un-busy
  // (as that would deadlock). Instead, we clear the callback and return false,
  // indicating that the callback was running.
  if (unlikely(active_cpu_.load_locked() == cpu)) {
    // Zero out as defense-in-depth so that incorrect reuse results in a null
    // dereference rather than executing stale code.
    callback_ = nullptr;
    arg_ = nullptr;

    // We're done, so return back to the callback.
    return false;
  }

  bool callback_not_running;

  // If this Timer is in a queue, remove it and adjust hardware timers if
  // needed.
  if (InContainer()) {
    callback_not_running = true;

    TimerQueue& timer_queue = percpu::Get(cpu).timer_queue;

    // Save the old head of the *current* CPU's queue so later we can see if we
    // modified the head of that queue and need to update the local hardware
    // timer.
    const Timer* oldhead = nullptr;
    fbl::DoublyLinkedList<Timer*>& timer_list = clock_id_ == ZX_CLOCK_MONOTONIC
                                                    ? timer_queue.monotonic_timer_list_
                                                    : timer_queue.boot_timer_list_;
    if (!timer_list.is_empty()) {
      oldhead = &timer_list.front();
    }

    // Remove this Timer from whatever TimerQueue it's on, which may not be the
    // current CPU's queue.
    RemoveFromContainer();
    kcounter_add(timer_canceled_counter, 1);

    // TODO(cpu): If, after removing |timer| there is one other single Timer with
    // the same scheduled_time_ and slack_ non-zero, then it is possible to return
    // that timer to the ideal scheduled_time_.

    // If the timer was the head of the *current* CPU's queue, we need to update
    // the hardware timer for this CPU.
    //
    // Note on cross-CPU cancellation: If the timer was on *another* CPU's
    // queue, we do not update that CPU's hardware timer here. The hardware
    // timer on that CPU will still fire at the scheduled time. When it does,
    // the tick handler will notice the queue head has changed (or is empty) and
    // will update the hardware timer accordingly. This "lazy" update avoids the
    // need to IPI the other CPU to update its hardware timer.
    if (unlikely(oldhead == this)) {
      // The Timer we're canceling was at head of this queue, so see if we
      // should update platform timer.
      if (!timer_list.is_empty()) {
        timer_queue.UpdatePlatformTimerLocked();
      } else if (timer_queue.next_timer_deadline_ == ZX_TIME_INFINITE) {
        LTRACEF("clearing old hw timer, preempt timer not set, nothing in the queue\n");
        platform_stop_timer();
      }
    }
  } else {
    // The timer was not in a queue. This means it either has already fired,
    // or is currently firing on another CPU.
    callback_not_running = false;
  }

  guard.Release();

  // Wait for the timer to become un-busy in case a callback is currently active
  // on another cpu. This ensures that after Cancel() returns, it is safe to
  // destroy the timer or its arguments.
  while (active_cpu_.load_unlocked() != INVALID_CPU) {
    arch::Yield();
  }

  // Zero out as defense-in-depth so that incorrect reuse results in a null
  // dereference rather than executing stale code.
  callback_ = nullptr;
  arg_ = nullptr;

  return callback_not_running;
}

// called at interrupt time to process any pending timers
void timer_tick() {
  DEBUG_ASSERT(arch_ints_disabled());

  CPU_STATS_INC(timer_ints);

  cpu_num_t cpu = arch_curr_cpu_num();
  percpu::Get(cpu).timer_queue.Tick(cpu);
}

void TimerQueue::Tick(cpu_num_t cpu) {
  zx_instant_mono_t now = current_mono_time();
  zx_instant_boot_t boot_now = current_boot_time();
  LTRACEF("cpu %u now %" PRIi64 ", sp %p\n", cpu, now, __GET_FRAME());

  // The platform timer has fired, so no deadline is set.
  next_timer_deadline_ = ZX_TIME_INFINITE;

  // Service the preemption timer before acquiring the timer lock.
  if (now >= preempt_timer_deadline_) {
    preempt_timer_deadline_ = ZX_TIME_INFINITE;
    Scheduler::TimerTick(SchedTime{now});
  }

  // Tick both of the timer lists.
  TickInternal(now, cpu, &monotonic_timer_list_);
  TickInternal(boot_now, cpu, &boot_timer_list_);

  // Update the platform timer.
  UpdatePlatformTimer();
}

template <typename TimestampType>
void TimerQueue::TickInternal(TimestampType now, cpu_num_t cpu,
                              fbl::DoublyLinkedList<Timer*>* timer_list) {
  Guard<MonitoredSpinLock, NoIrqSave> guard{Timer::TimerLock::Get(), SOURCE_TAG};

  for (;;) {
    // See if there's an event to process.
    if (timer_list->is_empty()) {
      break;
    }

    Timer& timer = timer_list->front();

    LTRACEF("next item on timer queue %p at %" PRIi64 " now %" PRIi64 " (%p, arg %p)\n", &timer,
            timer.scheduled_time_, now, timer.callback_, timer.arg_);
    if (likely(now < timer.scheduled_time_)) {
      break;
    }

    // Process it.
    LTRACEF("timer %p\n", &timer);
    DEBUG_ASSERT_MSG(timer.magic_ == Timer::kMagic,
                     "ASSERT: timer failed magic check: timer %p, magic 0x%x\n", &timer,
                     (uint)timer.magic_);
    timer_list->erase(timer);

    // Mark the timer busy.
    timer.active_cpu_.store_locked(cpu);
    // Unlocking the spinlock in CallUnlocked acts as a release fence.

    // Now that the timer is off of the list, release the spinlock to handle
    // the callback, then re-acquire in case it is requeued.
    guard.CallUnlocked([&timer, now]() {
      LTRACEF("dequeued timer %p, scheduled %" PRIi64 "\n", &timer, timer.scheduled_time_);

      CPU_STATS_INC(timers);
      kcounter_add(timer_fired_counter, 1);

      LTRACEF("timer %p firing callback %p, arg %p\n", &timer, timer.callback_, timer.arg_);
      timer.callback_(&timer, now, timer.arg_);

      DEBUG_ASSERT(arch_ints_disabled());
    });

    // Mark it not busy.
    timer.active_cpu_.store_locked(INVALID_CPU);
    // TODO(https://fxbug.dev/42142666): Consider whether this DeviceMemoryBarrier is required
    arch::DeviceMemoryBarrier();
  }

  // Verify that the head of the timer queue has a scheduled time after now.
  if (!timer_list->is_empty()) {
    DEBUG_ASSERT(timer_list->front().scheduled_time_ > now);
  }
}

zx_status_t Timer::TrylockOrCancel(MonitoredSpinLock* lock) {
  // spin trylocking on the passed in spinlock either waiting for it
  // to grab or the passed in timer to be canceled.
  while (unlikely(lock->TryAcquire(SOURCE_TAG))) {
    // we failed to grab it, check for cancel
    if (cancel_.load_unlocked()) {
      // we were canceled, so bail immediately
      return ZX_ERR_TIMED_OUT;
    }
    // tell the arch to wait
    arch::Yield();
  }

  return ZX_OK;
}

zx_status_t Timer::TrylockOrCancel(ChainLock& lock) {
  // Just like the previous implementation of TrylockOrCancel, but this version
  // attempts operates on a ChainLock instead of a MonitoredSpinLock, and is
  // used by the wait queue timeout mechanism.  Keep attempting to acquire the
  // lock, arch:Yield'ing in between attempts, and bailing out in the case that
  // we notice that the cancel_ flag has been set.
  while (!lock.TryAcquire()) {
    if (cancel_.load_unlocked()) {
      return ZX_ERR_TIMED_OUT;
    }
    arch::Yield();
  }

  return ZX_OK;
}

ktl::optional<Timer*> TimerQueue::TransitionTimerList(fbl::DoublyLinkedList<Timer*>& src_list,
                                                      fbl::DoublyLinkedList<Timer*>& dst_list) {
  // Keep track of what the first timer in the dst_list was.
  Timer* old_head = nullptr;
  if (!dst_list.is_empty()) {
    old_head = &dst_list.front();
  }

  // Move all the timers from the src_list to the dst_list.
  Timer* timer;
  while ((timer = src_list.pop_front()) != nullptr) {
    // We lost the original asymmetric slack information so when we combine them
    // with the other timer queue they are not coalesced again.
    // TODO(cpu): figure how important this case is.
    InsertIntoTimerList(dst_list, timer, timer->scheduled_time_, timer->scheduled_time_);
    // Note, we do not increment the "created" counter here because we are simply moving these
    // timers from one queue to another and we already counted them when they were first
    // created.
  }
  Timer* new_head = nullptr;
  if (!dst_list.is_empty()) {
    new_head = &dst_list.front();
  }

  // If the head of the timer list changed, then we need to return the new head.
  if (new_head != nullptr && new_head != old_head) {
    return ktl::optional<Timer*>(new_head);
  }
  return ktl::nullopt;
}

void TimerQueue::TransitionOffCpu(TimerQueue& source) {
  Guard<MonitoredSpinLock, IrqSave> guard{Timer::TimerLock::Get(), SOURCE_TAG};

  // Transition both timer lists. This may update the platform timer.
  const ktl::optional<Timer*> new_mono_head =
      TransitionTimerList(source.monotonic_timer_list_, monotonic_timer_list_);
  if (new_mono_head) {
    UpdatePlatformTimerMono(new_mono_head.value()->scheduled_time_);
  }
  const ktl::optional<Timer*> new_boot_head =
      TransitionTimerList(source.boot_timer_list_, boot_timer_list_);
  if (new_boot_head) {
    UpdatePlatformTimerBoot(new_boot_head.value()->scheduled_time_);
  }

  // The old TimerQueue has no tasks left, so reset the deadlines.
  source.preempt_timer_deadline_ = ZX_TIME_INFINITE;
  source.next_timer_deadline_ = ZX_TIME_INFINITE;
}

template <typename TimestampType>
void TimerQueue::PrintTimerList(TimestampType now, fbl::DoublyLinkedList<Timer*>& timer_list,
                                StringFile& buffer) {
  TimestampType last = now;
  for (Timer& t : timer_list) {
    zx_duration_t delta_now = zx_time_sub_time(t.scheduled_time_, now);
    zx_duration_t delta_last = zx_time_sub_time(t.scheduled_time_, last);
    fprintf(&buffer,
            "\ttime %" PRIi64 " delta_now %" PRIi64 " delta_last %" PRIi64 " func %p arg %p\n",
            t.scheduled_time_, delta_now, delta_last, t.callback_, t.arg_);
    last = t.scheduled_time_;
  }
}

void TimerQueue::PrintTimerQueues(char* buf, size_t len) {
  StringFile buffer{ktl::span(buf, len)};
  Guard<MonitoredSpinLock, IrqSave> guard{Timer::TimerLock::Get(), SOURCE_TAG};
  for (cpu_num_t i = 0; i < percpu::processor_count(); i++) {
    if (mp_is_cpu_online(i)) {
      fprintf(&buffer, "cpu %u:\n", i);
      PrintTimerList(current_mono_time(), percpu::Get(i).timer_queue.monotonic_timer_list_, buffer);
      fprintf(&buffer, "boot timers:\n");
      PrintTimerList(current_boot_time(), percpu::Get(i).timer_queue.boot_timer_list_, buffer);
    }
  }
  // Null terminate the buffer.
  ktl::move(buffer).take();
}

#include <lib/console.h>

static int cmd_timers(int argc, const cmd_args* argv, uint32_t flags) {
  const size_t timer_buffer_size = kPageSize;

  // allocate a buffer to dump the timer queue into to avoid reentrancy issues with the
  // timer spinlock
  char* buf = static_cast<char*>(malloc(timer_buffer_size));
  if (!buf) {
    return ZX_ERR_NO_MEMORY;
  }

  TimerQueue::PrintTimerQueues(buf, timer_buffer_size);

  printf("%s", buf);

  free(buf);

  return 0;
}

STATIC_COMMAND_START
STATIC_COMMAND_MASKED("timers", "dump the current kernel timer queues", &cmd_timers,
                      CMD_AVAIL_NORMAL)
STATIC_COMMAND_END(timer)
