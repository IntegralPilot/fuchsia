// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "kernel/idle_power_thread.h"

#include <assert.h>
#include <lib/affine/ratio.h>
#include <lib/fit/defer.h>
#include <lib/ktrace.h>
#include <lib/suspend_wakeup_timer.h>
#include <lib/userabi/vdso.h>
#include <platform.h>
#include <zircon/errors.h>
#include <zircon/time.h>
#include <zircon/types.h>

#include <arch/interrupt.h>
#include <arch/mp.h>
#include <arch/ops.h>
#include <kernel/cpu.h>
#include <kernel/mp.h>
#include <kernel/percpu.h>
#include <kernel/scheduler.h>
#include <kernel/thread.h>
#include <ktl/atomic.h>
#include <ktl/utility.h>
#include <lk/init.h>
#include <object/interrupt_dispatcher.h>
#include <platform/timer.h>

// Note: Do NOT attempt to make this inline static.  The storage needs to be
// explicitly declared in idle_power_thread.cc.
//
// Why?
//
// idle_power_thread.h declares the IdlePowerThread class, and this header is
// included by several other libraries (in particular, lib/syscalls).  The IPT
// class also declares this pointer as a static member of the class.  The
// libraries which include the IPT header cannot automatically pick up a
// dependency on `lib/suspend_wakeup_timer`, so we need to explicitly not
// include the SWT header in the IPT header.  Instead, we just fwd declare
// SuspendWakeupTimer.
//
// But... If we declare `resume_timer_` to be an inline static member of
// IdlePowerThread, then every translation unit which includes
// `idle_power_thread.h` (directly or indirectly) will need to generate a
// destructor for the `unique_ptr`, triggering template expansion (duplicates
// will be sorted out at link time).  In the clang standard implementation of
// the unique_ptr d'tor, there is a static assert which asserts that the
// `sizeof(T)` is > 0, but if we have only fwd declared SuspendWakeupTimer,
// `sizeof(T)` cannot be evaluated and we fail to build.
//
// So: do not declare the storage as inline.  Instead, simply mark the storage
// as static, and explicitly declare the storage in this translation unit where
// we can safely include the definition of SuspendWakeupTimer, generating the
// proper unique_ptr d'tor in this TU specifically.
ktl::unique_ptr<SuspendWakeupTimer> IdlePowerThread::resume_timer_;

namespace {

constexpr bool kEnableRunloopTracing = false;

constexpr const fxt::InternedString& ToInternedString(IdlePowerThread::State state) {
  using fxt::operator""_intern;
  switch (state) {
    case IdlePowerThread::State::Offline:
      return "offline"_intern;
    case IdlePowerThread::State::Suspend:
      return "suspend"_intern;
    case IdlePowerThread::State::Active:
      return "active"_intern;
    case IdlePowerThread::State::Wakeup:
      return "wakeup"_intern;
    default:
      return "unknown"_intern;
  }
}

constexpr const char* ToString(IdlePowerThread::State state) {
  return ToInternedString(state).string();
}

}  // namespace

void IdlePowerThread::UpdateMonotonicClock(cpu_num_t current_cpu,
                                           const StateMachine& current_state) {
  // Currently, UpdatePlatformTimer, called below, requires interrupts to be
  // disabled as an optimization. Since this method, UpdateMonotonicClock,
  // happens to be called with interrupts disabled in the main Run loop, nothing
  // else needs to be done here. Simply assert that interrupts are disabled to
  // ensure that any change to the interrupt disable behavior of the run loop is
  // handled appropriately.
  DEBUG_ASSERT(arch_ints_disabled());

  if (current_cpu == BOOT_CPU_ID) {
    if (current_state.current == State::Suspend) {
      // If we are suspending the system, the boot CPU has to pause the monotonic clock.
      timer_pause_monotonic();
      return;
    }

    if (current_state.current == State::Wakeup) {
      // If we are resuming the system, the boot CPU has to unpause the monotonic clock.
      // It must also update its platform timer to account for any monotonic timers that
      // are present.
      timer_unpause_monotonic();
      VDso::SetMonotonicTicksOffset(timer_get_mono_ticks_offset());
      percpu::Get(current_cpu).timer_queue.UpdatePlatformTimer();
      return;
    }

    return;
  }

  if (current_state.current == State::Active) {
    // If we are resuming the system, the secondary CPUs have to reset their platform
    // timers to account for any monotonic timers that are present.
    percpu::Get(current_cpu).timer_queue.UpdatePlatformTimer();
  }
}

