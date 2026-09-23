// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::fs::fuchsia::{BootZxTimer, MonotonicZxTimer};
use crate::power::OnWakeOps;
use crate::task::{
    CurrentTask, EventHandler, Kernel, SignalHandler, SignalHandlerInner, WaitCanceler, Waiter,
};
use crate::time::{GenericDuration, HrTimer, TargetTime, Timeline, TimerWakeup};
use crate::vfs::buffers::{InputBuffer, OutputBuffer};
use crate::vfs::{
    Anon, FileHandle, FileObject, FileObjectState, FileOps, fileops_impl_nonseekable,
    fileops_impl_noop_sync,
};
use futures::channel::mpsc;
use starnix_logging::{log_debug, log_warn};
use starnix_sync::{LockDepMutex, TimerFileInfoLock};
use starnix_types::time::{duration_from_timespec, timespec_from_duration, timespec_is_zero};
use starnix_uapi::errors::Errno;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::vfs::FdEvents;
use starnix_uapi::{TFD_TIMER_ABSTIME, TFD_TIMER_CANCEL_ON_SET, error, itimerspec};
use std::sync::{Arc, Weak};
use zerocopy::IntoBytes;
use zx::HandleRef;

pub trait TimerOps: Send + Sync + 'static {
    /// Starts the timer with the specified `deadline`.
    ///
    /// This method should start the timer and schedule it to trigger at the specified `deadline`.
    /// The timer should be cancelled if it is already running.
    fn start(
        &self,
        current_task: &CurrentTask,
        source: Option<Weak<dyn OnWakeOps>>,
        deadline: TargetTime,
    ) -> Result<(), Errno>;

    /// Stops the timer.
    ///
    /// This method should stop the timer and prevent it from triggering.
    fn stop(&self, kernel: &Arc<Kernel>) -> Result<(), Errno>;

    /// Returns a reference to the underlying Zircon handle.
    fn as_handle_ref(&self) -> HandleRef<'_>;

    /// For TimerOps that support monitoring timeline changes (e.g. timers on the
    /// UTC timeline), this returns a an object that counts the number of timeline
    /// changes since last reset.
    ///
    /// The caller must reset this value to restart the counting.
    fn get_timeline_change_observer(
        &self,
        current_task: &CurrentTask,
    ) -> Option<TimelineChangeObserver>;
}

/// Used to observe timeline changes from within TimerOps.
#[derive(Debug)]
pub struct TimelineChangeObserver {
    // Stores the number of timeline changes observed since initial watch, or
    // timeline reset.
    timeline_change_counter: zx::Counter,
    // Used to indicate timeline change interest.
    timeline_change_registration: mpsc::UnboundedSender<bool>,
}

impl Drop for TimelineChangeObserver {
    // Ensure that a lingering registration does not get retained.
    fn drop(&mut self) {
        self.set_timeline_change_interest(false);
    }
}

impl TimelineChangeObserver {
    pub fn new(
        timeline_change_counter: zx::Counter,
        timeline_change_registration: mpsc::UnboundedSender<bool>,
    ) -> Self {
        Self { timeline_change_counter, timeline_change_registration }
    }

    /// Resets the change counter, and returns the observed value.
    ///
    /// The reset is done in such a way that any "sets" that come while "reset" is running does not
    /// lose a "set".
    ///
    /// What is expected here is that at the end either (1) COUNTER_POSITIVE is cleared, or (2) if
    /// it happens not to be cleared due to a race, that Starnix has a way to probe the
    /// COUNTER_POSITIVE and react *again* if it remains asserted (without being strobed to zero)
    /// after this call returns.
    ///
    /// (2) happens to be true today due to how file ops work, and will likely continue to be so.
    /// But it's a tad bit disconcerting that the correct operation of this counter depends on two
    /// bits of code that are somewhat far away from each other. A safer alternative would be
    /// a "counter swap" operation that would write a value *and* return the old value atomically,
    /// but that does not exist today.
    pub fn reset_timeline_change_counter(&self) -> i64 {
        let counter = &self.timeline_change_counter;
        let value = counter.read().expect("it is possible to read the counter");
        if value != 0 {
            // We do not write a zero here, but write a number that neutralizes the effect of
            // `value`. Writing a zero would lose a concurrent "add" from the producer side if
            // the add came between the `read` and `write zero`.
            //
            // This approach will, for the same reason, not necessarily strobe the
            // COUNTER_POSITIVE signal, but the way Starnix handles async waits will ensure
            // that the signal value will not be missed.
            counter.add(-value).expect("it is possible to set the counter to zero");
        }
        return value;
    }

