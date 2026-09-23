// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::future::Future;
use std::panic::Location;
use std::time::Duration;

use fuchsia_async as fasync;
use futures::StreamExt;
use zx::MonotonicDuration;

/// Extension trait for `Future` to provide logging capabilities for long-running operations.
///
/// This trait allows for monitoring asynchronous operations and emitting warnings if they
/// take longer than expected. It uses `#[track_caller]` to ensure that the reported
/// file and line numbers in the logs refer to the call site of the extension method.
///
/// # Examples
///
/// ```ignore
/// use std::time::Duration;
///
/// // Log a warning every 10 seconds until the future completes.
/// fut.warn_every(Duration::from_secs(10), "read register").await;
///
/// // Log a single warning after 5 seconds if the future is still pending.
/// fut.warn_after(Duration::from_secs(5), "acquire lease").await;
/// ```
pub trait FutureLogExt: Future + Sized {
    /// Returns a future that logs a warning if `self` does not complete within `duration`.
    ///
    /// If the operation exceeds the duration, a single warning is logged including the
    /// provided `msg` and the caller's location.
    fn warn_after(self, duration: Duration, msg: &str) -> impl Future<Output = Self::Output>;

    /// Returns a future that logs a warning every `interval` until `self` completes.
    ///
    /// If the operation exceeds the interval, a warning is logged. Subsequent warnings
    /// are emitted at the same frequency. When the operation finally completes, a
    /// "resolved" message is logged if any warnings were previously emitted.
    fn warn_every(self, interval: Duration, msg: &str) -> impl Future<Output = Self::Output>;
}

impl<F: Future> FutureLogExt for F {
    #[track_caller]
    fn warn_after(self, duration: Duration, msg: &str) -> impl Future<Output = Self::Output> {
        let caller = Location::caller();
        let caller_file = caller.file();
        let caller_line = caller.line();

        async move {
            use futures::FutureExt;
            let fut = self;
            futures::pin_mut!(fut);

            let zx_duration = MonotonicDuration::from(duration);
            let timer = fasync::Timer::new(fasync::MonotonicInstant::after(zx_duration)).fuse();
            futures::pin_mut!(timer);

            futures::select! {
                res = fut.as_mut().fuse() => res,
                _ = timer => {
                    log::warn!(
                        "[{caller_file}:{caller_line}] {msg} taking longer than {duration:?}"
                    );
                    fut.await
                }
            }
        }
    }

    #[track_caller]
    fn warn_every(self, interval: Duration, msg: &str) -> impl Future<Output = Self::Output> {
        let caller = Location::caller();
        let caller_file = caller.file();
        let caller_line = caller.line();

        async move {
            use futures::FutureExt;
            let fut = self;
            futures::pin_mut!(fut);

            let zx_interval = MonotonicDuration::from(interval);
            let mut ticker = fasync::Interval::new(zx_interval);
            let mut logged = false;

            loop {
                let tick_fut = ticker.next().fuse();
                futures::pin_mut!(tick_fut);

                futures::select! {
                    res = fut.as_mut().fuse() => {
                        if logged {
                            log::warn!(
                                "[{caller_file}:{caller_line}] unexpected blocking is now resolved: {msg}");
                        }
                        break res;
                    }
                    _ = tick_fut => {
                        log::warn!(
                            "[{caller_file}:{caller_line}] unexpected blocking: {msg} taking longer than {interval:?}",
                        );
                        logged = true;
                    }
                }
            }
        }
    }
}
