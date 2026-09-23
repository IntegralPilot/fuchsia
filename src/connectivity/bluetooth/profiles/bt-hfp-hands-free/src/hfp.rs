// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result, format_err};
use async_helpers::maybe_stream::MaybeStream;
use async_utils::stream::FutureMap;
use bt_hfp::{audio, sco};
use fidl::endpoints::{ClientEnd, ControlHandle, RequestStream};
use fidl_fuchsia_bluetooth_bredr as bredr;
use fidl_fuchsia_bluetooth_hfp as fidl_hfp;
use fuchsia_async as fasync;
use fuchsia_bluetooth::profile::ProtocolDescriptor;
use fuchsia_bluetooth::types::PeerId;
use fuchsia_sync::Mutex;
use futures::stream::{FusedStream, FuturesUnordered};
use futures::{FutureExt, StreamExt, select};
use log::{debug, info, warn};
use profile_client::{ProfileClient, ProfileEvent};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use zx;

use crate::config::HandsFreeFeatureSupport;
use crate::one_to_one::OneToOneMatcher;
use crate::peer::Peer;

#[cfg(test)]
mod tests;

pub const SEARCH_RESULT_CONNECT_DELAY_SECONDS: i64 = 1;
const SEARCH_RESULT_CONNECT_DELAY_DURATION: fasync::MonotonicDuration =
    fasync::MonotonicDuration::from_seconds(SEARCH_RESULT_CONNECT_DELAY_SECONDS);

type SearchResultTimer = Pin<Box<dyn Future<Output = (PeerId, Option<Vec<ProtocolDescriptor>>)>>>;

type WatchPeerConnectedResponder = fidl_hfp::HandsFreeWatchPeerConnectedResponder;
type WatchPeerConnectedResponse = (PeerId, ClientEnd<fidl_hfp::PeerHandlerMarker>);
type WatchPeerConnectedResult = std::result::Result<(), anyhow::Error>;
type WatchPeerConnectedHangingGetMatcher = OneToOneMatcher<
    WatchPeerConnectedResponder,
    WatchPeerConnectedResponse,
    WatchPeerConnectedResult,
>;

/// Toplevel struct containing the streams of incoming events that are not specific to a single
/// peer.
///
/// The Stream of incoming HandsFree FIDL protocol connections should be instantiated as a
/// ServiceFs for non-test code or with another stream implementation for testing.
pub struct Hfp<S>
where
    S: FusedStream<Item = fidl_hfp::HandsFreeRequestStream> + Unpin + 'static,
{
    /// Configuration for which HF features we support.
    hf_features: HandsFreeFeatureSupport,
    /// Provides Hfp with a means to drive the `fuchsia.bluetooth.bredr` related APIs.
    profile_client: ProfileClient,
    /// The client connection to the `fuchsia.bluetooth.bredr.Profile` protocol.
    profile_proxy: bredr::ProfileProxy,
    /// Timers for asynchronously handling search result profile events.
    search_result_timers: FuturesUnordered<SearchResultTimer>,
    /// A stream of incoming HandsFree FIDL protocol connections. This should be instantiated as a
    /// ServiceFs for live code and as some other stream for testing.
    hands_free_connection_stream: S,
    /// Stream of incoming HandsFree FIDL protocol Requests
    hands_free_request_maybe_stream: MaybeStream<fidl_hfp::HandsFreeRequestStream>,
    // Matcher for matching connecting peers to hanging get responders for the `WatchPeerConnected` FIDL method.
    watch_peer_connected_hanging_get_matcher: WatchPeerConnectedHangingGetMatcher,
    /// A collection of discovered and/or connected Bluetooth peers that support the AG role.
    peers: FutureMap<PeerId, Peer>,
    // Struct for creating SCO connections
    sco_connector: sco::Connector,
    // Audio control for HFP aufio
    audio_control: Arc<Mutex<Box<dyn audio::Control>>>,
}

