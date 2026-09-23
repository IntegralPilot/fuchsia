// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::mm::MemoryAccessorExt;
use crate::security;
use crate::signals::SignalEvent;
use crate::task::{
    CurrentTask, EventHandler, SignalHandler, SignalHandlerInner, Task, ThreadGroup, Waiter,
};
use crate::time::utc::utc_now;
use crate::time::{ClockId, GenericDuration, Timeline, TimerId, TimerWakeup};
use fuchsia_runtime::UtcInstant;
use starnix_logging::{log_debug, log_error, log_trace, track_stub};
use starnix_types::time::{
    NANOS_PER_SECOND, duration_from_timespec, duration_to_scheduler_clock, time_from_timespec,
    timespec_from_duration, timespec_is_zero, timeval_from_time,
};
use starnix_uapi::auth::CAP_WAKE_ALARM;
use starnix_uapi::errors::{EINTR, Errno};
use starnix_uapi::user_address::{MultiArchUserRef, UserRef};
use starnix_uapi::{
    CLOCK_BOOTTIME, CLOCK_BOOTTIME_ALARM, CLOCK_MONOTONIC, CLOCK_MONOTONIC_COARSE,
    CLOCK_MONOTONIC_RAW, CLOCK_PROCESS_CPUTIME_ID, CLOCK_REALTIME, CLOCK_REALTIME_ALARM,
    CLOCK_REALTIME_COARSE, CLOCK_TAI, CLOCK_THREAD_CPUTIME_ID, MAX_CLOCKS, TIMER_ABSTIME, errno,
    error, from_status_like_fdio, pid_t, timespec, timezone, tms, uapi,
};
use zx::{
    Task as _, {self as zx},
};

pub type TimeSpecPtr = MultiArchUserRef<uapi::timespec, uapi::arch32::timespec>;
pub type ITimerSpecPtr = MultiArchUserRef<uapi::itimerspec, uapi::arch32::itimerspec>;
pub type ITimerValPtr = MultiArchUserRef<uapi::itimerval, uapi::arch32::itimerval>;
pub type TimeValPtr = MultiArchUserRef<uapi::timeval, uapi::arch32::timeval>;
type TimeZonePtr = MultiArchUserRef<uapi::timezone, uapi::arch32::timezone>;

fn get_clock_res(current_task: &CurrentTask, which_clock: i32) -> Result<timespec, Errno> {
    match which_clock as u32 {
        CLOCK_REALTIME
        | CLOCK_REALTIME_ALARM
        | CLOCK_REALTIME_COARSE
        | CLOCK_MONOTONIC
        | CLOCK_MONOTONIC_COARSE
        | CLOCK_MONOTONIC_RAW
        | CLOCK_BOOTTIME
        | CLOCK_BOOTTIME_ALARM
        | CLOCK_THREAD_CPUTIME_ID
        | CLOCK_PROCESS_CPUTIME_ID => Ok(timespec { tv_sec: 0, tv_nsec: 1 }),
        _ => {
            // Error if no dynamic clock can be found.
            let _ = get_dynamic_clock(current_task, which_clock)?;
            Ok(timespec { tv_sec: 0, tv_nsec: 1 })
        }
    }
}

pub fn sys_clock_getres(
    current_task: &CurrentTask,
    which_clock: i32,
    tp_addr: TimeSpecPtr,
) -> Result<(), Errno> {
    if which_clock < 0 && !is_valid_cpu_clock(which_clock) {
        return error!(EINVAL);
    }
    if tp_addr.is_null() {
        return Ok(());
    }
    let tv = get_clock_res(current_task, which_clock)?;
    current_task.write_multi_arch_object(tp_addr, tv)?;
    Ok(())
}