void IdlePowerThread::FlushAndHalt() {
  DEBUG_ASSERT(arch_ints_disabled());

  const StateMachine state = state_.load(ktl::memory_order_acquire);
  DEBUG_ASSERT(state.target == State::Offline);
  state_.store({State::Offline, State::Offline}, ktl::memory_order_release);

  arch_flush_state_and_halt(&complete_);
}

int IdlePowerThread::Run(void* arg) {
  const cpu_num_t cpu_num = arch_curr_cpu_num();
  IdlePowerThread& this_idle_power_thread = percpu::GetCurrent().idle_power_thread;
  Scheduler& this_scheduler = percpu::GetCurrent().scheduler;

  // The accumulated idle time should always be reset to zero when a CPU comes online. Make sure
  // that CPU hotplug does not leave this member in an inconsistent state with respect to the
  // scheduler bookkeeping when this thread is revived after going offline.
  DEBUG_ASSERT(this_scheduler.processor_idle_time() == 0);

  for (;;) {
    // Disable preemption and interrupts to avoid races between idle power thread requests, handling
    // interrupts, and entering the processor idle state. All pending preemtions are handled at the
    // end of this loop iteration. Pending interrupts may be handled either at the end of this loop
    // iteration or within ArchIdlePowerThread::EnterIdleState on architectures that re-enable
    // interrupts there (e.g. x64).
    AutoPreemptDisabler preempt_disabled;
    InterruptDisableGuard interrupt_disable;

    StateMachine state = this_idle_power_thread.state_.load(ktl::memory_order_acquire);
    bool state_changed = false;
    bool transition_success = false;
    if (state.target != state.current) {
      state_changed = true;
      ktrace::Scope trace = KTRACE_CPU_BEGIN_SCOPE_ENABLE(
          kEnableRunloopTracing, "kernel:sched", "transition",
          ("from", ToInternedString(state.current)), ("to", ToInternedString(state.target)));
      dprintf(INFO, "CPU %u: %s -> %s\n", cpu_num, ToString(state.current), ToString(state.target));

      switch (state.target) {
        case State::Active:
        case State::Wakeup:
        case State::Suspend: {
          // Complete the requested transition, which could be interrupted by a wake trigger.
          //
          // Take care to only update the monotonic clock after we have succeeded in transitioning
          // to the desired state.
          StateMachine expected = state;
          const StateMachine desired = {.current = state.target, .target = state.target};
          transition_success = this_idle_power_thread.CompareExchangeState(expected, desired);
          if (transition_success) {
            DEBUG_ASSERT_MSG(desired.current == desired.target, "current=%s target=%s",
                             ToString(desired.current), ToString(desired.target));
            state = desired;
            UpdateMonotonicClock(cpu_num, desired);
          } else {
            DEBUG_ASSERT_MSG(expected == kSuspendToWakeup || expected == kWakeup,
                             "current=%s target=%s", ToString(expected.current),
                             ToString(expected.target));
            state = expected;
          }
          // Do not signal completion when the boot CPU transitions to Suspend. The boot CPU is
          // suspended last and should not prematurely signal completion to
          // TransitionAllActiveToSuspend() before actually entering suspension. If preemption is
          // pending, the boot CPU will abort entering suspend and loop; delaying the signal allows
          // WaitDeadline() in TransitionFromTo() to observe the canceled state (e.g. Wakeup) on the
          // next transition and return ZX_ERR_CANCELED, or timeout if the canceled state is not
          // observed.
          if (state.current != State::Suspend || cpu_num != BOOT_CPU_ID) {
            this_idle_power_thread.complete_.Signal();
          }
          break;
        }

        case State::Offline: {
          // Emit the complete event early, since mp_unplug_current_cpu() will not return.
          trace.End();

          // Clear any accumulated idle time to ensure consistency with runtime vs. idle time
          // asserts in the scheduler when the CPU goes back online.
          this_scheduler.ResetProcessorIdleTime();

          // Updating the state and signaling the complete event is handled by
          // mp_unplug_current_cpu() when it calls FlushAndHalt().
          mp_unplug_current_cpu();
          break;
        }

        default:
          panic("unexpected case %u", static_cast<uint32_t>(state.target));
      }

      // If the power thread just transitioned to Active or Wakeup, reschedule to ensure that the
      // run queue is evaluated. Otherwise, this thread may continue to run the idle loop until the
      // next IPI.
      if (state.current == State::Active || state.current == State::Wakeup) {
        Thread::Current::Reschedule();
      }
    }

    const bool should_suspend_or_idle =
        !state_changed || (transition_success && state.current == State::Suspend);

    if (should_suspend_or_idle) {
      DEBUG_ASSERT(arch_ints_disabled());
      DEBUG_ASSERT(!Thread::Current::Get()->preemption_state().PreemptIsEnabled());

      // A preempt could become pending between disabling preemption and disabling interrupts above.
      // Handle the preemption instead of attempting to suspend or enter the processor idle state.
      if (Thread::Current::Get()->preemption_state().preempts_pending() != 0) {
        continue;
      }

      // WARNING: Be careful not to do anything that could pend a preemption after the check above.

      if (state.current == State::Suspend) {
        ktrace::Scope trace = KTRACE_CPU_BEGIN_SCOPE("kernel:sched", "suspend");

        // Make sure our resume_timer_ has been started if we are the BOOT_CPU and it has a deadline
        // set.
        if (arch_curr_cpu_num() == BOOT_CPU_ID) {
          resume_timer_->EnsureStarted();
        }

        if (platform_supports_suspend_cpu()) {
          zx_status_t status = platform_suspend_cpu(PlatformAllowDomainPowerDown::Yes);
          DEBUG_ASSERT_MSG(
              status == ZX_OK || status == ZX_ERR_ACCESS_DENIED || status == ZX_ERR_INVALID_ARGS,
              "%d", status);
        } else {
          ArchIdlePowerThread::EnterIdleState();
        }

      } else {
        ktrace::Scope trace =
            KTRACE_CPU_BEGIN_SCOPE_ENABLE(kEnableRunloopTracing, "kernel:sched", "idle");

        const zx_instant_mono_t idle_start_time = current_mono_time();
        ArchIdlePowerThread::EnterIdleState();
        const zx_instant_mono_t idle_finish_time = current_mono_time();

        this_scheduler.AddProcessorIdleTime(SchedDuration{idle_finish_time - idle_start_time});
      }

      // END WARNING: Pending preemptions is safe again.
    }
  }
}