    pub fn get_timeline_change_counter_ref(&self) -> &zx::Counter {
        &self.timeline_change_counter
    }

    pub fn set_timeline_change_interest(&mut self, is_interested: bool) {
        // Unbounded send on an async channel is OK.
        self.timeline_change_registration.unbounded_send(is_interested).expect("can send");
    }
}

/// Deadline interval information for this [TimerFile].
///
/// When the file is read, the deadline is recomputed based on the current time and the set
/// interval. If the interval is 0, `self.timer` is cancelled after the file is read.
#[derive(Debug)]
pub struct TimerFileInfo {
    /// The timeout, expressed as a target value in the chosen timeline.
    deadline: TargetTime,
    /// The period for interval timer repeat. `0` for timers that do not repeat.
    interval: zx::MonotonicDuration,
    /// Incremented when a timeline change occurs. We must set this to zero manually
    /// if we want to get repeated signaling. This timer is not shared with any
    /// other `TimerFileInfo`s, so the signaling protocol should not depend on
    /// how many timers are active.
    timeline_change_observer: Option<TimelineChangeObserver>,
    /// If set, this timer must return ECANCELED when read. (The read unblocks
    /// when there are UTC timeline changes.)
    cancel_on_set: bool,
}

impl TimerFileInfo {
    pub fn new(next_deadline: TargetTime, interval_period: zx::MonotonicDuration) -> Self {
        Self {
            deadline: next_deadline,
            interval: interval_period,
            timeline_change_observer: None,
            cancel_on_set: false,
        }
    }

    pub fn reset_timeline_change_counter(&self) -> i64 {
        if let Some(observer) = self.timeline_change_observer.as_ref() {
            return observer.reset_timeline_change_counter();
        } else {
            0
        }
    }

    pub fn set_deadline(&mut self, new_deadline: TargetTime) -> &mut Self {
        self.deadline = new_deadline;
        self
    }

    pub fn set_interval(&mut self, new_interval: zx::MonotonicDuration) -> &mut Self {
        self.interval = new_interval;
        self
    }

    /// Set the counter used for tracking changes to the underlying timeline. This counter gets
    /// incremented on each timeline change by Starnix from within `HrTimerManager`.
    pub fn set_timeline_change_observer(
        &mut self,
        observer: Option<TimelineChangeObserver>,
    ) -> &mut Self {
        self.timeline_change_observer = observer;
        self
    }

    /// Mark the timer as "cancel on set". Such a timer will report ECANCELED on read if armed with
    /// a zero valued realtime timeline, but will report data available for read when used in
    /// epoll.
    pub fn set_cancel_on_set(&mut self, value: bool) -> &mut Self {
        self.cancel_on_set = value;
        self.timeline_change_observer
            .as_mut()
            .map(|observer| observer.set_timeline_change_interest(value));
        self
    }
}

/// A `TimerFile` represents a file created by `timerfd_create`.
///
/// Clients can read the number of times the timer has triggered from the file. The file supports
/// blocking reads, waiting for the timer to trigger.
pub struct TimerFile {
    /// The timer that is used to wait for blocking reads.
    timer: Arc<dyn TimerOps>,

    /// The type of clock this file was created with.
    timeline: Timeline,

    /// Whether this timer can wake up the system.
    wakeup_type: TimerWakeup,

    /// Details about the timeline, deadline and cancel behavior requested from this
    /// [TimerFile].
    timer_file_info: Arc<LockDepMutex<TimerFileInfo, TimerFileInfoLock>>,
}