fn get_clock_gettime(current_task: &CurrentTask, which_clock: i32) -> Result<timespec, Errno> {
    let nanos = if which_clock < 0 {
        get_dynamic_clock(current_task, which_clock)?
    } else {
        match which_clock as u32 {
            CLOCK_REALTIME | CLOCK_REALTIME_COARSE => utc_now().into_nanos(),
            CLOCK_MONOTONIC | CLOCK_MONOTONIC_COARSE | CLOCK_MONOTONIC_RAW => {
                zx::MonotonicInstant::get().into_nanos()
            }
            CLOCK_BOOTTIME => zx::BootInstant::get().into_nanos(),
            CLOCK_THREAD_CPUTIME_ID => get_thread_cpu_time(current_task)?,
            CLOCK_PROCESS_CPUTIME_ID => get_process_cpu_time(current_task.thread_group())?,
            _ => return error!(EINVAL),
        }
    };
    Ok(timespec { tv_sec: nanos / NANOS_PER_SECOND, tv_nsec: nanos % NANOS_PER_SECOND })
}

pub fn sys_clock_gettime(
    current_task: &CurrentTask,
    which_clock: i32,
    tp_addr: TimeSpecPtr,
) -> Result<(), Errno> {
    let tv = get_clock_gettime(current_task, which_clock)?;
    current_task.write_multi_arch_object(tp_addr, tv)?;
    Ok(())
}

pub fn sys_gettimeofday(
    current_task: &CurrentTask,
    user_tv: TimeValPtr,
    user_tz: TimeZonePtr,
) -> Result<(), Errno> {
    if !user_tv.is_null() {
        let tv = timeval_from_time(utc_now());
        current_task.write_multi_arch_object(user_tv, tv)?;
    }
    if !user_tz.is_null() {
        // Return early if the user passes an obviously invalid pointer. This check is not a guarantee.
        current_task.mm()?.check_plausible(user_tz.addr(), std::mem::size_of::<timezone>())?;
        track_stub!(TODO("https://fxbug.dev/322874502"), "gettimeofday tz argument");
    }
    Ok(())
}

pub fn sys_settimeofday(
    current_task: &CurrentTask,
    tv: TimeValPtr,
    _tz: TimeZonePtr,
) -> Result<(), Errno> {
    const SEC_IN_NANOS: i64 = 1_000_000_000;
    const USEC_IN_NANOS: i64 = 1000;
    let kernel = current_task.kernel();
    if let Some(ref proxy) = kernel.time_adjustment_proxy {
        // Setting time is allowed.
        let boot_now = zx::BootInstant::get();
        let tv = current_task.read_multi_arch_object(tv)?;

        // Any errors here result in EINVAL, there should be no overflow in "normal" situations.
        let utc_now_sec_as_nanos =
            tv.tv_sec.checked_mul(SEC_IN_NANOS).ok_or_else(|| errno!(EINVAL))?;
        let utc_now_usec_as_nanos =
            tv.tv_usec.checked_mul(USEC_IN_NANOS).ok_or_else(|| errno!(EINVAL))?;
        let utc_now_nanos = utc_now_sec_as_nanos
            .checked_add(utc_now_usec_as_nanos)
            .ok_or_else(|| errno!(EINVAL))?;
        log_debug!(
            "settimeofday: reporting reference: boot_now={:?}, utc_now_nanos={:?}",
            boot_now,
            utc_now_nanos
        );
        proxy
            .report_boot_to_utc_mapping(
                boot_now.into(),
                utc_now_nanos,
                zx::MonotonicInstant::INFINITE,
            )
            .map_err(|e| {
                log_error!("FIDL error: {:?}", e);
                // Maybe a weird choice of the error code, but the only choice
                // between the documented error codes for `settimeofday` that
                // seems relevant for when FIDL breaks.
                errno!(ENOSYS)
            })?
            .map_err(|e| {
                log_error!("remote error: {:?}", e);
                // Remote should normally report an error only as a result of
                // invalid user input. Hence, EINVAL.
                errno!(EINVAL)
            })
    } else {
        // We expect most systems not to be allowed to set time.
        log_debug!("settimeofday: No functionality");
        error!(ENOSYS)
    }
}

