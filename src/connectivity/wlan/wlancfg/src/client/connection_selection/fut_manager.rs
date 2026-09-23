// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::client::connection_selection::ConnectionSelectionRequester;
use crate::client::types;
use crate::mode_management::iface_manager_api::ConnectAttemptRequest;
use anyhow::format_err;
use futures::channel::oneshot;
use futures::future::{FusedFuture, Future, FutureExt, LocalBoxFuture};
use futures::stream::{FuturesUnordered, Stream};
use log::warn;
use std::collections::HashMap;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

/// Manages the active connection selection futures, allowing for selective cancellation.
pub struct ConnectionSelectionManager {
    /// Collection of running connection selection futures, to be polled.
    #[allow(clippy::type_complexity)]
    futures: FuturesUnordered<
        LocalBoxFuture<
            'static,
            (SelectionIdentifier, Result<Option<ConnectionSelectionResponse>, anyhow::Error>),
        >,
    >,
    /// Maps a SelectionIdentifier to a sender that can cancel the corresponding future.
    cancellation_handles: HashMap<SelectionIdentifier, oneshot::Sender<()>>,
    requester: ConnectionSelectionRequester,
    waker: Option<Waker>,
}

impl ConnectionSelectionManager {
    pub fn new(requester: ConnectionSelectionRequester) -> Self {
        Self {
            futures: FuturesUnordered::new(),
            cancellation_handles: HashMap::new(),
            requester,
            waker: None,
        }
    }

    /// Spawns a new connection selection future with an associated SelectionIdentifier.
    /// Wraps the future in a cancellation logic.
    pub fn spawn(
        &mut self,
        selection_id: SelectionIdentifier,
        selection_future: LocalBoxFuture<
            'static,
            Result<ConnectionSelectionResponse, anyhow::Error>,
        >,
    ) {
        if self.cancellation_handles.contains_key(&selection_id) {
            warn!("{} already in progress. Ignoring new request.", selection_id);
            return;
        }

        let (sender, receiver) = oneshot::channel();
        let _ = self.cancellation_handles.insert(selection_id.clone(), sender);
        let cancellable_future = async move {
            futures::select! {
                result = selection_future.fuse() => (selection_id.clone(), result.map(Some)),
                _ = receiver.fuse() => (selection_id, Ok(None)),
            }
        };

        self.futures.push(cancellable_future.boxed_local());
        // If the manager was previously polled and returned Poll::Pending, it would have stored
        // a waker, provided by the poller. That waker needs to be awakened to inform the poller
        // that the manager now has more work to do.
        if let Some(waker) = &self.waker {
            waker.wake_by_ref();
        }
    }

    /// Cancels the future associated with the given selection_id.
    pub fn cancel(&mut self, selection_id: &SelectionIdentifier) {
        if let Some(cancel_tx) = self.cancellation_handles.remove(selection_id) {
            let _ = cancel_tx.send(());
        }
    }

    /// Cancels all active connection selections.
    pub fn cancel_all(&mut self) {
        for selection_id in self.active_selections() {
            self.cancel(&selection_id);
        }
    }

    /// Returns a list of SelectionIdentifiers for connection selections currently being processed.
    pub fn active_selections(&self) -> Vec<SelectionIdentifier> {
        self.cancellation_handles.keys().cloned().collect()
    }

    pub fn initiate_automatic_connection_selection(&mut self) {
        let mut requester = self.requester.clone();
        let fut = async move {
            requester
                .do_connection_selection(None, types::ConnectReason::IdleInterfaceAutoconnect)
                .await
                .map_err(|e| format_err!("Error sending connection selection request: {}.", e))
                .map(ConnectionSelectionResponse::Autoconnect)
        };
        self.spawn(SelectionIdentifier::Automatic, Box::pin(fut).boxed_local());
    }

    pub fn initiate_connection_selection_for_connect_request(
        &mut self,
        request: ConnectAttemptRequest,
    ) {
        let mut requester = self.requester.clone();
        let network = request.network.clone();
        let fut = async move {
            requester
                .do_connection_selection(Some(request.network.clone()), request.reason)
                .await
                .map_err(|e| format_err!("Error sending connection selection request: {}.", e))
                .map(move |candidate| ConnectionSelectionResponse::ConnectRequest {
                    candidate,
                    request,
                })
        };
        self.spawn(SelectionIdentifier::ConnectRequest(network), Box::pin(fut).boxed_local());
    }

    #[cfg(test)]
    #[allow(clippy::type_complexity)]
    pub fn get_futures(
        &mut self,
    ) -> &mut FuturesUnordered<
        LocalBoxFuture<
            'static,
            (SelectionIdentifier, Result<Option<ConnectionSelectionResponse>, anyhow::Error>),
        >,
    > {
        &mut self.futures
    }

    #[cfg(test)]
    pub fn get_cancellation_handles(
        &mut self,
    ) -> &mut HashMap<SelectionIdentifier, oneshot::Sender<()>> {
        &mut self.cancellation_handles
    }
}