IdlePowerThread::TransitionResult IdlePowerThread::TransitionFromTo(State expected_state,
                                                                    State target_state,
                                                                    zx_instant_mono_t timeout_at) {
  // Unsignal any stale completion signals from previous transitions that may have timed out or
  // aborted early. If a stale signal were left pending, WaitDeadline below would return
  // immediately before the target CPU begins processing the transition, racing with concurrent wake
  // events that modify state.target and causing premature cancellation.
  complete_.Unsignal();
  // Attempt to move from the expected state to the transitional state.
  StateMachine expected{.current = expected_state, .target = expected_state};
  const StateMachine transitional{.current = expected_state, .target = target_state};
  if (!CompareExchangeState(expected, transitional)) {
    // The move to the transitional state failed because the current state is already the target
    // state. Indicate that the transition did not occur but the target state is achieved anyway by
    // returning the original state.
    if (expected.current == target_state) {
      return {ZX_OK, target_state};
    }

    // The move to the transitional state failed because the current state is neither the expected
    // state nor the target state.
    dprintf(INFO,
            "Failed power state transition, "
            "expected(current=%s, target=%s) transitional(current=%s, target=%s)\n",
            ToString(expected.current), ToString(expected.target), ToString(transitional.current),
            ToString(transitional.target));
    return {ZX_ERR_BAD_STATE, expected.current};
  }
  DEBUG_ASSERT(expected.current == expected_state);

  {
    // Reschedule the CPU for the idle/power thread to expedite handling the transition. Disable
    // interrupts to ensure this thread doesn't migrate to another CPU after sampling the current
    // CPU.
    InterruptDisableGuard interrupt_disable;
    if (this_cpu_ == arch_curr_cpu_num()) {
      Thread::Current::Reschedule();
    } else {
      mp_reschedule(cpu_num_to_mask(this_cpu_), 0);
    }
  }

  // Wait for the transition to the target state to complete or timeout, looping to deal with
  // spurious wakeups.
  StateMachine state;
  do {
    const zx_status_t status = complete_.WaitDeadline(timeout_at, Interruptible::No);
    if (status != ZX_OK) {
      return {status, expected_state};
    }
    state = state_.load(ktl::memory_order_acquire);

    // If the current or target state changed while waiting, this transition may have been reversed
    // before the current thread could observe the successful transition. This can happen if an
    // interrupt transitions the current CPU from Suspend back to Active after a successful
    // transition to Suspend, but before this thread starts waiting for completion.
    if (state.target != target_state) {
      return {ZX_ERR_CANCELED, state.current};
    }
  } while (state.current != target_state);

  // The transition from the expected state to the target state was successful.
  return {ZX_OK, expected_state};
}