pub fn sys_clock_nanosleep(
    current_task: &mut CurrentTask,
    which_clock: ClockId,
    flags: u32,
    user_request: TimeSpecPtr,
    user_remaining: TimeSpecPtr,
) -> Result<(), Errno> {
    if which_clock < 0 {
        return error!(EINVAL);
    }
    let which_clock = which_clock as u32;
    let is_absolute = flags == TIMER_ABSTIME;
    // TODO(https://fxrev.dev/117507): For now, Starnix pretends that the monotonic and realtime
    // clocks advance at close to uniform rates and so we can treat relative realtime offsets the
    // same way that we treat relative monotonic clock offsets with a linear adjustment and retries
    // if we sleep for too little time.
    // At some point we'll need to monitor changes to the realtime clock proactively and adjust
    // timers accordingly.
    match which_clock {
        CLOCK_REALTIME | CLOCK_MONOTONIC | CLOCK_BOOTTIME => {}
        CLOCK_TAI => {
            track_stub!(TODO("https://fxbug.dev/322875165"), "clock_nanosleep, CLOCK_TAI", flags);
            return error!(EINVAL);
        }
        CLOCK_PROCESS_CPUTIME_ID => {
            track_stub!(
                TODO("https://fxbug.dev/322874886"),
                "clock_nanosleep, CLOCK_PROCESS_CPUTIME_ID",
                flags
            );
            return error!(EINVAL);
        }
        _ => return error!(ENOTSUP),
    }

    let request = current_task.read_multi_arch_object(user_request)?;
    log_trace!("clock_nanosleep({}, {}, {:?})", which_clock, flags, request);

    if timespec_is_zero(request) {
        return Ok(());
    }

    if which_clock == CLOCK_REALTIME {
        return clock_nanosleep_relative_to_utc(current_task, request, is_absolute, user_remaining);
    }

    // TODO(https://fxbug.dev/361583830): Support futex wait on different timeline deadlines.
    let boot_deadline = if is_absolute {
        time_from_timespec(request)?
    } else {
        zx::BootInstant::after(duration_from_timespec(request)?)
    };

    clock_nanosleep_boot_with_deadline(
        current_task,
        is_absolute,
        boot_deadline,
        None,
        user_remaining,
    )
}

/// Sleep until we've satisfied |request| relative to the UTC clock which may advance at
/// a different rate from the boot clock by repeatdly computing a boot target and sleeping.
fn clock_nanosleep_relative_to_utc(
    current_task: &mut CurrentTask,
    request: timespec,
    is_absolute: bool,
    user_remaining: TimeSpecPtr,
) -> Result<(), Errno> {
    let clock_deadline_absolute = if is_absolute {
        time_from_timespec(request)?
    } else {
        utc_now() + duration_from_timespec(request)?
    };
    loop {
        // Compute boot deadline that corresponds to the UTC clocks's current transformation to
        // boot. This may have changed while we were sleeping so check again on every
        // iteration.
        let (boot_deadline, _) =
            crate::time::utc::estimate_boot_deadline_from_utc(clock_deadline_absolute);
        clock_nanosleep_boot_with_deadline(
            current_task,
            is_absolute,
            boot_deadline,
            Some(clock_deadline_absolute),
            user_remaining,
        )?;
        // Look at |clock| again and decide if we're done.
        let clock_now = utc_now();
        if clock_now >= clock_deadline_absolute {
            return Ok(());
        }
        log_trace!(
            "clock_nanosleep_relative_to_clock short by {:?}, sleeping again",
            clock_deadline_absolute - clock_now
        );
    }
}

fn clock_nanosleep_boot_with_deadline(
    current_task: &mut CurrentTask,
    is_absolute: bool,
    deadline: zx::BootInstant,
    original_utc_deadline: Option<UtcInstant>,
    user_remaining: TimeSpecPtr,
) -> Result<(), Errno> {
    let waiter = Waiter::new();
    let timer = zx::BootTimer::create();
    let signal_handler = SignalHandler {
        inner: SignalHandlerInner::None,
        event_handler: EventHandler::None,
        err_code: None,
    };
    waiter
        .wake_on_zircon_signals(&timer, zx::Signals::TIMER_SIGNALED, signal_handler)
        .expect("wait can only fail in OOM conditions");
    let timer_slack = current_task.read().get_timerslack();
    timer.set(deadline, timer_slack).expect("timer set cannot fail with valid handles and slack");
    match waiter.wait(current_task) {
        Err(err) if err == EINTR && is_absolute => error!(ERESTARTNOHAND),
        Err(err) if err == EINTR => {
            if !user_remaining.is_null() {
                let remaining = match original_utc_deadline {
                    Some(original_utc_deadline) => {
                        GenericDuration::from(original_utc_deadline - utc_now())
                    }
                    None => GenericDuration::from(deadline - zx::BootInstant::get()),
                };
                let remaining = timespec_from_duration(*std::cmp::max(
                    GenericDuration::from_nanos(0),
                    remaining,
                ));
                current_task.write_multi_arch_object(user_remaining, remaining)?;
            }
            current_task.set_syscall_restart_func(move |current_task| {
                clock_nanosleep_boot_with_deadline(
                    current_task,
                    is_absolute,
                    deadline,
                    original_utc_deadline,
                    user_remaining,
                )
            });
            error!(ERESTART_RESTARTBLOCK)
        }
        non_eintr => non_eintr,
    }
}

