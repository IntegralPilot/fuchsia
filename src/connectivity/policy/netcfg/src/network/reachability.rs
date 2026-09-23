// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::pin::Pin;
use std::task::{Context, Poll};

use fidl::endpoints::{ControlHandle as _, RequestStream as _};
use fidl_fuchsia_net_policy_socketproxy as fnp_socketproxy;
use fidl_fuchsia_net_reachability as freachability;
use futures::Stream;
use log::{error, warn};

use super::NetworkProperties;

pub(crate) const MAX_REACHABILITY_WATCHERS: usize = 128;

/// Seals [`ReachabilityWatcherConnectionId`]: its private field makes [`IdAllocator::allocate`]
/// the only way to construct one. Ids are unique per allocator.
mod id {
    /// Identifies a reachability watcher connection.
    #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
    pub struct ReachabilityWatcherConnectionId(usize);

    /// Mints unique [`ReachabilityWatcherConnectionId`]s.
    #[derive(Default)]
    pub struct IdAllocator(usize);

    impl IdAllocator {
        pub fn allocate(&mut self) -> ReachabilityWatcherConnectionId {
            let id = ReachabilityWatcherConnectionId(self.0);
            self.0 += 1;
            id
        }
    }
}

use id::IdAllocator;
pub use id::ReachabilityWatcherConnectionId;

struct ReachabilityWatcherClient {
    /// Used to close the connection; see [`ReachabilityWatcherClient::close`].
    control_handle: freachability::MonitorControlHandle,
    /// The most recent snapshot observed by this client.
    last_observed: Option<freachability::Snapshot>,
    /// The responder for a hanging `Watch` call, if one is outstanding.
    responder: Option<freachability::MonitorWatchResponder>,
    /// Whether the client may still call `SetOptions`.
    ///
    /// `SetOptions` may only be called once, and only before the first
    /// call to `Watch`.
    can_set_options: bool,
}

impl ReachabilityWatcherClient {
    /// Closes the client's connection, with `epitaph` if one is provided.
    ///
    /// The watcher is left in place: shutting the connection down guarantees its
    /// [`ReachabilityStream`] yields the terminal item that the watcher is removed on.
    fn close(&mut self, epitaph: Option<zx::Status>) {
        // Shut down before releasing the responder below to communicate an epitaph
        // when one is provided.
        match epitaph {
            Some(status) => self.control_handle.shutdown_with_epitaph(status),
            None => self.control_handle.shutdown(),
        }
        // Drop the hanging responder, if any, so a closed client is no longer served.
        self.responder = None;
    }
}

pub(crate) struct ReachabilityStream {
    id: ReachabilityWatcherConnectionId,
    /// The client's request stream; `None` once the stream has terminated.
    stream: Option<freachability::MonitorRequestStream>,
}