impl TimerFile {
    /// Creates a new anonymous `TimerFile` in `kernel`.
    ///
    /// Returns an error if the `zx::Timer` could not be created.
    pub fn new_file(
        current_task: &CurrentTask,
        wakeup_type: TimerWakeup,
        timeline: Timeline,
        flags: OpenFlags,
    ) -> Result<FileHandle, Errno> {
        let timer: Arc<dyn TimerOps> = match (wakeup_type, timeline) {
            (TimerWakeup::Regular, Timeline::Monotonic) => Arc::new(MonotonicZxTimer::new()),
            (TimerWakeup::Regular, Timeline::BootInstant) => Arc::new(BootZxTimer::new()),
            (TimerWakeup::Regular, Timeline::RealTime)
            | (TimerWakeup::Alarm, Timeline::BootInstant | Timeline::RealTime) => {
                Arc::new(HrTimer::new())
            }
            (TimerWakeup::Alarm, Timeline::Monotonic) => {
                unreachable!("monotonic times cannot be alarm deadlines")
            }
        };

        let mut timer_file_info =
            TimerFileInfo::new(timeline.zero_time(), zx::MonotonicDuration::default());

        if timeline.is_realtime() {
            // Realtime timers must also track changes of the UTC timeline, so we configure
            // that here. In theory we could only do this when timerfd_settime is called.
            // However, it turns out that the appropriate wait_asyncs can be requested before
            // the timer is started with proper flags, such as when the timer is used in
            // `epoll`. This means we have to create this setup at timer creation.
            timer_file_info
                .set_timeline_change_observer(timer.get_timeline_change_observer(current_task));
        }

        Ok(Anon::new_private_file(
            current_task,
            Box::new(TimerFile {
                timer,
                timeline,
                wakeup_type,
                timer_file_info: Arc::new(timer_file_info.into()),
            }),
            flags,
            "[timerfd]",
        ))
    }

    pub fn wakeup_type(&self) -> TimerWakeup {
        self.wakeup_type
    }

    /// Returns the current `itimerspec` for the file.
    ///
    /// The returned `itimerspec.it_value` contains the amount of time remaining until the
    /// next timer trigger.
    pub fn current_timer_spec(&self) -> itimerspec {
        let (deadline, interval) = {
            let guard = self.timer_file_info.lock();
            (guard.deadline, guard.interval)
        };
        let now = self.timeline.now();
        let remaining_time = if interval == zx::MonotonicDuration::default() && deadline <= now {
            timespec_from_duration(zx::MonotonicDuration::default())
        } else {
            timespec_from_duration(
                *deadline.delta(&now).expect("deadline and now come from same timeline"),
            )
        };

        itimerspec { it_interval: timespec_from_duration(interval), it_value: remaining_time }
    }

    /// Sets the `itimerspec` for the timer, which will either update the associated `zx::Timer`'s
    /// scheduled trigger or cancel the timer.
    ///
    /// Returns the previous `itimerspec` on success.
    pub fn set_timer_spec(
        &self,
        current_task: &CurrentTask,
        file_object: &FileObject,
        timer_spec: itimerspec,
        flags: u32,
    ) -> Result<itimerspec, Errno> {
        let mut tfi = self.timer_file_info.lock();
        // On each timer "set", we need to figure out if we want to set this flag again, since each
        // timer can be used in both "cancel or set" or "regular" flavors over its lifetim.  Reset
        // it here first unconditionally, then set again below, if the right combination of
        // settings comes along.
        tfi.set_cancel_on_set(false);
        let old_itimerspec = tfi.deadline.itimerspec(tfi.interval);

        if timespec_is_zero(timer_spec.it_value) {
            // Sayeth timerfd_settime(2):
            // Setting both fields of new_value.it_value to zero disarms the timer.
            tfi.set_deadline(self.timeline.zero_time()).set_interval(zx::MonotonicDuration::ZERO);
            self.timer.stop(current_task.kernel())?;

            // Also sayeth timerfd_settime(2):
            // TFD_TIMER_CANCEL_ON_SET
            // If this flag is specified along with TFD_TIMER_ABSTIME and
            // the clock for this timer is CLOCK_REALTIME or
            // CLOCK_REALTIME_ALARM, then mark this timer as cancelable if
            // the real-time clock undergoes a discontinuous change
            // (settimeofday(2), clock_settime(2), or similar).  When such
            // changes occur, a current or future read(2) from the file
            // descriptor will fail with the error ECANCELED.
            if (flags & TFD_TIMER_ABSTIME != 0)
                && (flags & TFD_TIMER_CANCEL_ON_SET != 0)
                && self.timeline.is_realtime()
            {
                // This timer is configured as "cancel on set", so mark it as such
                // to allow wait_async to be configured properly. "Cancel on set"
                // timers don't get scheduled anywhere, they just monitor timeline
                // changes, so we don't need to start anything here.
                tfi.set_cancel_on_set(true);
            }
        } else {
            let new_deadline = if flags & TFD_TIMER_ABSTIME != 0 {
                // If the time_spec represents an absolute time, then treat the
                // `it_value` as the deadline..
                self.timeline.target_from_timespec(timer_spec.it_value)?
            } else {
                // .. otherwise the deadline is computed relative to the current time.
                self.timeline.now()
                    + GenericDuration::from(duration_from_timespec::<zx::SyntheticTimeline>(
                        timer_spec.it_value,
                    )?)
            };
            let new_interval = duration_from_timespec(timer_spec.it_interval)?;

            self.timer.start(current_task, Some(file_object.weak_handle.clone()), new_deadline)?;
            tfi.set_deadline(new_deadline).set_interval(new_interval);
        }

        Ok(old_itimerspec)
    }