pub fn sys_nanosleep(
    current_task: &mut CurrentTask,
    user_request: TimeSpecPtr,
    user_remaining: TimeSpecPtr,
) -> Result<(), Errno> {
    sys_clock_nanosleep(current_task, CLOCK_REALTIME as ClockId, 0, user_request, user_remaining)
}

/// Returns the cpu time for `task`.
fn get_thread_cpu_time(task: &Task) -> Result<i64, Errno> {
    Ok(task.thread_runtime_info()?.cpu_time)
}

/// Returns the cpu time for the thread group `tg`.
fn get_process_cpu_time(tg: &ThreadGroup) -> Result<i64, Errno> {
    Ok(tg.process.get_runtime_info().map_err(|status| from_status_like_fdio!(status))?.cpu_time)
}

/// Returns the type of cpu clock that `clock` encodes.
fn which_cpu_clock(clock: i32) -> i32 {
    const CPU_CLOCK_MASK: i32 = 3;
    clock & CPU_CLOCK_MASK
}

/// Returns whether or not `clock` encodes a valid clock type.
fn is_valid_cpu_clock(clock: i32) -> bool {
    const MAX_CPU_CLOCK: i32 = 3;
    if clock & 7 == 7 {
        return false;
    }
    if which_cpu_clock(clock) >= MAX_CPU_CLOCK {
        return false;
    }

    true
}

/// Returns the pid encoded in `clock`.
fn pid_of_clock_id(clock: i32) -> pid_t {
    // The pid is stored in the most significant 29 bits.
    !(clock >> 3) as pid_t
}

/// Returns true if the clock references a thread specific clock.
fn is_thread_clock(clock: i32) -> bool {
    const PER_THREAD_MASK: i32 = 4;
    clock & PER_THREAD_MASK != 0
}

/// Returns the cpu time for the clock specified in `which_clock`.
///
/// This is to support "dynamic clocks."
/// https://man7.org/linux/man-pages/man2/clock_gettime.2.html
///
/// `which_clock` is decoded as follows:
///   - Bit 0 and 1 are used to determine the type of clock.
///   - Bit 3 is used to determine whether the clock is for a thread or process.
///   - The remaining bits encode the pid of the thread/process.
fn get_dynamic_clock(current_task: &CurrentTask, which_clock: i32) -> Result<i64, Errno> {
    if !is_valid_cpu_clock(which_clock) {
        return error!(EINVAL);
    }

    let pid = pid_of_clock_id(which_clock);
    let target_pid = current_task.kernel().pids.get(pid).map_err(|_| errno!(EINVAL))?;

    if is_thread_clock(which_clock) {
        let task = target_pid.get_task().map_err(|_| errno!(EINVAL))?;
        get_thread_cpu_time(&task)
    } else {
        let tg = target_pid.get_thread_group().map_err(|_| errno!(EINVAL))?;
        get_process_cpu_time(&tg)
    }
}