impl Stream for ReachabilityStream {
    /// Yields `(id, Some(request))` per request from the client, then exactly one `(id, None)`
    /// when the client's stream terminates, which is how the end of a connection is observed.
    type Item = (
        ReachabilityWatcherConnectionId,
        Option<Result<freachability::MonitorRequest, fidl::Error>>,
    );

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = Pin::into_inner(self);
        let id = this.id;
        let poll = match this.stream.as_mut() {
            Some(stream) => Pin::new(stream).poll_next(cx),
            None => return Poll::Ready(None),
        };
        match poll {
            Poll::Ready(Some(request)) => Poll::Ready(Some((id, Some(request)))),
            Poll::Ready(None) => {
                this.stream = None;
                Poll::Ready(Some((id, None)))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl futures::stream::FusedStream for ReachabilityStream {
    fn is_terminated(&self) -> bool {
        self.stream.is_none()
    }
}

#[derive(Default)]
pub(crate) struct ReachabilityHandler {
    watchers: HashMap<ReachabilityWatcherConnectionId, ReachabilityWatcherClient>,
    next_id: IdAllocator,
}

impl ReachabilityHandler {
    pub(crate) fn add_stream(
        &mut self,
        stream: freachability::MonitorRequestStream,
    ) -> Option<ReachabilityStream> {
        if self.watchers.len() >= MAX_REACHABILITY_WATCHERS {
            warn!(
                "Max reachability watchers ({MAX_REACHABILITY_WATCHERS}) reached; rejecting stream."
            );
            stream.control_handle().shutdown_with_epitaph(zx::Status::NO_RESOURCES);
            return None;
        }
        let id = self.next_id.allocate();
        let previous = self.watchers.insert(
            id,
            ReachabilityWatcherClient {
                control_handle: stream.control_handle(),
                last_observed: None,
                responder: None,
                can_set_options: true,
            },
        );
        assert!(previous.is_none(), "reachability watcher {id:?} is already registered");
        Some(ReachabilityStream { id, stream: Some(stream) })
    }

    /// Synthesizes a reachability [`freachability::Snapshot`] from the active default network.
    ///
    /// On multi-network systems, the snapshot reflects the reachability and validation state of
    /// the system's active default network, through which ambient, unbound network traffic is
    /// routed.
    pub(crate) fn synthesize_snapshot(
        default_network: Option<&NetworkProperties>,
    ) -> freachability::Snapshot {
        // `ConnectivityState` does not report link-layer reachability directly, so
        // `Monitor` states are inferred.
        let (gateway_reachable, internet_available, dns_active, http_active) = match default_network
            .and_then(|properties| properties.connectivity_state)
        {
            // Internet reachability has been verified end to end, including DNS resolution
            // and HTTP/HTTPS fetches.
            Some(fnp_socketproxy::ConnectivityState::FullConnectivity) => (true, true, true, true),
            // The Internet is partially reachable. DNS resolution and at least one HTTP
            // probe succeeded, but HTTPS probes were unsuccessful. We conservatively report this
            // as a state with `http_active=false` since this probe did not provide strong evidence
            // that all HTTP requests will be servicable.
            Some(fnp_socketproxy::ConnectivityState::PartialConnectivity) => {
                (true, true, true, false)
            }
            // The Internet is not reachable and we have not verified upper layer connectivity.
            // LocalConnectivity implies that devices can be reached on the local network, but does
            // not confirm that there is a gateway present and accessible.
            Some(
                fnp_socketproxy::ConnectivityState::LocalConnectivity
                | fnp_socketproxy::ConnectivityState::NoConnectivity,
            )
            | None => (false, false, false, false),
            Some(fnp_socketproxy::ConnectivityState::__SourceBreaking { unknown_ordinal }) => {
                unreachable!(
                    "New variants of ConnectivityState must be updated: {unknown_ordinal:?}"
                )
            }
        };
        freachability::Snapshot {
            gateway_reachable: Some(gateway_reachable),
            dns_active: Some(dns_active),
            internet_available: Some(internet_available),
            http_active: Some(http_active),
            ..Default::default()
        }
    }

    /// Handles a single item produced by a client's [`ReachabilityStream`].
    pub(crate) fn handle_request(
        &mut self,
        current_snapshot: &freachability::Snapshot,
        id: ReachabilityWatcherConnectionId,
        request: Option<Result<freachability::MonitorRequest, fidl::Error>>,
    ) {
        let Entry::Occupied(mut entry) = self.watchers.entry(id) else {
            // A watcher is removed only when its stream yields its terminal item, and the stream
            // is Fused, so every request belongs to a live watcher.
            unreachable!("request for unknown reachability watcher {id:?}");
        };

        let request = match request {
            Some(Ok(request)) => request,
            Some(Err(e)) => {
                // A clean disconnect yields the terminal item below rather than an error, so
                // this is always abnormal: a malformed request or a channel-level read
                // failure. No epitaph is sent because the channel may no longer be writable.
                error!("Reachability monitor client {id:?} stream error: {e}");
                entry.get_mut().close(None);
                return;
            }
            // The client's stream has terminated, either because the client went away or because
            // of a `close` above. This is the only place watchers are removed.
            None => {
                let _: ReachabilityWatcherClient = entry.remove();
                return;
            }
        };

        let client = entry.get_mut();
        match request {
            freachability::MonitorRequest::SetOptions {
                payload: freachability::MonitorOptions { __source_breaking },
                control_handle: _,
            } => {
                if !client.can_set_options {
                    warn!(
                        "Client {id:?} called SetOptions after SetOptions or Watch; \
                        closing channel."
                    );
                    client.close(Some(zx::Status::CONNECTION_ABORTED));
                } else {
                    client.can_set_options = false;
                }
            }
            freachability::MonitorRequest::Watch { responder } => {
                if client.responder.is_some() {
                    warn!(
                        "Client {id:?} called Watch while a previous Watch was pending; \
                        closing channel."
                    );
                    client.close(Some(zx::Status::ALREADY_EXISTS));
                    return;
                }
                client.can_set_options = false;
                if client.last_observed.as_ref() != Some(current_snapshot) {
                    client.last_observed = Some(current_snapshot.clone());
                    if let Err(e) = responder.send(current_snapshot) {
                        warn!("failed to send reachability snapshot to client {id:?}: {e}");
                    }
                } else {
                    client.responder = Some(responder);
                }
            }
        }
    }

    pub(crate) fn maybe_notify_watchers(&mut self, current_snapshot: &freachability::Snapshot) {
        for (id, client) in self.watchers.iter_mut() {
            if client.last_observed.as_ref() != Some(current_snapshot) {
                if let Some(responder) = client.responder.take() {
                    client.last_observed = Some(current_snapshot.clone());
                    if let Err(e) = responder.send(current_snapshot) {
                        warn!("failed to send updated reachability snapshot to client {id:?}: {e}");
                    }
                }
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn watcher_count(&self) -> usize {
        self.watchers.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use futures::StreamExt as _;
    use futures::stream::FusedStream as _;

    /// The snapshot synthesized when there is no default network, or when it reports
    /// `NoConnectivity` or `LocalConnectivity`.
    fn disconnected_snapshot() -> freachability::Snapshot {
        freachability::Snapshot {
            gateway_reachable: Some(false),
            dns_active: Some(false),
            internet_available: Some(false),
            http_active: Some(false),
            ..Default::default()
        }
    }

    /// The snapshot synthesized from a default network reporting `PartialConnectivity`.
    fn limited_snapshot() -> freachability::Snapshot {
        freachability::Snapshot {
            gateway_reachable: Some(true),
            dns_active: Some(true),
            internet_available: Some(true),
            http_active: Some(false),
            ..Default::default()
        }
    }

    /// The snapshot synthesized from a default network reporting `FullConnectivity`.
    fn validated_snapshot() -> freachability::Snapshot {
        freachability::Snapshot {
            gateway_reachable: Some(true),
            dns_active: Some(true),
            internet_available: Some(true),
            http_active: Some(true),
            ..Default::default()
        }
    }

    /// Dispatches every remaining item produced by `stream`, including its terminal item.
    ///
    /// Watchers are removed when their stream terminates, so a client that has been closed is
    /// only reaped once its stream has been drained.
    async fn drain_stream(
        handler: &mut ReachabilityHandler,
        current_snapshot: &freachability::Snapshot,
        stream: &mut ReachabilityStream,
    ) {
        while let Some((id, request)) = stream.next().await {
            handler.handle_request(current_snapshot, id, request);
        }
    }

    #[test]
    fn test_synthesize_snapshot() {
        // Confirm that no network produces a fully disconnected snapshot.
        assert_eq!(ReachabilityHandler::synthesize_snapshot(None), disconnected_snapshot());

        // Confirm that a `NoConnectivity` connectivity state produces a disconnected snapshot.
        let net_none = NetworkProperties {
            connectivity_state: Some(fnp_socketproxy::ConnectivityState::NoConnectivity),
            ..Default::default()
        };
        assert_eq!(
            ReachabilityHandler::synthesize_snapshot(Some(&net_none)),
            disconnected_snapshot()
        );

        // Confirm that a `LocalConnectivity` connectivity state produces a disconnected snapshot.
        let net_local = NetworkProperties {
            connectivity_state: Some(fnp_socketproxy::ConnectivityState::LocalConnectivity),
            ..Default::default()
        };
        assert_eq!(
            ReachabilityHandler::synthesize_snapshot(Some(&net_local)),
            disconnected_snapshot()
        );

        // Confirm that a `PartialConnectivity` connectivity state reports DNS as active but
        // HTTP as inactive.
        let net_limited = NetworkProperties {
            connectivity_state: Some(fnp_socketproxy::ConnectivityState::PartialConnectivity),
            ..Default::default()
        };
        assert_eq!(
            ReachabilityHandler::synthesize_snapshot(Some(&net_limited)),
            limited_snapshot()
        );

        // Confirm that a `FullConnectivity` connectivity state produces a validated snapshot.
        let net_full = NetworkProperties {
            connectivity_state: Some(fnp_socketproxy::ConnectivityState::FullConnectivity),
            ..Default::default()
        };
        assert_eq!(ReachabilityHandler::synthesize_snapshot(Some(&net_full)), validated_snapshot());
    }

    #[fuchsia::test]
    async fn test_reachability_handler_hanging_get() {
        let mut handler = ReachabilityHandler::default();
        let disconnected = disconnected_snapshot();
        let validated = validated_snapshot();

        let (proxy1, stream1) =
            fidl::endpoints::create_proxy_and_stream::<freachability::MonitorMarker>();
        let mut s1 = handler.add_stream(stream1).expect("add stream");
        assert_eq!(handler.watcher_count(), 1);

        // Client 1 initial Watch() returns immediately with current snapshot.
        let watch_fut1 = proxy1.watch();
        let (id, req) = s1.next().await.expect("stream item");
        handler.handle_request(&disconnected, id, req);
        let snapshot = watch_fut1.await.expect("watch error");
        assert_eq!(snapshot, disconnected);

        // Client 2 connects
        let (proxy2, stream2) =
            fidl::endpoints::create_proxy_and_stream::<freachability::MonitorMarker>();
        let mut s2 = handler.add_stream(stream2).expect("add stream");
        assert_eq!(handler.watcher_count(), 2);

        // Client 2 initial Watch() returns immediately with current snapshot.
        let watch_fut2 = proxy2.watch();
        let (id, req) = s2.next().await.expect("stream item");
        handler.handle_request(&disconnected, id, req);
        let snapshot2 = watch_fut2.await.expect("watch error");
        assert_eq!(snapshot2, disconnected);

        // Both clients call Watch() again: both will hang because snapshot hasn't changed.
        let mut second_watch1 = proxy1.watch();
        let (id, req) = s1.next().await.expect("stream item");
        handler.handle_request(&disconnected, id, req);
        assert_matches!(futures::poll!(&mut second_watch1), std::task::Poll::Pending);

        let mut second_watch2 = proxy2.watch();
        let (id, req) = s2.next().await.expect("stream item");
        handler.handle_request(&disconnected, id, req);
        assert_matches!(futures::poll!(&mut second_watch2), std::task::Poll::Pending);

        // State changes to validated: both watchers unblock.
        handler.maybe_notify_watchers(&validated);
        let snap1 = second_watch1.await.expect("watch1 should succeed");
        let snap2 = second_watch2.await.expect("watch2 should succeed");
        assert_eq!(snap1, validated);
        assert_eq!(snap2, validated);

        // Client 2 disconnects, and is reaped once its stream terminates.
        drop(proxy2);
        drain_stream(&mut handler, &validated, &mut s2).await;
        assert!(s2.is_terminated());
        assert_eq!(handler.watcher_count(), 1);
    }

    #[fuchsia::test]
    async fn test_set_options_validation() {
        let mut handler = ReachabilityHandler::default();
        let disconnected = disconnected_snapshot();
        let (proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<freachability::MonitorMarker>();
        let mut s = handler.add_stream(stream).expect("add stream");

        // Calling SetOptions as the first call should succeed.
        proxy.set_options(&freachability::MonitorOptions::default()).expect("set_options FIDL");
        let (id, req) = s.next().await.expect("stream item");
        handler.handle_request(&disconnected, id, req);
        assert_eq!(handler.watcher_count(), 1);

        // Calling SetOptions a second time aborts connection.
        proxy.set_options(&freachability::MonitorOptions::default()).expect("set_options FIDL");
        let (id, req) = s.next().await.expect("stream item");
        handler.handle_request(&disconnected, id, req);

        assert_matches!(
            proxy.watch().await,
            Err(fidl::Error::ClientChannelClosed { epitaph, .. })
                if epitaph == zx::Status::CONNECTION_ABORTED
        );

        // The watcher is reaped once its now-shut-down stream terminates.
        drain_stream(&mut handler, &disconnected, &mut s).await;
        assert!(s.is_terminated());
        assert_eq!(handler.watcher_count(), 0);

        // Calling SetOptions after calling Watch aborts connection.
        let (proxy2, stream2) =
            fidl::endpoints::create_proxy_and_stream::<freachability::MonitorMarker>();
        let mut s2 = handler.add_stream(stream2).expect("add stream");

        let watch_fut = proxy2.watch();
        let (id, req) = s2.next().await.expect("stream item");
        handler.handle_request(&disconnected, id, req);
        let _ = watch_fut.await.expect("initial watch");
        assert_eq!(handler.watcher_count(), 1);

        proxy2.set_options(&freachability::MonitorOptions::default()).expect("set_options FIDL");
        let (id, req) = s2.next().await.expect("stream item");
        handler.handle_request(&disconnected, id, req);

        assert_matches!(
            proxy2.watch().await,
            Err(fidl::Error::ClientChannelClosed { epitaph, .. })
                if epitaph == zx::Status::CONNECTION_ABORTED
        );

        drain_stream(&mut handler, &disconnected, &mut s2).await;
        assert!(s2.is_terminated());
        assert_eq!(handler.watcher_count(), 0);
    }

    #[fuchsia::test]
    async fn test_concurrent_watch_closes_channel() {
        let mut handler = ReachabilityHandler::default();
        let disconnected = disconnected_snapshot();
        let (proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<freachability::MonitorMarker>();
        let mut s = handler.add_stream(stream).expect("add stream");

        // First watch consumes initial snapshot.
        let watch_fut1 = proxy.watch();
        let (id, req) = s.next().await.expect("stream item");
        handler.handle_request(&disconnected, id, req);
        let _ = watch_fut1.await.expect("initial watch");

        // Second watch hangs.
        let mut second_watch1 = proxy.watch();
        let (id, req) = s.next().await.expect("stream item");
        handler.handle_request(&disconnected, id, req);
        assert_matches!(futures::poll!(&mut second_watch1), std::task::Poll::Pending);

        // Illegal concurrent watch aborts the channel with ALREADY_EXISTS.
        let second_watch2 = proxy.watch();
        let (id, req) = s.next().await.expect("stream item");
        handler.handle_request(&disconnected, id, req);

        assert_matches!(
            second_watch2.await,
            Err(fidl::Error::ClientChannelClosed { epitaph, .. })
                if epitaph == zx::Status::ALREADY_EXISTS
        );

        // Both the pending and the rejected watch are abandoned, and the watcher is reaped once
        // its stream terminates.
        assert_matches!(second_watch1.await, Err(fidl::Error::ClientChannelClosed { .. }));
        drain_stream(&mut handler, &disconnected, &mut s).await;
        assert!(s.is_terminated());
        assert_eq!(handler.watcher_count(), 0);
    }

    #[fuchsia::test]
    async fn test_max_watchers_limit() {
        let mut handler = ReachabilityHandler::default();
        let mut proxies = Vec::new();
        for _ in 0..MAX_REACHABILITY_WATCHERS {
            let (proxy, stream) =
                fidl::endpoints::create_proxy_and_stream::<freachability::MonitorMarker>();
            assert!(handler.add_stream(stream).is_some());
            proxies.push(proxy);
        }
        assert_eq!(handler.watcher_count(), MAX_REACHABILITY_WATCHERS);

        // Next connection exceeds limit and is rejected with NO_RESOURCES.
        let (overflow_proxy, overflow_stream) =
            fidl::endpoints::create_proxy_and_stream::<freachability::MonitorMarker>();
        assert!(handler.add_stream(overflow_stream).is_none());
        assert_matches!(
            overflow_proxy.watch().await,
            Err(fidl::Error::ClientChannelClosed { epitaph, .. })
                if epitaph == zx::Status::NO_RESOURCES
        );
    }
}
