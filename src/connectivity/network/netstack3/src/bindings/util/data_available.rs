// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A multiple-producer, single-consumer notification mechanism.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::Poll;

use futures::Future;
use futures::task::AtomicWaker;

#[derive(Debug, Default)]
struct DataAvailable {
    available: AtomicBool,
    waker: AtomicWaker,
}

/// The notifier side of the underlying data availability signal.
///
/// Notifiers can be cloned to allow for multiple current producers.
#[derive(Debug, Clone)]
pub struct DataNotifier {
    inner: Arc<DataAvailable>,
}

impl DataNotifier {
    /// Notifies the watcher that data is available.
    ///
    /// If the watcher is not currently waiting, the notification will have no
    /// effect until the watcher starts waiting. Multiple notifications are
    /// coalesced.
    pub fn notify(&self) {
        let DataAvailable { available, waker } = &*self.inner;

        let prev = available.swap(true, Ordering::Relaxed);
        if !prev {
            waker.wake();
        }
    }
}

/// The receiver side of the underlying data availability signal.
///
/// The watcher is used to wait for notifications from one or more
/// [`DataNotifier`]s.
#[derive(Debug)]
pub struct DataWatcher {
    inner: Arc<DataAvailable>,
}

impl DataWatcher {
    /// Creates a new watcher and notifier pair.
    pub fn new() -> (Self, DataNotifier) {
        let watcher = DataWatcher { inner: Arc::new(DataAvailable::default()) };
        let notifier = DataNotifier { inner: Arc::clone(&watcher.inner) };
        (watcher, notifier)
    }

    /// Resets the data availability state and returns a future that completes when
    /// a new notification is received.
    ///
    /// To be clear, this method clears any previous notification, and the returned
    /// future will only complete the *next* time [`DataNotifier::notify`] is
    /// called.
    pub fn reset_and_wait(&mut self) -> impl Future<Output = ()> + use<'_> {
        let DataAvailable { available, waker } = &*self.inner;

        available.store(false, Ordering::Relaxed);

        futures::future::poll_fn(|cx| {
            if available.load(Ordering::Relaxed) {
                return Poll::Ready(());
            }

            waker.register(cx.waker());

            // Check again after registering the waker to avoid a race where a notifier
            // flipped the flag after we checked it but before we registered to be woken up,
            // which would result in a lost notification.
            if available.load(Ordering::Relaxed) {
                return Poll::Ready(());
            }
            Poll::Pending
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test() {
        let mut exec = fuchsia_async::TestExecutor::new();

        let (mut watcher, tcp) = DataWatcher::new();
        let udp = tcp.clone();

        // If we notify before the watcher wait has been initialized, the watcher is not
        // notified.
        tcp.notify();
        let mut fut = watcher.reset_and_wait();
        assert_eq!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // If we notify after the watcher wait has been initialized, it should wake the
        // watcher future.
        tcp.notify();
        assert_eq!(exec.run_until_stalled(&mut fut), Poll::Ready(()));
        drop(fut);

        // If we notify again before a new watcher wait is initialized, again, the
        // notification is swallowed.
        tcp.notify();
        let mut fut = watcher.reset_and_wait();
        assert_eq!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // We can notify arbitrarily many times on the same notifier or on arbitrarily
        // many notifiers attached to the same watcher, and regardless it should result
        // in the future waking.
        tcp.notify();
        udp.notify();
        tcp.notify();
        assert_eq!(exec.run_until_stalled(&mut fut), Poll::Ready(()));
        drop(fut);

        // But the notifications are coalesced, so a subsequent wait should not
        // complete.
        let mut fut = watcher.reset_and_wait();
        assert_eq!(exec.run_until_stalled(&mut fut), Poll::Pending);
    }
}