pub fn sys_timer_create(
    current_task: &CurrentTask,
    clock_id: ClockId,
    event: MultiArchUserRef<uapi::sigevent, uapi::arch32::sigevent>,
    timerid: UserRef<TimerId>,
) -> Result<(), Errno> {
    if clock_id >= MAX_CLOCKS as TimerId {
        return error!(EINVAL);
    }
    let user_event = if event.addr().is_null() {
        None
    } else {
        Some(current_task.read_multi_arch_object(event)?)
    };

    let mut checked_signal_event: Option<SignalEvent> = None;
    let thread_group = current_task.thread_group();
    if let Some(user_event) = user_event {
        let signal_event: SignalEvent = user_event.try_into()?;
        if !signal_event.is_valid(&thread_group.read()) {
            return error!(EINVAL);
        }
        checked_signal_event = Some(signal_event);
    }
    let timeline = match clock_id as u32 {
        CLOCK_REALTIME => Timeline::RealTime,
        CLOCK_MONOTONIC => Timeline::Monotonic,
        CLOCK_BOOTTIME => Timeline::BootInstant,
        CLOCK_REALTIME_ALARM => Timeline::RealTime,
        CLOCK_BOOTTIME_ALARM => Timeline::BootInstant,
        CLOCK_TAI => {
            track_stub!(TODO("https://fxbug.dev/349191834"), "timers w/ TAI");
            return error!(ENOTSUP);
        }
        CLOCK_PROCESS_CPUTIME_ID => {
            track_stub!(TODO("https://fxbug.dev/349188105"), "timers w/ calling process cpu time");
            return error!(ENOTSUP);
        }
        CLOCK_THREAD_CPUTIME_ID => {
            track_stub!(TODO("https://fxbug.dev/349188105"), "timers w/ calling thread cpu time");
            return error!(ENOTSUP);
        }
        _ => {
            track_stub!(TODO("https://fxbug.dev/349188105"), "timers w/ dynamic process clocks");
            return error!(ENOTSUP);
        }
    };
    let timer_wakeup = match clock_id as u32 {
        CLOCK_BOOTTIME_ALARM | CLOCK_REALTIME_ALARM => {
            security::check_task_capable(current_task, CAP_WAKE_ALARM)?;
            TimerWakeup::Alarm
        }
        _ => TimerWakeup::Regular,
    };

    let id = &thread_group.timers.create(timeline, timer_wakeup, checked_signal_event)?;
    current_task.write_object(timerid, &id)?;
    Ok(())
}

pub fn sys_timer_delete(current_task: &CurrentTask, id: TimerId) -> Result<(), Errno> {
    current_task.thread_group().timers.delete(current_task, id)
}

pub fn sys_timer_gettime(
    current_task: &CurrentTask,
    id: TimerId,
    curr_value: ITimerSpecPtr,
) -> Result<(), Errno> {
    let timers = &current_task.thread_group().timers;
    current_task.write_multi_arch_object(curr_value, timers.get_time(id)?)?;
    Ok(())
}

pub fn sys_timer_getoverrun(current_task: &CurrentTask, id: TimerId) -> Result<i32, Errno> {
    current_task.thread_group().timers.get_overrun(id)
}

pub fn sys_timer_settime(
    current_task: &CurrentTask,
    id: TimerId,
    flags: i32,
    user_new_value: ITimerSpecPtr,
    user_old_value: ITimerSpecPtr,
) -> Result<(), Errno> {
    if user_new_value.is_null() {
        return error!(EINVAL);
    }
    let new_value = current_task.read_multi_arch_object(user_new_value)?;

    // Return early if the user passes an obviously invalid pointer. This avoids changing the timer
    // settings for common pointer errors.
    if !user_old_value.is_null() {
        current_task.write_multi_arch_object(user_old_value, Default::default())?;
    }

    let old_value =
        current_task.thread_group().timers.set_time(current_task, id, flags, new_value)?;

    if !user_old_value.is_null() {
        current_task.write_multi_arch_object(user_old_value, old_value)?;
    }
    Ok(())
}

pub fn sys_getitimer(
    current_task: &CurrentTask,
    which: u32,
    user_curr_value: ITimerValPtr,
) -> Result<(), Errno> {
    let remaining = current_task.thread_group().get_itimer(which)?;
    current_task.write_multi_arch_object(user_curr_value, remaining)?;
    Ok(())
}

pub fn sys_setitimer(
    current_task: &CurrentTask,
    which: u32,
    user_new_value: ITimerValPtr,
    user_old_value: ITimerValPtr,
) -> Result<(), Errno> {
    let new_value = current_task.read_multi_arch_object(user_new_value)?;

    let old_value = current_task.thread_group().set_itimer(current_task, which, new_value)?;

    if !user_old_value.is_null() {
        current_task.write_multi_arch_object(user_old_value, old_value)?;
    }

    Ok(())
}