impl<S> Hfp<S>
where
    S: FusedStream<Item = fidl_hfp::HandsFreeRequestStream> + Unpin + 'static,
{
    pub fn new(
        hf_features: HandsFreeFeatureSupport,
        profile_client: ProfileClient,
        profile_proxy: bredr::ProfileProxy,
        sco_connector: sco::Connector,
        audio_control: Box<dyn audio::Control>,
        hands_free_connection_stream: S,
    ) -> Self {
        let hands_free_connection_stream = hands_free_connection_stream;
        let hands_free_request_maybe_stream = MaybeStream::default();
        let watch_peer_connected_hanging_get_matcher = OneToOneMatcher::new(
            |responder: WatchPeerConnectedResponder, response: WatchPeerConnectedResponse| {
                let peer_id: fidl_fuchsia_bluetooth::PeerId = response.0.into();
                let client_end = response.1;
                responder
                    .send(Ok((&peer_id, client_end)))
                    .context(format!("Sending peer connected result for peer {:}", response.0))
            },
        );
        let peers = FutureMap::new();
        let search_result_timers = FuturesUnordered::new();
        let audio_control = Arc::new(Mutex::new(audio_control));

        Self {
            hf_features,
            profile_client,
            profile_proxy,
            hands_free_connection_stream,
            hands_free_request_maybe_stream,
            watch_peer_connected_hanging_get_matcher,
            peers,
            search_result_timers,
            sco_connector,
            audio_control,
        }
    }

    /// Returns true if there is currently an active FIDL client connection to the `HandsFree`
    /// protocol.
    fn fidl_client_active(&mut self) -> bool {
        self.hands_free_request_maybe_stream
            .inner_mut()
            .map_or(false, |s| !s.control_handle().is_closed())
    }

    /// Clear any active FIDL client connection resources, including outstanding responders.
    fn clear_active_fidl_client(&mut self) {
        // Reset the FIDL connection to the `HandsFree` protocol.
        let _old_stream = MaybeStream::take(&mut self.hands_free_request_maybe_stream);
        // Evict any outstanding `HandsFree.WatchPeerConnected` responders.
        self.watch_peer_connected_hanging_get_matcher.clear_left_queue();
    }

    /// Handle incoming profile events, HFP FIDL streams from new client connections and HandsFree
    /// FIDL protocol events. This is all the incoming events that are not specific to a single
    /// peer.
    pub async fn run(mut self) -> Result<()> {
        loop {
            select! {
                profile_event_result_option = self.profile_client.next() => {
                    debug!("Received profile event: {:?}", profile_event_result_option);
                    let profile_event_result = profile_event_result_option
                        .ok_or_else(|| format_err!("Profile client stream closed."))?;
                    let profile_event = profile_event_result?;
                    self.handle_profile_event(profile_event)?;
                },

                (peer_id, protocol) = self.search_result_timers.select_next_some() => {
                    debug!("Timer for search result from peer {} expired.", peer_id);
                    self.handle_search_result_timer_expiry(peer_id, protocol).await;
                },

                hands_free_request_stream_option = self.hands_free_connection_stream.next() => {
                    let stream_str = hands_free_request_stream_option
                     .as_ref().map(|_stream| "<stream>");
                    info!("HandsFree FIDL protocol client connected: {:?}", stream_str);
                    let hands_free_request_stream = hands_free_request_stream_option
                        .ok_or_else(|| format_err!("HandsFree FIDL protocol connection stream closed."))?;
                    self.handle_hands_free_request_stream(hands_free_request_stream)
                },

                hands_free_request_option = self.hands_free_request_maybe_stream.next() => {
                    info!("Received HandsFree FIDL protocol request: {:?}",
                        hands_free_request_option);
                    let err = match hands_free_request_option {
                        Some(Ok(hands_free_request)) => {
                            self.handle_hands_free_request(hands_free_request);
                            continue;
                        }
                        Some(Err(e)) => format!("{e:?}"),
                        None => format!("HandsFree stream closed"),
                    };
                    warn!("Dropping HandsFree FIDL protocol request stream: {err}");
                    self.clear_active_fidl_client();
                }
                watch_peer_connected_send_response_result = self.watch_peer_connected_hanging_get_matcher.next() => {
                    if let Some(Err(err)) = watch_peer_connected_send_response_result {
                        warn!("Error sending peer connected result: {:?}", err);
                    }
                    // None means the stream was closed, but that's not an error; it could reopen
                    // later when more elements are enqueued.

                }
                finished_peer_option = self.peers.next() => {
                    if let Some(finished_peer) = finished_peer_option {
                        info!("Peer task for peer {:?} finished.", finished_peer);
                        // Peer is automatically removed by FutureMap on completion.
                    }
                    // Otherwise the map is empty.
                }
            }
        }
    }

    fn handle_profile_event(&mut self, event: ProfileEvent) -> Result<()> {
        let peer_id = event.peer_id();

        let peer = self.peers.inner().entry(peer_id).or_insert_with(|| {
            Box::pin(Peer::new(
                peer_id,
                self.hf_features,
                self.profile_proxy.clone(),
                self.sco_connector.clone(),
                self.audio_control.clone(),
            ))
        });

        match event {
            ProfileEvent::PeerConnected { channel, .. } => {
                info!("Received peer_connected for peer {}.", peer_id);
                let peer_handler_client_end = peer.handle_peer_connected(channel);
                self.report_peer_handler(peer_id, peer_handler_client_end)
            }
            ProfileEvent::SearchResult { protocol, .. } => {
                debug!("Received search results for peer {}", peer_id);

                if peer.task_exists() {
                    debug!(
                        "Peer task already created by previous profile event for peer {}",
                        peer_id
                    );
                } else {
                    debug!("Setting timer for peer task search results for peer {}", peer_id);

                    // Convert FIDL ProtocolDescriptor to BT ProtocolDescriptor.
                    let protocol = protocol.map_or(Ok(None), |p| {
                        p.iter()
                            .map(|p| ProtocolDescriptor::try_from(p))
                            .collect::<Result<Vec<_>, _>>()
                            .map(|p| Some(p))
                    })?;

                    let search_result_timer = Self::search_result_timer(peer_id, protocol);
                    self.search_result_timers.push(search_result_timer);
                }
            }
        }
        Ok(())
    }

    // We expect peers to connect to us.  If they don't connect to us but we get
    // a search result, we should connect to them.  To prevent races where both
    // we and the remote peer attempt to connect to the other simultaneously, we
    // delay connecting after receiving a search result and see if the remote
    // peer has connected first.
    fn search_result_timer(
        peer_id: PeerId,
        protocol: Option<Vec<ProtocolDescriptor>>,
    ) -> SearchResultTimer {
        let time = fasync::MonotonicInstant::after(SEARCH_RESULT_CONNECT_DELAY_DURATION);
        let timer = fasync::Timer::new(time);

        let fut = FutureExt::map(timer, move |_| (peer_id, protocol));

        Box::pin(fut)
    }

    async fn handle_search_result_timer_expiry(
        &mut self,
        peer_id: PeerId,
        protocol: Option<Vec<ProtocolDescriptor>>,
    ) {
        debug!("Handle search results timer expired for peer {:?}", peer_id);

        let peer_result = self.peers.inner().get_mut(&peer_id);

        let peer_handler_client_end_result = match peer_result {
            None => {
                info!("Peer task for peer {} completed before handling search result.", peer_id);
                Ok(None)
            }
            Some(peer) => peer.handle_search_result(protocol).await,
        };

        let peer_handler_client_end_option = match peer_handler_client_end_result {
            Ok(proxy) => proxy,
            Err(err) => {
                // An error handling one peer should not be a fatal error.
                warn!("Error handling search result timer expiry for peer {:}: {:?}", peer_id, err);
                let _removed_peer = self.peers.remove(&peer_id);
                return; // Early return.
            }
        };

        if let Some(peer_handler_client_end) = peer_handler_client_end_option {
            self.report_peer_handler(peer_id, peer_handler_client_end);
        } // Else the task was already created for this peer so nothing to do.
    }

    /// Report the PeerHandler clienr end of a newly connected peer to the FIDL client of th HFP protocol
    fn report_peer_handler(
        &mut self,
        peer_id: PeerId,
        peer_handler_client_end: ClientEnd<fidl_hfp::PeerHandlerMarker>,
    ) {
        self.watch_peer_connected_hanging_get_matcher
            .enqueue_right((peer_id, peer_handler_client_end));
    }

    /// Attempts to serve a new FIDL client connection to the `HandsFree` protocol.
    fn handle_hands_free_request_stream(&mut self, stream: fidl_hfp::HandsFreeRequestStream) {
        if self.fidl_client_active() {
            info!("New `HandsFree` connection while one already exists. Closing the new stream.");
            stream.control_handle().shutdown_with_epitaph(zx::Status::ALREADY_BOUND);
            return;
        }

        // Clear any stale FIDL responders from the previous FIDL client connection.
        self.clear_active_fidl_client();
        self.hands_free_request_maybe_stream.set(stream);
        // TODO(https://fxbug.dev/42077961) Update HangingGet with all peers. Make sure to set the
        // new PeerProxy on each peer. Careful of races between the new PeerProxy and any old ones
    }

    fn handle_hands_free_request(&mut self, request: fidl_hfp::HandsFreeRequest) {
        let fidl_hfp::HandsFreeRequest::WatchPeerConnected { responder } = request;
        self.watch_peer_connected_hanging_get_matcher.enqueue_left(responder);
    }
}