zx_instant_boot_t IdlePowerThread::TransitionAllActiveToSuspend(zx_instant_boot_t resume_at) {
  // Ensure CPUs in |cpus_active_before_suspend| are not modified during the execution of
  // TransitionAllActiveToSuspend.
  Guard<Mutex> guard{TransitionLock::Get()};

  // Ensure that the monotonic clock is running and is returned to the running state no matter how
  // we leave this function.  This is a CYA check to ensure we don't inadvertently leave the clock
  // paused.
  DEBUG_ASSERT(!timer_is_monotonic_paused());
  auto ensure_mono_not_paused = fit::defer([] { DEBUG_ASSERT(!timer_is_monotonic_paused()); });

  const zx_instant_boot_t suspend_start_time = current_boot_time();
  if (resume_at < suspend_start_time) {
    resume_timer_wake_vector_->wake_event.Strobe();
    return suspend_start_time;
  }

  // Conditionally set the global suspended flag so that other subsystems can act appropriately
  // during suspend. If there are pending wake events, abort the suspend.
  {
    uint64_t expected = SystemSuspendStateActive;
    if (!system_suspend_state_.compare_exchange_strong(expected, SystemSuspendStateSuspendedBit,
                                                       ktl::memory_order_release,
                                                       ktl::memory_order_relaxed)) {
      DEBUG_ASSERT((expected & SystemSuspendStateSuspendedBit) != SystemSuspendStateSuspendedBit);
      const uint64_t pending_wake_events = expected & SystemSuspendStateWakeEventPendingMask;
      dprintf(INFO, "Aborting suspend due to %" PRIu64 " pending wake events\n",
              pending_wake_events);

      // TODO(https://fxbug.dev/348668110): Revisit this logging.  Still needed?
      printf("begin dump of pending wake events\n");
      WakeEvent::Dump(stdout, suspend_start_time);
      printf("end dump of pending wake events\n");

      return suspend_start_time;
    }
  }

  auto restore_suspend_flag = fit::defer([] {
    system_suspend_state_.fetch_and(~SystemSuspendStateSuspendedBit, ktl::memory_order_release);
  });

  // Move to the boot CPU which will be suspended last.
  auto restore_affinity = fit::defer(
      [previous_affinity = Thread::Current::Get()->SetCpuAffinity(cpu_num_to_mask(BOOT_CPU_ID))] {
        Thread::Current::Get()->SetCpuAffinity(previous_affinity);
      });
  DEBUG_ASSERT(arch_curr_cpu_num() == BOOT_CPU_ID);

  // TODO(eieio): Consider temporarily pinning the debuglog notifier thread to the boot CPU to give
  // it a chance to notify readers during the non-boot CPU suspend sequence. This is not needed for
  // correctness, since the debuglog critical sections are protected by a spinlock, which prevents
  // the local CPU from being suspended while held. However, given that readers might also get
  // suspended on non-boot CPUs, this may not have much effect on timeliness of log output.

  // Keep track of which non-boot CPUs to resume.
  const cpu_mask_t cpus_active_before_suspend =
      Scheduler::PeekActiveMask() & ~cpu_num_to_mask(BOOT_CPU_ID);
  dprintf(INFO, "Active non-boot CPUs before suspend: %#x\n", cpus_active_before_suspend);

  // Suspend all of the active CPUs besides the boot CPU.
  dprintf(INFO, "Suspending non-boot CPUs...\n");

  const Deadline suspend_timeout_at = Deadline::after_mono(ZX_MIN(1));
  cpu_mask_t cpus_to_suspend = cpus_active_before_suspend;
  while (cpus_to_suspend != 0) {
    const cpu_num_t cpu_id = highest_cpu_set(cpus_to_suspend);
    const TransitionResult active_to_suspend_result =
        percpu::Get(cpu_id).idle_power_thread.TransitionFromTo(State::Active, State::Suspend,
                                                               suspend_timeout_at.when());
    if (active_to_suspend_result.status != ZX_OK) {
      KERNEL_OOPS("Failed to transition CPU %u to suspend: %d\n", cpu_id,
                  active_to_suspend_result.status);
      break;
    }

    cpus_to_suspend &= ~cpu_num_to_mask(cpu_id);
  }

  if (cpus_to_suspend != 0) {
    dprintf(INFO, "Aborting suspend and resuming non-boot CPUs...\n");
  } else {
    dprintf(INFO, "Done suspending non-boot CPUs.\n");

    // Set the resume timer before suspending the boot CPU.
    if (resume_at != ZX_TIME_INFINITE) {
      dprintf(INFO, "Setting boot CPU to resume at time %" PRId64 "\n", resume_at);
      resume_timer_->SetResumeDeadline(resume_at);
    } else {
      // TODO(eieio): Check that at least one wake source is configured if no resume time is set.
      // Maybe this check needs require at least one wake source if the resume time is too far in
      // the future?
      dprintf(INFO, "No kernel resume timeout set. System will only resume by wake vector.\n");
    }

    dprintf(INFO, "Attempting to suspend boot CPU...\n");

    IdlePowerThread& boot_idle_power_thread = percpu::Get(BOOT_CPU_ID).idle_power_thread;
    const TransitionResult active_to_suspend_result = boot_idle_power_thread.TransitionFromTo(
        State::Active, State::Suspend, suspend_timeout_at.when());

    // Cancel the resume timer in case it wasn't the reason for the wakeup.
    resume_timer_->CancelTimer();

    switch (active_to_suspend_result.status) {
      case ZX_ERR_TIMED_OUT:
        dprintf(INFO, "Timed out waiting for boot CPU to suspend!\n");
        break;

      case ZX_ERR_CANCELED:
        dprintf(INFO, "Boot CPU resumed.\n");
        DEBUG_ASSERT_MSG(active_to_suspend_result.starting_state == State::Wakeup,
                         "startring_state=%s", ToString(active_to_suspend_result.starting_state));
        break;

      case ZX_ERR_BAD_STATE:
        dprintf(INFO, "Boot CPU suspend aborted.\n");
        DEBUG_ASSERT_MSG(active_to_suspend_result.starting_state == State::Wakeup,
                         "startring_state=%s", ToString(active_to_suspend_result.starting_state));
        break;

      default:
        DEBUG_ASSERT_MSG(
            false, "Unexpected result from boot CPU suspend: status=%d starting_state=%s",
            active_to_suspend_result.status, ToString(active_to_suspend_result.starting_state));
    }

    dprintf(INFO, "Wake events triggered after/during suspend:\n");
    WakeEvent::Dump(stdout, suspend_start_time);

    // If the boot CPU is in the Wakeup state, set it to Active.
    StateMachine expected = kWakeup;
    const bool succeeded = boot_idle_power_thread.CompareExchangeState(expected, kActive);
    DEBUG_ASSERT_MSG(succeeded || expected == kActive, "current=%s target=%s",
                     ToString(expected.current), ToString(expected.target));
  }

  // Resume all non-boot CPUs that were successfully suspended.
  dprintf(INFO, "Resuming non-boot CPUs...\n");

  const Deadline resume_timeout_at = Deadline::after_mono(ZX_MIN(1));
  cpu_mask_t cpus_to_resume = cpus_active_before_suspend ^ cpus_to_suspend;
  while (cpus_to_resume != 0) {
    const cpu_num_t cpu_id = highest_cpu_set(cpus_to_resume);
    const TransitionResult suspend_to_active_result =
        percpu::Get(cpu_id).idle_power_thread.TransitionFromTo(State::Suspend, State::Active,
                                                               resume_timeout_at.when());
    if (suspend_to_active_result.status != ZX_OK) {
      KERNEL_OOPS("Failed to transition CPU %u to active: %d\n", cpu_id,
                  suspend_to_active_result.status);
      break;
    }

    cpus_to_resume &= ~cpu_num_to_mask(cpu_id);
  }
  DEBUG_ASSERT(cpus_to_resume == 0);

  dprintf(INFO, "Done resuming non-boot CPUs.\n");
  return suspend_start_time;
}