pub fn sys_times(current_task: &CurrentTask, buf: UserRef<tms>) -> Result<i64, Errno> {
    if !buf.is_null() {
        let thread_group = current_task.thread_group();
        let process_time_stats = thread_group.time_stats();
        let children_time_stats = thread_group.read().children_time_stats;
        let tms_result = tms {
            tms_utime: duration_to_scheduler_clock(process_time_stats.user_time),
            tms_stime: duration_to_scheduler_clock(process_time_stats.system_time),
            tms_cutime: duration_to_scheduler_clock(children_time_stats.user_time),
            tms_cstime: duration_to_scheduler_clock(children_time_stats.system_time),
        };
        current_task.write_object(buf, &tms_result)?;
    }

    Ok(duration_to_scheduler_clock(zx::MonotonicInstant::get() - zx::MonotonicInstant::ZERO))
}

// Syscalls for arch32 usage
#[cfg(target_arch = "aarch64")]
mod arch32 {
    use crate::task::CurrentTask;
    use crate::time::TimerId;
    use starnix_uapi::errors::Errno;
    use starnix_uapi::uapi;
    use starnix_uapi::user_address::UserRef;
    use static_assertions::const_assert;

    pub fn sys_arch32_clock_gettime64(
        current_task: &CurrentTask,
        which_clock: i32,
        tp_addr: UserRef<uapi::timespec>,
    ) -> Result<(), Errno> {
        const_assert!(
            std::mem::size_of::<uapi::timespec>()
                == std::mem::size_of::<uapi::arch32::__kernel_timespec>()
        );
        super::sys_clock_gettime(current_task, which_clock, tp_addr.into())
    }

    pub fn sys_arch32_timer_gettime64(
        current_task: &CurrentTask,
        id: TimerId,
        curr_value: UserRef<uapi::itimerspec>,
    ) -> Result<(), Errno> {
        const_assert!(
            std::mem::size_of::<uapi::itimerspec>()
                == std::mem::size_of::<uapi::arch32::__kernel_itimerspec>()
        );
        super::sys_timer_gettime(current_task, id, curr_value.into())
    }

    pub use super::{
        sys_clock_getres as sys_arch32_clock_getres, sys_clock_gettime as sys_arch32_clock_gettime,
        sys_gettimeofday as sys_arch32_gettimeofday, sys_nanosleep as sys_arch32_nanosleep,
        sys_setitimer as sys_arch32_setitimer, sys_settimeofday as sys_arch32_settimeofday,
        sys_timer_create as sys_arch32_timer_create, sys_timer_delete as sys_arch32_timer_delete,
        sys_timer_getoverrun as sys_arch32_timer_getoverrun,
        sys_timer_gettime as sys_arch32_timer_gettime,
        sys_timer_settime as sys_arch32_timer_settime,
    };
}

#[cfg(target_arch = "aarch64")]
pub use arch32::*;

#[cfg(test)]
mod test {
    use super::*;
    use crate::mm::PAGE_SIZE;
    use crate::testing::{map_memory, spawn_kernel_and_run};
    use crate::time::utc::UtcClockOverrideGuard;
    use fuchsia_runtime::{UtcDuration, UtcTimeline};
    use starnix_uapi::signals;
    use starnix_uapi::user_address::UserAddress;
    use std::sync::Arc;
    use test_util::{assert_geq, assert_leq};
    use zx::{BootTimeline, Clock, ClockUpdate};

    // TODO(https://fxbug.dev/356911500): Use types below from fuchsia_runtime
    type UtcClock = Clock<BootTimeline, UtcTimeline>;
    type UtcClockUpdate = ClockUpdate<BootTimeline, UtcTimeline>;