    /// Returns the `zx::Signals` to listen for given `events`. Used to wait on the `TimerOps`
    /// associated with a `TimerFile`.
    fn get_signals_from_events(events: FdEvents) -> zx::Signals {
        if events.contains(FdEvents::POLLIN) {
            zx::Signals::TIMER_SIGNALED
        } else {
            zx::Signals::NONE
        }
    }

    fn get_events_from_signals(signals: zx::Signals) -> FdEvents {
        let mut events = FdEvents::empty();

        if signals.contains(zx::Signals::TIMER_SIGNALED) {
            events |= FdEvents::POLLIN;
        }
        events
    }

    /// Converts the events that can happen on a `zx::Counter` to
    /// corresponding [FdEvents] on a timer. This is used when polling
    /// for timeline changes on the timers.
    fn get_counter_events_from_signals(signals: zx::Signals) -> FdEvents {
        let mut events = FdEvents::empty();

        if signals.contains(zx::Signals::COUNTER_POSITIVE) {
            events |= FdEvents::POLLIN;
        }
        events
    }
}

impl FileOps for TimerFile {
    fileops_impl_nonseekable!();
    fileops_impl_noop_sync!();

    fn close(self: Box<Self>, _file: &FileObjectState, current_task: &CurrentTask) {
        if let Err(e) = self.timer.stop(current_task.kernel()) {
            log_warn!("Failed to stop the timer when closing the timerfd: {e:?}");
        }
    }

    fn write(
        &self,
        file: &FileObject,
        _current_task: &CurrentTask,
        offset: usize,
        _data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        debug_assert!(offset == 0);
        // The expected error seems to vary depending on the open flags..
        if file.flags().contains(OpenFlags::NONBLOCK) { error!(EINVAL) } else { error!(ESPIPE) }
    }

