// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Provides utilities for working with futures.

use std::pin::Pin;

use futures::future::FusedFuture;
use futures::{task, Future};

/// A future that yields to the executor only once.
///
/// This future returns [`Poll::Pending`] the first time it's polled after
/// waking the context waker. This effectively yields the currently running task
/// to the executor, but puts it back in the executor's ready task queue.
///
/// Example:
/// ```
/// loop {
///   let read = read_big_thing().await;
///
///   while let Some(x) = read.next() {
///     process_one_thing(x);
///     YieldToExecutorOnce::new().await;
///   }
/// }
/// ```
#[derive(Default)]
pub struct YieldToExecutorOnce(YieldToExecutorOnceInner);

#[derive(Default)]
enum YieldToExecutorOnceInner {
    #[default]
    NotPolled,
    Ready,
    Terminated,
}

impl YieldToExecutorOnce {
    /// Creates a new `YieldToExecutorOnce`.
    pub fn new() -> Self {
        Self::default()
    }
}

impl Future for YieldToExecutorOnce {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> task::Poll<Self::Output> {
        let Self(inner) = self.get_mut();
        match *inner {
            YieldToExecutorOnceInner::NotPolled => {
                *inner = YieldToExecutorOnceInner::Ready;
                // Wake the executor before returning pending. We only want to yield
                // once.
                cx.waker().wake_by_ref();
                task::Poll::Pending
            }
            YieldToExecutorOnceInner::Ready => {
                *inner = YieldToExecutorOnceInner::Terminated;
                task::Poll::Ready(())
            }
            YieldToExecutorOnceInner::Terminated => {
                panic!("polled future after completion");
            }
        }
    }
}

impl FusedFuture for YieldToExecutorOnce {
    fn is_terminated(&self) -> bool {
        let Self(inner) = self;
        match inner {
            YieldToExecutorOnceInner::Ready | YieldToExecutorOnceInner::NotPolled => false,
            YieldToExecutorOnceInner::Terminated => true,
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn yield_to_executor_once() {
        use futures::future::FusedFuture as _;
        use futures::FutureExt as _;

        let (waker, count) = futures_test::task::new_count_waker();
        let mut context = std::task::Context::from_waker(&waker);
        let mut fut = super::YieldToExecutorOnce::new();

        assert!(!fut.is_terminated());
        assert_eq!(count, 0);
        assert_eq!(fut.poll_unpin(&mut context), std::task::Poll::Pending);
        assert!(!fut.is_terminated());
        assert_eq!(count, 1);
        assert_eq!(fut.poll_unpin(&mut context), std::task::Poll::Ready(()));
        assert!(fut.is_terminated());
        // The waker is never hit again.
        assert_eq!(count, 1);
    }
}
