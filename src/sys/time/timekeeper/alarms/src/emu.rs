// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
//
use crate::{SawResponseFut, TimerConfig, TimerOps, TimerOpsError, signal};
use async_trait::async_trait;
use fidl_fuchsia_hardware_hrtimer as ffhh;
use fuchsia_async as fasync;
use futures::channel::mpsc;
use futures::sink::SinkExt;
use futures::{Future, StreamExt, select};
use log::debug;
use scopeguard::defer;
use std::cell::RefCell;
use std::pin::Pin;
use std::rc::Rc;

// TimerOps for platforms that do not support wake alarms.
//
// On such platforms, we only signal on timers, and expect that the platform
// is always awake, and can not go to sleep.
#[derive(Debug)]
pub(crate) struct EmulationTimerOps {
    // When populated, indicates that a timer is active. The timer protocol forbids calling
    // start_and_wait twice, so this is used to check for protocol errors.
    stop_snd: Rc<RefCell<Option<mpsc::Sender<()>>>>,
}

impl EmulationTimerOps {
    pub(crate) fn new() -> Self {
        Self { stop_snd: Rc::new(RefCell::new(None)) }
    }
}

#[async_trait(?Send)]
impl TimerOps for EmulationTimerOps {
    async fn stop(&self, _id: u64) {
        if let Some(snd) = self.stop_snd.borrow().as_ref() {
            let mut snd_clone = snd.clone();
            snd_clone.send(()).await.expect("always able to send")
        }
        // Mark the timer as inactive.
        let _ = self.stop_snd.borrow_mut().take();
    }

    async fn get_timer_properties(&self) -> TimerConfig {
        fasync::Timer::new(fasync::BootInstant::after(zx::BootDuration::ZERO)).await; // yield.
        TimerConfig::new_from_data(0, &[zx::BootDuration::from_nanos(1000)], i64::MAX as u64)
    }

    fn start_and_wait(
        &self,
        id: u64,
        resolution: &ffhh::Resolution,
        ticks: u64,
        setup_event: zx::Event,
    ) -> std::pin::Pin<Box<dyn SawResponseFut>> {
        let ticks: i64 = ticks.try_into().unwrap();
        if let Some(_) = *self.stop_snd.borrow() {
            // We already have one timer going on, this is bad state.
            return Box::pin(ForwardingFuture {
                inner_future: Box::pin(async move {
                    Err(TimerOpsError::Driver(ffhh::DriverError::BadState))
                }),
            });
        }
        defer!(
                signal(&setup_event);
                debug!("emu/start_and_wait: START: setup_event signaled");
        );
        let (stop_snd, mut stop_rcv) = mpsc::channel(1);
        let tick_duration = match resolution {
            ffhh::Resolution::Duration(d) => *d,
            _ => 0,
        };
        let sleep_duration = zx::BootDuration::from_nanos(ticks * tick_duration);
        let post_cancel = self.stop_snd.clone();
        let fut = Box::pin(async move {
            defer! {
                // Mark the timer as inactive once the future completes.
                let _ = post_cancel.borrow_mut().take();
            };
            let ret = select! {
                _ = fasync::Timer::new(fasync::BootInstant::after(sleep_duration)) => {
                    let (one, _) = zx::EventPair::create();
                    Ok(one)
                },
                _ = stop_rcv.next() => {
                    debug!("CANCELED: timer {id} canceled");
                    Err(TimerOpsError::Driver(ffhh::DriverError::Canceled))
                },
            };
            ret
        });
        *self.stop_snd.borrow_mut() = Some(stop_snd);
        Box::pin(ForwardingFuture { inner_future: Box::pin(fut) })
    }
}

struct ForwardingFuture<F: Future> {
    inner_future: Pin<Box<F>>,
}