    fn read(
        &self,
        file: &FileObject,
        current_task: &CurrentTask,
        offset: usize,
        data: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno> {
        debug_assert!(offset == 0);
        file.blocking_op(current_task, FdEvents::POLLIN | FdEvents::POLLHUP, None, || {
            let mut tfi = self.timer_file_info.lock();
            let is_cancel_on_set = tfi.cancel_on_set;
            // A "cancel-on-set" timer, this was prepared in `set_timer_spec`. It is handled
            // specially.
            if is_cancel_on_set {
                if tfi.reset_timeline_change_counter() != 0 {
                    // Timeline has changed, we communicate that by returning ECANCELED
                    // to the reader. `data` is ignored.
                    return error!(ECANCELED);
                }
                // The timer state has not changed, tell the caller to try again later.
                return error!(EAGAIN);
            }

            if tfi.deadline.is_zero() {
                return error!(EAGAIN);
            }

            let now = self.timeline.now();
            log_debug!(
                "read:\n\tnow={now:?}\n\ttfi={:?}\n\tnow_boot={:?}",
                zx::MonotonicInstant::get(),
                tfi.deadline
            );
            if tfi.deadline > now {
                // The next deadline has not yet passed.
                return error!(EAGAIN);
            }

            let count: i64 = if tfi.interval > zx::MonotonicDuration::default() {
                let elapsed_nanos =
                    now.delta(&tfi.deadline).expect("timelines must match").into_nanos();
                // The number of times the timer has triggered is written to `data`.
                let num_intervals = elapsed_nanos / tfi.interval.into_nanos() + 1;
                let new_deadline =
                    tfi.deadline + GenericDuration::from(tfi.interval * num_intervals);

                // The timer is set to clear the `ZX_TIMER_SIGNALED` signal until the next deadline
                // is reached.
                self.timer.start(current_task, Some(file.weak_handle.clone()), new_deadline)?;
                tfi.set_deadline(new_deadline);

                /*count=*/
                num_intervals
            } else {
                tfi.set_deadline(self.timeline.zero_time())
                    .set_interval(zx::MonotonicDuration::ZERO)
                    .set_cancel_on_set(false);
                // The timer is non-repeating, so cancel the timer to clear the `ZX_TIMER_SIGNALED`
                // signal.
                self.timer.stop(current_task.kernel())?;

                /*count=*/
                1
            };

            data.write(count.as_bytes())
        })
    }

    fn wait_async(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        waiter: &Waiter,
        events: FdEvents,
        event_handler: EventHandler,
    ) -> Option<WaitCanceler> {
        let signal_handler = SignalHandler {
            inner: SignalHandlerInner::ZxHandle(TimerFile::get_events_from_signals),
            event_handler: event_handler.clone(),
            err_code: None,
        };
        let canceler = waiter
            .wake_on_zircon_signals(
                &self.timer.as_handle_ref(),
                TimerFile::get_signals_from_events(events),
                signal_handler,
            )
            .expect("TODO return error");
        //
        let cancel_timeline_change = {
            // For timers that support timeline change notifications, set up an additional wake
            // option on the counter that tallies the occurrences of timeline changes.
            //
            // Note that such a counter will not get notified unless the timer is also configured
            // to receive such notifications.
            if !self.timeline.is_realtime() {
                None
            } else {
                let guard = self.timer_file_info.lock();
                guard.timeline_change_observer.as_ref().map(|obs| {
                    let handler = SignalHandler {
                        inner: SignalHandlerInner::ZxHandle(
                            TimerFile::get_counter_events_from_signals,
                        ),
                        event_handler,
                        err_code: None,
                    };
                    waiter
                        .wake_on_zircon_signals(
                            obs.get_timeline_change_counter_ref(),
                            zx::Signals::COUNTER_POSITIVE,
                            handler,
                        )
                        .expect("TODO return error")
                })
            }
        };
        let mut cancel = WaitCanceler::new_port(canceler);
        if let Some(cancel_timeline_change) = cancel_timeline_change {
            let cancel_timeline_change = WaitCanceler::new_port(cancel_timeline_change);
            cancel = cancel.merge(cancel_timeline_change);
        }

        Some(cancel)
    }

    fn query_events(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
    ) -> Result<FdEvents, Errno> {
        let guard = self.timer_file_info.lock();
        let counter_signals = guard
            .timeline_change_observer
            .as_ref()
            .map(|observer| {
                observer
                    .get_timeline_change_counter_ref()
                    .wait_one(zx::Signals::COUNTER_POSITIVE, zx::Instant::ZERO)
                    .to_result()
            })
            // It seems that translating errors into empty signal sets is OK.
            .unwrap_or_else(|| Ok(zx::Signals::empty()))
            .unwrap_or_else(|_| zx::Signals::empty());
        let events_from_counter = TimerFile::get_counter_events_from_signals(counter_signals);

        let timer_signals = match self
            .timer
            .as_handle_ref()
            .wait_one(zx::Signals::TIMER_SIGNALED, zx::MonotonicInstant::ZERO)
            .to_result()
        {
            Err(zx::Status::TIMED_OUT) => zx::Signals::empty(),
            res => res.unwrap(),
        };
        let events_from_timer = TimerFile::get_events_from_signals(timer_signals);
        Ok(events_from_timer | events_from_counter)
    }
}