IdlePowerThread::WakeResult IdlePowerThread::WakeBootCpu() {
  DEBUG_ASSERT(system_suspended());
  DEBUG_ASSERT(arch_ints_disabled());
  DEBUG_ASSERT(Thread::Current::Get()->preemption_state().PreemptIsEnabled() == false);

  // Poke the boot CPU when preemption is reenabled to ensure it responds to the potential state
  // change.
  Thread::Current::Get()->preemption_state().PreemptSetPending(cpu_num_to_mask(BOOT_CPU_ID));

  IdlePowerThread& boot_idle_power_thread = percpu::Get(BOOT_CPU_ID).idle_power_thread;

  // Attempt to transition the boot CPU from Suspend to Wakeup. This method may be running on the
  // boot CPU and interrupting the context of the idle/power thread.
  StateMachine expected = kSuspend;
  do {
    bool success = boot_idle_power_thread.CompareExchangeState(expected, kSuspendToWakeup);
    if (success) {
      return WakeResult::Resumed;
    }

    // If the current state is Active or Active-to-Suspend, this wake up occurred before or during
    // the Active-to-Suspend transition of the boot CPU. Set the state to Wakeup to prevent the
    // Active-to-Suspend transition, which would cause this wake up to be missed.
    if (expected == kActive || expected == kActiveToSuspend) {
      success = boot_idle_power_thread.CompareExchangeState(expected, kWakeup);
      if (success) {
        return WakeResult::SuspendAborted;
      }
    }

    // Try again if the previous attempt collided with completing the Active-to-Suspend transition.
  } while (expected == kSuspend);

  // Racing with another wake trigger can cause the attempts above to gracefully fail. However,
  // colliding with other states is an error.
  DEBUG_ASSERT_MSG(expected == kSuspendToWakeup || expected == kWakeup, "current=%s target=%s",
                   ToString(expected.current), ToString(expected.target));
  return WakeResult::Resumed;
}