use std::task::{Context, Poll};
type FRet = Result<zx::EventPair, TimerOpsError>;
impl<F: Future<Output = FRet>> SawResponseFut for ForwardingFuture<F> {}
impl<F: Future<Output = FRet>> Future for ForwardingFuture<F> {
    type Output = F::Output;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.inner_future.as_mut().poll(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use std::task::Poll;

    const FAKE_ID: u64 = 42;

    #[fuchsia::test]
    fn test_alarm_triggers() {
        let mut executor = fasync::TestExecutor::new_with_fake_time();
        executor.set_fake_time(fasync::MonotonicInstant::from_nanos(0));
        let ops = EmulationTimerOps::new();
        let setup_event = zx::Event::create();

        // Event is not signaled at the beginning.
        let maybe_signaled = setup_event
            .as_handle_ref()
            .wait_one(zx::Signals::EVENT_SIGNALED, zx::MonotonicInstant::INFINITE_PAST);
        assert_matches!(maybe_signaled, zx::WaitResult::TimedOut(zx::Signals::NONE));

        let mut start_fut = ops.start_and_wait(
            FAKE_ID,
            // 10us total.
            &ffhh::Resolution::Duration(1000),
            10,
            setup_event.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
        );

        // Try polling now - future isn't ready yet.
        let res = executor.run_until_stalled(&mut start_fut);
        assert_matches!(res, Poll::Pending);

        // Check that we signaled setup_event, so that callers can unblock.
        let maybe_signaled = setup_event
            .as_handle_ref()
            .wait_one(zx::Signals::EVENT_SIGNALED, zx::MonotonicInstant::INFINITE_PAST);
        assert_matches!(maybe_signaled, zx::WaitResult::Ok(zx::Signals::EVENT_SIGNALED));

        // Poll after 5us - future isn't ready yet.
        executor.set_fake_time(fasync::MonotonicInstant::from_nanos(5_000));
        executor.wake_expired_timers();
        let res = executor.run_until_stalled(&mut start_fut);
        assert_matches!(res, Poll::Pending);

        // Poll past 10us - future is ready.
        executor.set_fake_time(fasync::MonotonicInstant::from_nanos(11_000));
        executor.wake_expired_timers();
        let res = executor.run_until_stalled(&mut start_fut);
        assert_matches!(res, Poll::Ready(Ok(_)));
    }

    #[fuchsia::test]
    fn test_alarm_cancelation() {
        let mut executor = fasync::TestExecutor::new_with_fake_time();
        executor.set_fake_time(fasync::MonotonicInstant::from_nanos(0));
        let ops = EmulationTimerOps::new();
        let setup_event = zx::Event::create();
        let mut start_fut = ops.start_and_wait(
            FAKE_ID,
            // 10us total.
            &ffhh::Resolution::Duration(1000),
            10,
            setup_event.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
        );

        // Try polling now - future isn't ready yet.
        let res = executor.run_until_stalled(&mut start_fut);
        assert_matches!(res, Poll::Pending);

        executor.set_fake_time(fasync::MonotonicInstant::from_nanos(5_000));
        executor.wake_expired_timers();
        let res = executor.run_until_stalled(&mut start_fut);
        assert_matches!(res, Poll::Pending);

        let mut stop_fut = ops.stop(FAKE_ID);
        let res = executor.run_until_stalled(&mut stop_fut);
        assert_matches!(res, Poll::Ready(_));

        // Attempt to poll now and note that the alarm is now canceled.
        let res = executor.run_until_stalled(&mut start_fut);
        assert_matches!(res, Poll::Ready(Err(TimerOpsError::Driver(ffhh::DriverError::Canceled))));
    }

    #[fuchsia::test]
    fn test_alarm_start_twice_is_error() {
        let mut executor = fasync::TestExecutor::new_with_fake_time();
        executor.set_fake_time(fasync::MonotonicInstant::from_nanos(0));
        let ops = EmulationTimerOps::new();
        let setup_event = zx::Event::create();
        let mut start_fut = ops.start_and_wait(
            FAKE_ID,
            // 10us total.
            &ffhh::Resolution::Duration(1000), // 1us per tick
            10,
            setup_event.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
        );

        // Try polling now - future isn't ready yet.
        let res = executor.run_until_stalled(&mut start_fut);
        assert_matches!(res, Poll::Pending);

        // Try to start another time while one is already active, is an error.
        let mut start_fut_2 = ops.start_and_wait(
            FAKE_ID,
            &ffhh::Resolution::Duration(1000),
            10,
            setup_event.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
        );
        let res = executor.run_until_stalled(&mut start_fut_2);
        assert_matches!(res, Poll::Ready(Err(TimerOpsError::Driver(ffhh::DriverError::BadState))));
    }

    #[fuchsia::test]
    fn test_alarm_start_stop_start() {
        let mut executor = fasync::TestExecutor::new_with_fake_time();
        executor.set_fake_time(fasync::MonotonicInstant::from_nanos(0));
        let ops = EmulationTimerOps::new();
        let setup_event = zx::Event::create();
        let mut start_fut = ops.start_and_wait(
            FAKE_ID,
            // 10us total.
            &ffhh::Resolution::Duration(1000),
            10,
            setup_event.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
        );

        executor.set_fake_time(fasync::MonotonicInstant::from_nanos(5_000));
        executor.wake_expired_timers();
        let res = executor.run_until_stalled(&mut start_fut);
        assert_matches!(res, Poll::Pending);

        let mut stop_fut = ops.stop(FAKE_ID);
        let res = executor.run_until_stalled(&mut stop_fut);
        assert_matches!(res, Poll::Ready(_));

        // Start a new alarm.
        let mut start_fut = ops.start_and_wait(
            FAKE_ID,
            // 10us total.
            &ffhh::Resolution::Duration(1000),
            10,
            setup_event.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
        );

        let res = executor.run_until_stalled(&mut start_fut);
        assert_matches!(res, Poll::Pending);

        executor.set_fake_time(fasync::MonotonicInstant::from_nanos(50_000));
        executor.wake_expired_timers();
        let res = executor.run_until_stalled(&mut start_fut);
        assert_matches!(res, Poll::Ready(_));
    }

    #[fuchsia::test]
    fn test_alarm_start_expire_start() {
        let mut executor = fasync::TestExecutor::new_with_fake_time();
        executor.set_fake_time(fasync::MonotonicInstant::from_nanos(0));
        let ops = EmulationTimerOps::new();
        let setup_event = zx::Event::create();
        let mut start_fut = ops.start_and_wait(
            FAKE_ID,
            // 10us total.
            &ffhh::Resolution::Duration(1000),
            10,
            setup_event.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
        );
        let res = executor.run_until_stalled(&mut start_fut);
        assert_matches!(res, Poll::Pending);

        executor.set_fake_time(fasync::MonotonicInstant::from_nanos(20_000));
        executor.wake_expired_timers();
        let res = executor.run_until_stalled(&mut start_fut);
        assert_matches!(res, Poll::Ready(_));

        // Start a new alarm.
        let mut start_fut = ops.start_and_wait(
            FAKE_ID,
            // 10us total.
            &ffhh::Resolution::Duration(1000),
            10,
            setup_event.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
        );
        let res = executor.run_until_stalled(&mut start_fut);
        assert_matches!(res, Poll::Pending);

        // Ensure the timer expires, and check that it has expired.
        executor.set_fake_time(fasync::MonotonicInstant::from_nanos(50_000));
        executor.wake_expired_timers();
        let res = executor.run_until_stalled(&mut start_fut);
        assert_matches!(res, Poll::Ready(_));
    }
}