    #[::fuchsia::test]
    async fn test_nanosleep_without_remainder() {
        spawn_kernel_and_run(async |current_task| {
            let thread = std::thread::spawn({
                let task = current_task.weak_task();
                move || {
                    let task = task.upgrade().expect("task must be alive");
                    // Wait until the task is in nanosleep, and interrupt it.
                    while !task.read().is_blocked() {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    task.interrupt();
                }
            });

            let duration = timespec_from_duration(zx::MonotonicDuration::from_seconds(60));
            let address = map_memory(
                &current_task,
                UserAddress::default(),
                std::mem::size_of::<timespec>() as u64,
            );
            let address_ptr = UserRef::<timespec>::from(address);
            current_task.write_object(address_ptr, &duration).expect("write_object");

            // nanosleep will be interrupted by the current thread and should not fail with EFAULT
            // because the remainder pointer is null.
            assert_eq!(
                sys_nanosleep(current_task, address_ptr.into(), UserRef::default().into()),
                error!(ERESTART_RESTARTBLOCK)
            );

            thread.join().expect("join");
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_clock_nanosleep_relative_to_slow_clock() {
        spawn_kernel_and_run(async |current_task| {
            let test_clock = UtcClock::create(zx::ClockOpts::AUTO_START, None).unwrap();
            let _test_clock_guard = UtcClockOverrideGuard::new(
                test_clock.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
            );

            // Slow |test_clock| down and verify that we sleep long enough.
            let slow_clock_update = UtcClockUpdate::builder().rate_adjust(-1000).build();
            test_clock.update(slow_clock_update).unwrap();

            let before = test_clock.read().unwrap();

            let tv = timespec { tv_sec: 1, tv_nsec: 0 };

            let remaining = UserRef::new(UserAddress::default());

            super::clock_nanosleep_relative_to_utc(current_task, tv, false, remaining.into())
                .unwrap();
            let elapsed = test_clock.read().unwrap() - before;
            assert!(elapsed >= UtcDuration::from_seconds(1));
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_clock_nanosleep_interrupted_relative_to_fast_utc_clock() {
        spawn_kernel_and_run(async |current_task| {
            let test_clock = UtcClock::create(zx::ClockOpts::AUTO_START, None).unwrap();
            let _test_clock_guard = UtcClockOverrideGuard::new(
                test_clock.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
            );

            // Speed |test_clock| up.
            let slow_clock_update = UtcClockUpdate::builder().rate_adjust(1000).build();
            test_clock.update(slow_clock_update).unwrap();

            let before = test_clock.read().unwrap();

            let tv = timespec { tv_sec: 2, tv_nsec: 0 };

            let remaining = {
                let addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);
                UserRef::new(addr)
            };

            // Interrupt the sleep roughly halfway through. The actual interruption might be before the
            // sleep starts, during the sleep, or after.
            let interruption_target =
                zx::MonotonicInstant::get() + zx::MonotonicDuration::from_seconds(1);

            let thread_group = Arc::downgrade(current_task.thread_group());
            let thread_join_handle = std::thread::Builder::new()
                .name("clock_nanosleep_interruptor".to_string())
                .spawn(move || {
                    std::thread::sleep(std::time::Duration::from_nanos(
                        (interruption_target - zx::MonotonicInstant::get()).into_nanos() as u64,
                    ));
                    if let Some(thread_group) = thread_group.upgrade() {
                        let signal = signals::SIGALRM;
                        thread_group
                            .write()
                            .send_signal(crate::signals::SignalInfo::kernel(signal));
                    }
                })
                .unwrap();

            let result =
                super::clock_nanosleep_relative_to_utc(current_task, tv, false, remaining.into());

            // We can't know deterministically if our interrupter thread will be able to interrupt our sleep.
            // If it did, result should be ERESTART_RESTARTBLOCK and |remaining| will be populated.
            // If it didn't, the result will be OK and |remaining| will not be touched.
            let mut remaining_written = Default::default();
            if result.is_err() {
                assert_eq!(result, error!(ERESTART_RESTARTBLOCK));
                remaining_written = current_task.read_object(remaining).unwrap();
            }
            assert_leq!(
                duration_from_timespec::<zx::MonotonicTimeline>(remaining_written).unwrap(),
                zx::MonotonicDuration::from_seconds(2)
            );
            let elapsed = test_clock.read().unwrap() - before;
            thread_join_handle.join().unwrap();

            assert_geq!(
                elapsed + duration_from_timespec(remaining_written).unwrap(),
                UtcDuration::from_seconds(2)
            );
        })
        .await;
    }
}