IdlePowerThread::WakeResult IdlePowerThread::TriggerSystemWakeEvent() {
  DEBUG_ASSERT(arch_ints_disabled());
  DEBUG_ASSERT(Thread::Current::Get()->preemption_state().PreemptIsEnabled() == false);

  const uint64_t previous = system_suspend_state_.fetch_add(1, ktl::memory_order_relaxed);
  const uint64_t previous_pending_wake_events = previous & SystemSuspendStateWakeEventPendingMask;
  const bool suspended = previous & SystemSuspendStateSuspendedBit;

  DEBUG_ASSERT_MSG(previous_pending_wake_events != SystemSuspendStateWakeEventPendingMask,
                   "Pending wake event count overflow!");

  // Wake the boot CPU if the system is suspended and this is the first pending wake event.
  if (suspended && previous_pending_wake_events == 0) {
    return WakeBootCpu();
  }
  return suspended ? WakeResult::Resumed : WakeResult::Active;
}

void IdlePowerThread::AcknowledgeSystemWakeEvent() {
  const uint64_t previous = system_suspend_state_.fetch_add(-1, ktl::memory_order_relaxed);
  const uint64_t previous_pending_wake_events = previous & SystemSuspendStateWakeEventPendingMask;
  DEBUG_ASSERT_MSG(previous_pending_wake_events != 0, "Pending wake event count underflow!");
}

