// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::signals::{SignalEvent, SignalEventNotify, SignalEventValue};
use crate::task::CurrentTask;
use crate::time::interval_timer::{IntervalTimer, IntervalTimerHandle};
use crate::time::{Timeline, TimerWakeup};
use starnix_sync::{LockDepMutex, TimerTableStateLock};
use starnix_uapi::errors::Errno;
use starnix_uapi::signals::SIGALRM;
use starnix_uapi::{TIMER_ABSTIME, error, itimerspec, uapi};
use std::collections::HashMap;

static_assertions::const_assert!(
    std::mem::size_of::<uapi::__kernel_timer_t>()
        == std::mem::size_of::<uapi::arch32::__kernel_timer_t>()
);
pub type TimerId = uapi::__kernel_timer_t;

static_assertions::const_assert!(
    std::mem::size_of::<uapi::__kernel_clockid_t>()
        == std::mem::size_of::<uapi::arch32::__kernel_clockid_t>()
);
pub type ClockId = uapi::__kernel_clockid_t;

// Table for POSIX timers from timer_create() that deliver timers via signals (not new-style
// timerfd's).
#[derive(Debug, Default)]
pub struct TimerTable {
    state: LockDepMutex<TimerTableMutableState, TimerTableStateLock>,
}

#[derive(Debug)]
struct TimerTableMutableState {
    /// The `TimerId` at which allocation should begin searching for an unused ID.
    next_timer_id: TimerId,
    timers: HashMap<TimerId, IntervalTimerHandle>,
    itimer_real: IntervalTimerHandle,
}

impl Default for TimerTableMutableState {
    fn default() -> Self {
        let signal_event =
            SignalEvent::new(SignalEventValue(0), SIGALRM, SignalEventNotify::Signal);
        let itimer_real =
            IntervalTimer::new(0, Timeline::RealTime, TimerWakeup::Regular, signal_event)
                .expect("Failed to create itimer_real");
        TimerTableMutableState {
            itimer_real,
            timers: Default::default(),
            next_timer_id: Default::default(),
        }
    }
}

impl TimerTable {
    /// Creates a new per-process interval timer.
    ///
    /// The new timer is initially disarmed.
    pub fn create(
        &self,
        timeline: Timeline,
        wakeup_type: TimerWakeup,
        signal_event: Option<SignalEvent>,
    ) -> Result<TimerId, Errno> {
        let mut state = self.state.lock();

        // Find a vacant timer id.
        let end = state.next_timer_id;
        let timer_id = loop {
            let timer_id = state.next_timer_id;
            state.next_timer_id += 1;

            if state.next_timer_id == TimerId::MAX {
                state.next_timer_id = 0;
            } else if state.next_timer_id == end {
                // After searching the entire timer map, there is no vacant timer id.
                // Fails the call and implies the program could try it again later.
                return error!(EAGAIN);
            }

            if !state.timers.contains_key(&timer_id) {
                break timer_id;
            }
        };

        state.timers.insert(
            timer_id,
            IntervalTimer::new(
                timer_id,
                timeline,
                wakeup_type,
                signal_event.unwrap_or_else(|| {
                    SignalEvent::new(
                        SignalEventValue(timer_id as u64),
                        SIGALRM,
                        SignalEventNotify::Signal,
                    )
                }),
            )?,
        );

        Ok(timer_id)
    }

    pub fn itimer_real(&self) -> IntervalTimerHandle {
        self.state.lock().itimer_real.clone()
    }

    /// Disarms and deletes a timer.
    pub fn delete(&self, current_task: &CurrentTask, id: TimerId) -> Result<(), Errno> {
        let mut state = self.state.lock();
        match state.timers.remove_entry(&id) {
            Some(entry) => entry.1.disarm(current_task),
            None => error!(EINVAL),
        }
    }

    /// Fetches the time remaining until the next expiration of a timer, along with the interval
    /// setting of the timer.
    pub fn get_time(&self, id: TimerId) -> Result<itimerspec, Errno> {
        Ok(self.get_timer(id)?.time_remaining().into())
    }

    /// Returns the overrun count for the last timer expiration.
    pub fn get_overrun(&self, id: TimerId) -> Result<i32, Errno> {
        Ok(self.get_timer(id)?.overrun_last())
    }

    /// Arms (start) or disarms (stop) the timer identifierd by `id`. The `new_value` arg is a
    /// pointer to an `itimerspec` structure that specifies the new initial value and the new
    /// interval for the timer.
    pub fn set_time(
        &self,
        current_task: &CurrentTask,
        id: TimerId,
        flags: i32,
        new_value: itimerspec,
    ) -> Result<itimerspec, Errno> {
        let itimer = self.get_timer(id)?;
        let old_value: itimerspec = itimer.time_remaining().into();
        if new_value.it_value.tv_sec != 0 || new_value.it_value.tv_nsec != 0 {
            let is_absolute = flags == TIMER_ABSTIME as i32;
            itimer.arm(current_task, new_value, is_absolute)?;
        } else {
            itimer.disarm(current_task)?;
        }

        Ok(old_value)
    }

    pub fn get_timer(&self, id: TimerId) -> Result<IntervalTimerHandle, Errno> {
        match self.state.lock().timers.get(&id) {
            Some(itimer) => Ok(itimer.clone()),
            None => error!(EINVAL),
        }
    }
}
