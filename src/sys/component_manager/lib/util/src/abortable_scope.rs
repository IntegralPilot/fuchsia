// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_sync::Mutex;
use futures::channel::oneshot::{self, Canceled};
use futures::future::{FutureExt, Shared};
use futures::task::Poll;
use futures::Future;
use pin_project::pin_project;
use std::fmt::Debug;
use std::pin::Pin;
use std::sync::Arc;

/// [`AbortableScope`] transforms futures into abortable futures.
///
/// The wrapped futures can then be aborted via the [`AbortHandle`].
/// When [`AbortHandle::abort`] is called, the wrapped future will
/// complete immediately without making further progress.
#[derive(Debug)]
pub struct AbortableScope {
    rx: Shared<oneshot::Receiver<()>>,
}

/// The error returned when a future wrapped with `abortable` is aborted.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct AbortError;

/// [`AbortHandle`] allows aborting a future.
#[derive(Debug, Clone)]
pub struct AbortHandle {
    tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

impl AbortHandle {
    /// Interrupt the future as soon as possible.
    pub fn abort(&self) {
        let _ = self.tx.lock().take().map(|tx| tx.send(()));
    }
}

impl AbortableScope {
    /// Creates a scope and a handle to abort futures running in the scope.
    pub fn new() -> (AbortableScope, AbortHandle) {
        let (tx, rx) = oneshot::channel();
        (AbortableScope { rx: rx.shared() }, AbortHandle { tx: Arc::new(Mutex::new(Some(tx))) })
    }

    /// Runs a future in this scope.
    ///
    /// Returns an abort error if [`AbortHandle::abort`] is called.
    pub async fn run<T, Fut>(&self, future: Fut) -> Result<T, AbortError>
    where
        Fut: Future<Output = T> + Send,
    {
        AbortableFuture { future, abort_rx: self.rx.clone(), tx_dropped: false }.await
    }
}

pub trait AbortFutureExt<T>: Future<Output = T> + Send {
    /// Causes the future to complete with an [`InterruptError`] if the scope is aborted.
    ///
    /// Syntax sugar for `scope.run(future)`:
    ///
    /// ```
    /// let (scope, handle) = AbortableScope::new();
    /// handle.abort();
    /// some_future.with(&scope).await;   // Result<T, InterruptError>
    /// ```
    fn with(self, scope: &AbortableScope) -> impl Future<Output = Result<T, AbortError>>;
}

impl<Fut, T> AbortFutureExt<T> for Fut
where
    Fut: Future<Output = T> + Send,
{
    fn with(self, scope: &AbortableScope) -> impl Future<Output = Result<T, AbortError>> {
        scope.run(self)
    }
}

#[pin_project]
struct AbortableFuture<T, Fut: Future<Output = T> + Send, InterruptFut> {
    #[pin]
    future: Fut,
    #[pin]
    abort_rx: InterruptFut,
    tx_dropped: bool,
}

impl<T, Fut: Future<Output = T> + Send, InterruptFut: Future<Output = Result<(), Canceled>>> Future
    for AbortableFuture<T, Fut, InterruptFut>
{
    type Output = Result<T, AbortError>;

    fn poll(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let this = self.project();

        if !*this.tx_dropped {
            match this.abort_rx.poll(cx) {
                Poll::Ready(Ok(())) => return Poll::Ready(Err(AbortError)),
                Poll::Ready(Err(Canceled)) => {
                    *this.tx_dropped = true;
                }
                Poll::Pending => {}
            }
        }

        match this.future.poll(cx) {
            Poll::Ready(output) => Poll::Ready(Ok(output)),
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use fuchsia_async as fasync;

    #[test]
    fn abort_a_future_pending() {
        let mut ex = fasync::TestExecutor::new();
        let forever = std::future::pending::<()>();
        let (scope, handle) = AbortableScope::new();
        let fut = scope.run(forever);
        let mut fut = std::pin::pin!(fut);

        assert!(ex.run_until_stalled(&mut fut).is_pending());
        handle.abort();
        assert_eq!(ex.run_until_stalled(&mut fut), Poll::Ready(Err(AbortError)));
    }

    #[test]
    fn abort_a_future_ready() {
        let mut ex = fasync::TestExecutor::new();
        let now = std::future::ready(());
        let (scope, handle) = AbortableScope::new();
        let fut = scope.run(now);
        let mut fut = std::pin::pin!(fut);

        handle.abort();
        assert_eq!(ex.run_until_stalled(&mut fut), Poll::Ready(Err(AbortError)));
    }

    #[test]
    fn abort_many_futures() {
        let mut ex = fasync::TestExecutor::new();
        let (scope, handle) = AbortableScope::new();

        let now = std::future::ready(());
        let fut1 = scope.run(now);
        let mut fut1 = std::pin::pin!(fut1);

        let now = std::future::ready(());
        let fut2 = scope.run(now);
        let mut fut2 = std::pin::pin!(fut2);

        handle.abort();

        assert_eq!(ex.run_until_stalled(&mut fut1), Poll::Ready(Err(AbortError)));
        assert_eq!(ex.run_until_stalled(&mut fut2), Poll::Ready(Err(AbortError)));

        let now = std::future::ready(());
        let fut3 = scope.run(now);
        let mut fut3 = std::pin::pin!(fut3);
        assert_eq!(ex.run_until_stalled(&mut fut3), Poll::Ready(Err(AbortError)));
    }

    #[test]
    fn abort_a_future_handle_dropped() {
        let mut ex = fasync::TestExecutor::new();
        let forever = std::future::pending::<()>();
        let (scope, handle) = AbortableScope::new();
        let fut = scope.run(forever);
        let mut fut = std::pin::pin!(fut);

        assert!(ex.run_until_stalled(&mut fut).is_pending());
        drop(handle);
        assert!(ex.run_until_stalled(&mut fut).is_pending());
        assert!(ex.run_until_stalled(&mut fut).is_pending());
    }

    #[test]
    fn not_aborting_a_future() {
        let mut ex = fasync::TestExecutor::new();
        let now = std::future::ready(());
        let (scope, _handle) = AbortableScope::new();
        let fut = scope.run(now);
        let mut fut = std::pin::pin!(fut);

        assert_eq!(ex.run_until_stalled(&mut fut), Poll::Ready(Ok(())));
    }
}