void IdlePowerThread::ResumeFromTimerIrq(zx_instant_boot_t now, zx_instant_boot_t resume_at) {
  // Verify this handler is running in the correct context.
  DEBUG_ASSERT(arch_curr_cpu_num() == BOOT_CPU_ID);

  const WakeResult wake_result = resume_timer_wake_vector_->wake_event.Strobe(now);
  const char* message_prefix = wake_result == WakeResult::SuspendAborted
                                   ? "Wakeup before suspend completed. Aborting suspend"
                                   : "Resuming boot CPU";

  const zx_duration_boot_t resume_delta = zx_time_sub_time(now, resume_at);
  dprintf(INFO, "%s at time %" PRId64 ", %" PRId64 "ns after target resume time.\n", message_prefix,
          now, resume_delta);
}

void IdlePowerThread::ResumeTimerWakeVector::GetDiagnostics(
    WakeVector::Diagnostics& diagnostics_out) const {
  diagnostics_out.enabled = true;
  diagnostics_out.koid = ZX_KOID_KERNEL;
  diagnostics_out.PrintExtra("suspend timeout");
}

void IdlePowerThread::InitHook(uint level) {
  switch (level) {
    case LK_INIT_LEVEL_EARLIEST:
      // Register the resume timer wake event after global ctors have initialized the global list.
      resume_timer_wake_vector_.Initialize();
      break;

    case LK_INIT_LEVEL_PLATFORM: {
      // Create the SuspendResumeTimer after platform drivers have been initialized.  This gives
      // systems which use special timer HW to exit suspend a chance to detect and initialize their
      // hardware before we start to use it.
      Guard<Mutex> guard{TransitionLock::Get()};
      resume_timer_ = SuspendWakeupTimer::Create(ResumeFromTimerIrq);
      ASSERT((resume_timer_) != nullptr);
      break;
    }
  }
}
LK_INIT_HOOK(idle_power_thread_earliest, IdlePowerThread::InitHook, LK_INIT_LEVEL_EARLIEST)
LK_INIT_HOOK(idle_power_thread_platform, IdlePowerThread::InitHook, LK_INIT_LEVEL_PLATFORM)

#include <lib/console.h>

static int cmd_suspend(int argc, const cmd_args* argv, uint32_t flags) {
  auto usage = [&argv]() -> int {
    printf("usage:\n");
    printf("%s enter <resume delay> <m|s|ms|us|ns>\n", argv[0].str);
    printf("%s wake-events\n", argv[0].str);
    return ZX_ERR_INTERNAL;
  };

  if (argc < 2) {
    return usage();
  }

  if (!strcmp(argv[1].str, "enter")) {
    if (argc < 4) {
      return usage();
    }

    zx_duration_boot_t delay = argv[2].i;
    const char* units = argv[3].str;
    if (!strcmp(units, "m")) {
      delay = ZX_MIN(delay);
    } else if (!strcmp(units, "s")) {
      delay = ZX_SEC(delay);
    } else if (!strcmp(units, "ms")) {
      delay = ZX_MSEC(delay);
    } else if (!strcmp(units, "us")) {
      delay = ZX_USEC(delay);
    } else if (!strcmp(units, "ns")) {
      delay = ZX_NSEC(delay);
    } else {
      printf("Invalid units: %s\n", units);
      return usage();
    }

    const zx_instant_boot_t resume_at = zx_time_add_duration(current_boot_time(), delay);
    IdlePowerThread::TransitionAllActiveToSuspend(resume_at);
    return ZX_OK;
  }

  if (!strcmp(argv[1].str, "wake-events")) {
    if (argc != 2) {
      return usage();
    }

    printf("Wake events:\n");
    wake_vector::WakeEvent::Dump(stdout, 0);
    return 0;
  }

  printf("unknown command\n");
  return usage();
}

STATIC_COMMAND_START
STATIC_COMMAND_MASKED("suspend", "processor suspend commands", &cmd_suspend, CMD_AVAIL_ALWAYS)
STATIC_COMMAND_END(suspend)