/// A wrapper future to safely poll the ConnectionSelectionManager within a select! macro.
pub struct ConnectionSelectionFutures<'a> {
    manager: &'a mut ConnectionSelectionManager,
}
impl<'a> ConnectionSelectionFutures<'a> {
    pub fn new(manager: &'a mut ConnectionSelectionManager) -> Self {
        Self { manager }
    }
}
impl<'a> Future for ConnectionSelectionFutures<'a> {
    type Output = (SelectionIdentifier, Result<Option<ConnectionSelectionResponse>, anyhow::Error>);

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Clear any stored waker, in case its obsolete.
        self.manager.waker = None;

        match Pin::new(&mut self.manager.futures).poll_next(cx) {
            Poll::Ready(Some((selection_id, result))) => {
                // Clean up the cancellation map when a future completes.
                let _ = self.manager.cancellation_handles.remove(&selection_id);
                Poll::Ready((selection_id, result))
            }
            Poll::Ready(None) | Poll::Pending => {
                // According to the docs, a FuturesUnordered must be polled at least once after a
                // new future is added in order for that new future to get advanced. We want to
                // achieve that by having the external caller poll ConnectionSelectionFutures, which
                // will cause the internal FuturesUnordered to be polled. Therefore, this stores a
                // copy of the external caller's waker, and use it to wake up that caller when a
                // new future is spawned. Otherwise, there is no guarantee that the new futures get
                // polled.
                self.manager.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}
impl<'a> FusedFuture for ConnectionSelectionFutures<'a> {
    fn is_terminated(&self) -> bool {
        // This tells select! that futures will continue to be added to the manager, so it should
        // never be considered terminated.
        false
    }
}

// Identifier for cancellable connection selection futures.
#[derive(Clone, PartialEq, Eq, Hash)]
#[cfg_attr(test, derive(Debug))]
pub enum SelectionIdentifier {
    ConnectRequest(types::NetworkIdentifier),
    Automatic,
}
impl std::fmt::Display for SelectionIdentifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SelectionIdentifier::ConnectRequest(id) => {
                write!(f, "Connection selection for network: {}", id)
            }
            SelectionIdentifier::Automatic => write!(f, "Automatic connection selection"),
        }
    }
}

#[cfg_attr(test, derive(Debug))]
pub enum ConnectionSelectionResponse {
    ConnectRequest { candidate: Option<types::ScannedCandidate>, request: ConnectAttemptRequest },
    Autoconnect(Option<types::ScannedCandidate>),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::testing::generate_random_network_identifier;
    use assert_matches::assert_matches;
    use fuchsia_async::TestExecutor;
    use futures::task::Poll;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::task::{Wake, Waker};

    fn fake_requester() -> ConnectionSelectionRequester {
        let (sender, _receiver) = futures::channel::mpsc::channel(1);
        ConnectionSelectionRequester::new(sender)
    }

    #[test]
    fn test_spawn_and_poll() {
        let mut exec = TestExecutor::new();
        let mut manager = ConnectionSelectionManager::new(fake_requester());

        let selection_id = SelectionIdentifier::Automatic;
        let fut = async move { Ok(ConnectionSelectionResponse::Autoconnect(None)) };
        manager.spawn(selection_id.clone(), Box::pin(fut));

        let mut futures = ConnectionSelectionFutures::new(&mut manager);
        assert_matches!(
            exec.run_until_stalled(&mut futures),
            Poll::Ready((id, Ok(Some(ConnectionSelectionResponse::Autoconnect(None))))) if id == selection_id
        );
    }

    #[test]
    fn test_spawn_duplicate() {
        let mut manager = ConnectionSelectionManager::new(fake_requester());

        let selection_id = SelectionIdentifier::Automatic;

        // Spawn first
        manager.spawn(
            selection_id.clone(),
            Box::pin(async { Ok(ConnectionSelectionResponse::Autoconnect(None)) }),
        );
        assert_eq!(manager.active_selections().len(), 1);

        // Spawn second (duplicate)
        manager.spawn(
            selection_id.clone(),
            Box::pin(async { Ok(ConnectionSelectionResponse::Autoconnect(None)) }),
        );

        // Should still be 1 because duplicate was ignored
        assert_eq!(manager.active_selections().len(), 1);
    }

    #[test]
    fn test_cancel_specific() {
        let mut exec = TestExecutor::new();
        let mut manager = ConnectionSelectionManager::new(fake_requester());

        let id_auto = SelectionIdentifier::Automatic;
        let id_req = SelectionIdentifier::ConnectRequest(types::NetworkIdentifier {
            ssid: types::Ssid::from_bytes_unchecked(b"test".to_vec()),
            security_type: types::SecurityType::Wpa2,
        });

        // Spawn two futures that pending forever
        manager.spawn(id_auto.clone(), Box::pin(futures::future::pending()));
        manager.spawn(id_req.clone(), Box::pin(futures::future::pending()));

        assert_eq!(manager.active_selections().len(), 2);

        // Cancel one
        manager.cancel(&id_auto);

        // Polling should return cancelled result for id_auto
        let mut futures = ConnectionSelectionFutures::new(&mut manager);
        assert_matches!(
            exec.run_until_stalled(&mut futures),
            Poll::Ready((id, Ok(None))) if id == id_auto
        );

        // Check active selections - the cancelled handle should be removed immediately.
        assert_eq!(manager.active_selections().len(), 1);
        assert!(manager.active_selections().contains(&id_req));
    }

    #[test]
    fn test_cancel_all() {
        let mut exec = TestExecutor::new();
        let mut manager = ConnectionSelectionManager::new(fake_requester());

        let id1 = SelectionIdentifier::Automatic;
        let id2 = SelectionIdentifier::ConnectRequest(types::NetworkIdentifier {
            ssid: types::Ssid::from_bytes_unchecked(b"test".to_vec()),
            security_type: types::SecurityType::Wpa2,
        });

        manager.spawn(id1.clone(), Box::pin(futures::future::pending()));
        manager.spawn(id2.clone(), Box::pin(futures::future::pending()));

        assert_eq!(manager.active_selections().len(), 2);

        manager.cancel_all();

        assert_eq!(manager.active_selections().len(), 0);

        // Both should return cancelled
        let mut futures = ConnectionSelectionFutures::new(&mut manager);

        // We expect two ready results of None
        let mut cancelled_count = 0;

        loop {
            match exec.run_until_stalled(&mut futures) {
                Poll::Ready((_, Ok(None))) => cancelled_count += 1,
                Poll::Pending => break,
                _ => panic!("Unexpected result"),
            }
        }

        assert_eq!(cancelled_count, 2);
    }

    #[test]
    fn test_internal_state_access() {
        let mut manager = ConnectionSelectionManager::new(fake_requester());
        let id = SelectionIdentifier::Automatic;
        manager.spawn(id.clone(), Box::pin(futures::future::pending()));

        assert_eq!(manager.get_cancellation_handles().len(), 1);
        assert!(!manager.get_futures().is_empty());
    }

    #[test]
    fn test_poll_empty_twice_does_not_panic() {
        let mut exec = TestExecutor::new();
        let mut manager = ConnectionSelectionManager::new(fake_requester());

        let mut futures = ConnectionSelectionFutures::new(&mut manager);
        assert_matches!(exec.run_until_stalled(&mut futures), Poll::Pending);
        assert_matches!(exec.run_until_stalled(&mut futures), Poll::Pending);
    }

    #[test]
    fn test_spawn_wakes_after_empty() {
        struct FlagWaker(AtomicBool);
        impl Wake for FlagWaker {
            fn wake(self: Arc<Self>) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let mut manager = ConnectionSelectionManager::new(fake_requester());

        let flag = Arc::new(FlagWaker(AtomicBool::new(false)));
        let waker = Waker::from(flag.clone());
        let mut cx = Context::from_waker(&waker);

        // Poll empty futures
        {
            let mut futures = ConnectionSelectionFutures::new(&mut manager);
            assert_matches!(Pin::new(&mut futures).poll(&mut cx), Poll::Pending);
        }

        // Verify the wakers flag is still false.
        assert!(!flag.0.load(Ordering::SeqCst));

        // Spawn a new future
        let selection_id = SelectionIdentifier::Automatic;
        let fut = async move { Ok(ConnectionSelectionResponse::Autoconnect(None)) };
        manager.spawn(selection_id, Box::pin(fut));

        // Verify waker was triggered
        assert!(
            flag.0.load(Ordering::SeqCst),
            "Waker should have been awakened by spawn() after empty poll"
        );
    }

    #[test]
    fn test_spawn_wakes_while_busy() {
        // Create a waker that increments a counter when awakened.
        struct CounterWaker(AtomicUsize);
        impl Wake for CounterWaker {
            fn wake(self: Arc<Self>) {
                let _ = self.0.fetch_add(1, Ordering::SeqCst);
            }
        }

        let mut manager = ConnectionSelectionManager::new(fake_requester());

        let flag = Arc::new(CounterWaker(AtomicUsize::new(0)));
        let waker = Waker::from(flag.clone());
        let mut cx = Context::from_waker(&waker);

        // Spawn a future that never completes. This represents an ongoing connection selection future.
        manager.spawn(SelectionIdentifier::Automatic, Box::pin(futures::future::pending()));

        {
            let mut futures = ConnectionSelectionFutures::new(&mut manager);
            assert_matches!(Pin::new(&mut futures).poll(&mut cx), Poll::Pending);
        }
        assert_eq!(flag.0.load(Ordering::SeqCst), 1);

        // Spawn a new future.
        let selection_id =
            SelectionIdentifier::ConnectRequest(generate_random_network_identifier());
        manager.spawn(
            selection_id.clone(),
            Box::pin(futures::future::ready(Ok(ConnectionSelectionResponse::Autoconnect(None)))),
        );
        assert_eq!(
            flag.0.load(Ordering::SeqCst),
            2,
            "Manager should wake stored waker on spawn even when busy"
        );
    }
}
