// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Context as _;
use bt_avdtp::{
    self as avdtp, MediaCodecType, ServiceCapability, ServiceCategory, StreamEndpoint,
    StreamEndpointId,
};
use fidl_fuchsia_bluetooth::ChannelParameters;
use fidl_fuchsia_bluetooth_avrcp as avrcp;
use fidl_fuchsia_bluetooth_bredr::{
    ConnectParameters, L2capParameters, PSM_AVDTP, ProfileDescriptor, ProfileProxy,
};
use fuchsia_async as fasync;
use fuchsia_bluetooth::inspect::DebugExt;
use fuchsia_bluetooth::types::{Channel, PeerId};
use fuchsia_inspect as inspect;
use fuchsia_inspect_derive::{AttachError, Inspect};
use fuchsia_sync::Mutex;
use futures::channel::mpsc;
use futures::future::{BoxFuture, Either};
use futures::stream::FuturesUnordered;
use futures::task::{Context, Poll, Waker};
use futures::{Future, FutureExt, StreamExt, select};
use log::{debug, info, trace, warn};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::pin::Pin;
use std::sync::{Arc, Weak};

/// For sending out-of-band commands over the A2DP peer.
mod controller;
pub use controller::ControllerPool;
mod volume_relay;
pub(crate) use volume_relay::run_avrcp_volume_relay;

use crate::codec::MediaCodecConfig;
use crate::media_task::MediaTaskStatus;
use crate::permits::{Permit, Permits};
use crate::stream::{Stream, Streams};

/// A Peer represents an A2DP peer which may be connected to this device.
/// Only one A2DP peer should exist for each Bluetooth peer.
#[derive(Inspect)]
pub struct Peer {
    /// The id of the peer we are connected to.
    id: PeerId,
    /// Inner keeps track of the peer and the streams.
    #[inspect(forward)]
    inner: Arc<Mutex<PeerInner>>,
    /// Profile Proxy to connect new transport channels
    profile: ProfileProxy,
    /// The profile descriptor for this peer, if it has been discovered.
    descriptor: Mutex<Option<ProfileDescriptor>>,
    /// Wakers that are to be woken when the peer disconnects.  If None, the peers have been woken
    /// and this peer is disconnected.  Shared weakly with ClosedPeer future objects that complete
    /// when the peer disconnects.
    closed_wakers: Arc<Mutex<Option<Vec<Waker>>>>,
    /// Used to report peer metrics to Cobalt.
    metrics: bt_metrics::MetricsLogger,
}

/// How long to wait after a stream is opened for the peer to start it before we start it
/// ourselves.  Chosen to produce reasonably quick startup while allowing for a peer start.
const STREAM_DWELL: zx::MonotonicDuration = zx::MonotonicDuration::from_millis(500);

/// How long to wait before trying to start a stream again after a start attempt failed.
const START_RETRY_DELAY: zx::MonotonicDuration = zx::MonotonicDuration::from_seconds(1);

/// How many times to try to start a stream, `START_RETRY_DELAY` apart, before giving up and
/// waiting for a new reason to start it.
const START_ATTEMPTS: usize = 3;

/// StreamPermits handles reserving and retrieving permits for streaming audio.
/// Reservations are automatically retrieved for streams that are revoked, and when the
/// reservation is completed, the permit is stored and a StreamPermit is sent so it can be started.
#[derive(Clone)]
struct StreamPermits {
    permits: Permits,
    open_streams: Arc<Mutex<HashMap<StreamEndpointId, Permit>>>,
    reserved_streams: Arc<Mutex<HashSet<StreamEndpointId>>>,
    inner: Weak<Mutex<PeerInner>>,
    peer_id: PeerId,
    sender: mpsc::UnboundedSender<BoxFuture<'static, StreamPermit>>,
}

#[derive(Debug)]
struct StreamPermit {
    local_id: StreamEndpointId,
    open_streams: Arc<Mutex<HashMap<StreamEndpointId, Permit>>>,
}

impl StreamPermit {
    fn local_id(&self) -> &StreamEndpointId {
        &self.local_id
    }

    /// Returns true if a Permit is held for this stream endpoint.
    fn is_held(&self) -> bool {
        self.open_streams.lock().contains_key(&self.local_id)
    }
}

impl Drop for StreamPermit {
    fn drop(&mut self) {
        let _ = self.open_streams.lock().remove(&self.local_id);
    }
}

impl StreamPermits {
    fn new(
        inner: Weak<Mutex<PeerInner>>,
        peer_id: PeerId,
        permits: Permits,
    ) -> (Self, mpsc::UnboundedReceiver<BoxFuture<'static, StreamPermit>>) {
        let (sender, reservations_receiver) = futures::channel::mpsc::unbounded();
        (
            Self {
                inner,
                permits,
                peer_id,
                sender,
                open_streams: Default::default(),
                reserved_streams: Default::default(),
            },
            reservations_receiver,
        )
    }

    fn label_for(&self, local_id: &StreamEndpointId) -> String {
        format!("{} {}", self.peer_id, local_id)
    }

    /// Get a permit to stream on the stream with id `local_id`.
    /// Returns Some() if there is a permit available.
    fn get(&self, local_id: StreamEndpointId) -> Option<StreamPermit> {
        let revoke_fn = self.make_revocation_fn(&local_id);
        let Some(permit) = self.permits.get_revokable(revoke_fn) else {
            info!("No permits available: {:?}", self.permits);
            return None;
        };
        permit.relabel(self.label_for(&local_id));
        if let Some(_) = self.open_streams.lock().insert(local_id.clone(), permit) {
            warn!(id:% = self.peer_id; "Started stream {local_id:?} twice, dropping previous permit");
        }
        Some(StreamPermit { local_id, open_streams: self.open_streams.clone() })
    }

    /// Get a reservation that will resolve to a StreamPermit to start a stream with the id
    /// `local_id`
    fn setup_reservation_for(&self, local_id: StreamEndpointId) {
        if !self.reserved_streams.lock().insert(local_id.clone()) {
            // Already reserved.
            return;
        }
        let restart_stream_available_fut = {
            let self_revoke_fn = Self::make_revocation_fn(&self, &local_id);
            let reservation = self.permits.reserve_revokable(self_revoke_fn);
            let open_streams = self.open_streams.clone();
            let reserved_streams = self.reserved_streams.clone();
            let label = self.label_for(&local_id);
            let local_id = local_id.clone();
            async move {
                let permit = reservation.await;
                permit.relabel(label);
                if open_streams.lock().insert(local_id.clone(), permit).is_some() {
                    warn!("Reservation replaces acquired permit for {}", local_id.clone());
                }
                if !reserved_streams.lock().remove(&local_id) {
                    warn!(local_id:%; "Unrecorded reservation resolved");
                }
                StreamPermit { local_id, open_streams }
            }
        };
        if let Err(e) = self.sender.unbounded_send(restart_stream_available_fut.boxed()) {
            warn!(id:% = self.peer_id, local_id:%, e:?; "Couldn't queue reservation to finish");
        }
    }

    /// Revokes a permit that was previously delivered, suspending the local stream and signaling
    /// the peer.
    /// The permit must have been previously received through StreamPermits::get or a have been
    /// restarted after being revoked, otherwise will panic.
    fn revocation_fn(self, local_id: StreamEndpointId) -> Permit {
        if let Ok(peer) = PeerInner::upgrade(self.inner.clone()) {
            {
                let mut lock = peer.lock();
                match lock.suspend_local_stream(&local_id) {
                    Ok(remote_id) => drop(lock.peer.suspend(&[remote_id])),
                    Err(e) => warn!("Couldn't stop local stream {local_id:?}: {e:?}"),
                }
            }
            self.setup_reservation_for(local_id.clone());
        }
        self.open_streams.lock().remove(&local_id).expect("permit revoked but don't have it")
    }

    fn make_revocation_fn(&self, local_id: &StreamEndpointId) -> impl FnOnce() -> Permit + use<> {
        let local_id = local_id.clone();
        let cloned = self.clone();
        move || cloned.revocation_fn(local_id)
    }
}

impl Peer {
    /// Make a new Peer which is connected to the peer `id` using the AVDTP `peer`.
    /// The `streams` are the local endpoints available to the peer.
    /// `profile` will be used to initiate connections for Media Transport.
    /// The `permits`, if provided, will acquire a permit before starting streams on this peer.
    /// If `metrics` is included, metrics for codec availability will be reported.
    /// This also starts a task on the executor to handle incoming events from the peer.
    pub fn create(
        id: PeerId,
        peer: avdtp::Peer,
        streams: Streams,
        permits: Option<Permits>,
        profile: ProfileProxy,
        avrcp: Option<avrcp::PeerManagerProxy>,
        metrics: bt_metrics::MetricsLogger,
    ) -> Self {
        let inner = Arc::new(Mutex::new(PeerInner::new(peer, id, streams, avrcp, metrics.clone())));
        inner.lock().self_weak = Arc::downgrade(&inner);
        let reservations_receiver = if let Some(permits) = permits {
            let (stream_permits, receiver) =
                StreamPermits::new(Arc::downgrade(&inner), id, permits);
            inner.lock().permits = Some(stream_permits);
            receiver
        } else {
            let (_, receiver) = mpsc::unbounded();
            receiver
        };
        let res = Self {
            id,
            inner,
            profile,
            descriptor: Mutex::new(None),
            closed_wakers: Arc::new(Mutex::new(Some(Vec::new()))),
            metrics,
        };
        res.start_requests_task(reservations_receiver);
        res
    }

    pub fn set_descriptor(&self, descriptor: ProfileDescriptor) -> Option<ProfileDescriptor> {
        self.descriptor.lock().replace(descriptor)
    }

    #[cfg(test)]
    pub fn set_volume_relay_task(&self, task: fasync::Task<()>) {
        self.inner.lock().volume_relay_task = Some(task);
    }

    /// Receive a channel from the peer that was initiated remotely.
    /// This function should be called whenever the peer associated with this opens an L2CAP channel.
    /// If this completes opening a stream, the stream will be scheduled to start locally when the
    /// audio becomes active, or after a dwell if the peer doesn't start it first.
    pub fn receive_channel(&self, channel: Channel) -> avdtp::Result<()> {
        let opened_stream = {
            let mut lock = self.inner.lock();
            lock.receive_channel(channel)?
        };
        if let Some(stream_id) = opened_stream {
            PeerInner::schedule_start(Arc::downgrade(&self.inner), stream_id);
        }
        PeerInner::maybe_start_volume_relay(&self.inner);
        Ok(())
    }

    /// Return a handle to the AVDTP peer, to use as initiator of commands.
    pub fn avdtp(&self) -> avdtp::Peer {
        let lock = self.inner.lock();
        lock.peer.clone()
    }

    /// Returns the stream endpoints discovered by this peer.
    pub fn remote_endpoints(&self) -> Option<Vec<avdtp::StreamEndpoint>> {
        self.inner.lock().remote_endpoints()
    }

    /// Perform Discovery and Collect Capabilities to enumerate the endpoints and capabilities of
    /// the connected peer.
    /// Returns a future which performs the work and resolves to a vector of peer stream endpoints.
    pub fn collect_capabilities(
        &self,
    ) -> impl Future<Output = avdtp::Result<Vec<avdtp::StreamEndpoint>>> + use<> {
        let avdtp = self.avdtp();
        let get_all = self.descriptor.lock().clone().is_some_and(a2dp_version_check);
        let inner = self.inner.clone();
        let metrics = self.metrics.clone();
        let peer_id = self.id;
        async move {
            if let Some(caps) = inner.lock().remote_endpoints() {
                return Ok(caps);
            }
            trace!("Discovering peer streams..");
            let infos = avdtp.discover().await?;
            trace!("Discovered {} streams", infos.len());
            let mut remote_streams = Vec::new();
            for info in infos {
                let capabilities = if get_all {
                    avdtp.get_all_capabilities(info.id()).await
                } else {
                    avdtp.get_capabilities(info.id()).await
                };
                match capabilities {
                    Ok(capabilities) => {
                        trace!("Stream {:?}", info);
                        for cap in &capabilities {
                            trace!("  - {:?}", cap);
                        }
                        remote_streams.push(avdtp::StreamEndpoint::from_info(&info, capabilities));
                    }
                    Err(e) => {
                        info!(peer_id:%; "Stream {} capabilities failed: {:?}, skipping", info.id(), e);
                    }
                };
            }
            inner.lock().set_remote_endpoints(&remote_streams);
            Self::record_cobalt_metrics(metrics, &remote_streams);
            Ok(remote_streams)
        }
    }

    fn record_cobalt_metrics(metrics: bt_metrics::MetricsLogger, endpoints: &[StreamEndpoint]) {
        let codec_metrics: HashSet<_> = endpoints
            .iter()
            .filter_map(|endpoint| {
                endpoint.codec_type().map(|t| codectype_to_availability_metric(t) as u32)
            })
            .collect();
        metrics
            .log_occurrences(bt_metrics::A2DP_CODEC_AVAILABILITY_MIGRATED_METRIC_ID, codec_metrics);

        let cap_metrics: HashSet<_> = endpoints
            .iter()
            .flat_map(|endpoint| {
                endpoint
                    .capabilities()
                    .iter()
                    .filter_map(|t| capability_to_metric(t))
                    .chain(std::iter::once(
                        bt_metrics::A2dpRemotePeerCapabilitiesMetricDimensionCapability::Basic,
                    ))
                    .map(|t| t as u32)
            })
            .collect();
        metrics.log_occurrences(bt_metrics::A2DP_REMOTE_PEER_CAPABILITIES_METRIC_ID, cap_metrics);
    }

    fn transport_channel_params() -> L2capParameters {
        L2capParameters {
            psm: Some(PSM_AVDTP),
            parameters: Some(ChannelParameters {
                max_rx_packet_size: Some(65535),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    /// Open and start a media transport stream, connecting a compatible local stream to the remote
    /// stream `remote_id`, configuring it with the `capabilities` provided.
    /// Returns a future which should be awaited on.
    /// The future returns Ok(()) if successfully started, and an appropriate error otherwise.
    pub fn stream_start(
        &self,
        remote_id: StreamEndpointId,
        capabilities: Vec<ServiceCapability>,
    ) -> impl Future<Output = avdtp::Result<()>> {
        let peer = Arc::downgrade(&self.inner);
        let peer_id = self.id.clone();
        let avdtp = self.avdtp();
        let profile = self.profile.clone();

        async move {
            let codec_params =
                capabilities.iter().find(|x| x.is_codec()).ok_or(avdtp::Error::InvalidState)?;
            let (local_id, local_capabilities) = {
                let peer = PeerInner::upgrade(peer.clone())?;
                let lock = peer.lock();
                lock.find_compatible_local_capabilities(codec_params, &remote_id)?
            };

            let local_by_cat: HashMap<ServiceCategory, ServiceCapability> =
                local_capabilities.into_iter().map(|i| (i.category(), i)).collect();

            // Filter things out if they don't have a match in the local capabilities.
            // Order them by the ServiceCategory ordinal - some noncompliant devices care about it.
            let shared_capabilities: BTreeMap<ServiceCategory, ServiceCapability> = capabilities
                .into_iter()
                .filter_map(|cap| {
                    let Some(local_cap) = local_by_cat.get(&cap.category()) else {
                        return None;
                    };
                    if cap.category() == ServiceCategory::MediaCodec {
                        let Ok(a) = MediaCodecConfig::try_from(&cap) else {
                            return None;
                        };
                        let Ok(b) = MediaCodecConfig::try_from(local_cap) else {
                            return None;
                        };
                        let Some(negotiated) = MediaCodecConfig::negotiate(&a, &b) else {
                            return None;
                        };
                        Some((cap.category(), (&negotiated).into()))
                    } else {
                        Some((cap.category(), cap))
                    }
                })
                .collect();
            let shared_capabilities: Vec<_> = shared_capabilities.into_values().collect();

            trace!("Starting stream {local_id} to remote {remote_id} with {shared_capabilities:?}");

            avdtp.set_configuration(&remote_id, &local_id, &shared_capabilities).await?;
            {
                let strong = PeerInner::upgrade(peer.clone())?;
                strong.lock().set_opening(&local_id, &remote_id, shared_capabilities)?;
            }
            avdtp.open(&remote_id).await?;

            debug!(peer_id:%; "Connecting transport channel");
            let channel = profile
                .connect(
                    &peer_id.into(),
                    &ConnectParameters::L2cap(Self::transport_channel_params()),
                )
                .await
                .context("FIDL error: {}")?
                .or(Err(avdtp::Error::PeerDisconnected))?;

            trace!(peer_id:%; "Connected transport channel, converting to local Channel");

            let channel = match channel.try_into() {
                Err(e) => {
                    warn!(peer_id:%, e:?; "Couldn't connect media transport: no channel");
                    return Err(avdtp::Error::PeerDisconnected);
                }
                Ok(c) => c,
            };

            let opened_stream = {
                let strong = PeerInner::upgrade(peer.clone())?;
                let mut lock = strong.lock();
                lock.receive_channel(channel)?
            };
            if let Some(stream_id) = opened_stream {
                PeerInner::schedule_start(peer, stream_id);
            }
            Ok(())
        }
    }

    /// Query whether any streams are currently started or scheduled to start.
    pub fn streaming_active(&self) -> bool {
        self.inner.lock().is_streaming()
    }

    /// Returns true if there are any streams that are currently started.
    #[cfg(test)]
    fn is_streaming_now(&self) -> bool {
        self.inner.lock().is_streaming_now()
    }

    /// Suspend a media transport stream `local_id`.
    /// It's possible that the stream is not active - a suspend will be attempted, but an
    /// error from the command will be returned.
    /// Returns the result of the suspend command.
    pub fn stream_suspend(
        &self,
        local_id: StreamEndpointId,
    ) -> impl Future<Output = avdtp::Result<()>> {
        let peer = Arc::downgrade(&self.inner);
        PeerInner::suspend(peer, local_id)
    }

    /// Start an asynchronous task to handle any requests from the AVDTP peer.
    /// This task completes when the remote end closes the signaling connection.
    fn start_requests_task(
        &self,
        mut reservations_receiver: mpsc::UnboundedReceiver<BoxFuture<'static, StreamPermit>>,
    ) {
        let lock = self.inner.lock();
        let mut request_stream = lock.peer.take_request_stream();
        let id = self.id.clone();
        let peer = Arc::downgrade(&self.inner);
        let mut stream_reservations = FuturesUnordered::new();
        let disconnect_wakers = Arc::downgrade(&self.closed_wakers);
        fuchsia_async::Task::local(async move {
            loop {
                select! {
                    request = request_stream.next() => {
                        match request {
                            None => break,
                            Some(Err(e)) => info!(peer_id:% = id, e:?; "Request stream error"),
                            Some(Ok(request)) => match peer.upgrade() {
                                None => return,
                                Some(p) => {
                                    let result_or_future = p.lock().handle_request(request);
                                    let result = match result_or_future {
                                        Either::Left(result) => result,
                                        Either::Right(future) => future.await,
                                    };
                                    if let Err(e) = result {
                                        warn!(peer_id:% = id, e:?; "Error handling request");
                                    }
                                    PeerInner::maybe_start_volume_relay(&p);
                                }
                            },
                        }
                    },
                    reservation_fut = reservations_receiver.select_next_some() => {
                        stream_reservations.push(reservation_fut)
                    },
                    permit = stream_reservations.select_next_some() => {
                        if let Err(e) = PeerInner::start_permit(peer.clone(), permit).await {
                            warn!(peer_id:% = id, e:?; "Couldn't start stream after unpause");
                        }
                    }
                    complete => break,
                }
            }
            info!(peer_id:% = id; "disconnected");
            if let Some(wakers) = disconnect_wakers.upgrade() {
                for waker in wakers.lock().take().unwrap_or_else(Vec::new) {
                    waker.wake();
                }
            }
        })
        .detach();
    }

    /// Returns a future that will complete when the peer disconnects.
    pub fn closed(&self) -> ClosedPeer {
        ClosedPeer { inner: Arc::downgrade(&self.closed_wakers) }
    }
}

/// Future which completes when the A2DP peer has closed the control connection.
/// See `Peer::closed`
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct ClosedPeer {
    inner: Weak<Mutex<Option<Vec<Waker>>>>,
}

impl Future for ClosedPeer {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.inner.upgrade() {
            None => Poll::Ready(()),
            Some(inner) => match inner.lock().as_mut() {
                None => Poll::Ready(()),
                Some(wakers) => {
                    wakers.push(cx.waker().clone());
                    Poll::Pending
                }
            },
        }
    }
}

/// Determines if Peer profile version is newer (>= 1.3) or older (< 1.3)
fn a2dp_version_check(profile: ProfileDescriptor) -> bool {
    let (Some(major), Some(minor)) = (profile.major_version, profile.minor_version) else {
        return false;
    };
    (major == 1 && minor >= 3) || major > 1
}

/// Peer handles the communication with the AVDTP layer, and provides responses as appropriate
/// based on the current state of local streams available.
/// Each peer has its own set of local stream endpoints, and tracks a set of remote peer endpoints.
struct PeerInner {
    /// AVDTP peer communicating to this.
    peer: avdtp::Peer,
    /// The PeerId that this peer is representing
    peer_id: PeerId,
    /// Some(local_id) if an endpoint has been configured but hasn't finished opening.
    /// Per AVDTP Sec 6.11 only up to one stream can be in this state.
    opening: Option<StreamEndpointId>,
    /// The local stream endpoint collection
    local: Streams,
    /// The permits that are available for this peer.
    permits: Option<StreamPermits>,
    /// Tasks watching for the end of a started stream. Key is the local stream id.
    started: HashMap<StreamEndpointId, WatchedStream>,
    /// Tasks deciding whether a stream that is not started should be: waiting for the stream to
    /// become activated, or cleaning up after a stream that finished and may start again.
    waiting_start_tasks: HashMap<StreamEndpointId, fasync::Task<()>>,
    /// The inspect node for this peer
    inspect: fuchsia_inspect::Node,
    /// The set of discovered remote endpoints. None until set.
    remote_endpoints: Option<Vec<StreamEndpoint>>,
    /// The inspect node representing the remote endpoints.
    remote_inspect: fuchsia_inspect::Node,
    /// Cobalt logger used to report peer metrics.
    metrics: bt_metrics::MetricsLogger,
    /// AVRCP client to control the peer's volume.
    avrcp: Option<avrcp::PeerManagerProxy>,
    /// Weak reference to self for background tasks.
    self_weak: Weak<Mutex<PeerInner>>,
    /// Task that runs the AVRCP Absolute Volume relay loop.
    volume_relay_task: Option<fasync::Task<()>>,
}

impl Inspect for &mut PeerInner {
    // Set up the StreamEndpoint to update the state
    // The MediaTask node will be created when the media task is started.
    fn iattach(self, parent: &inspect::Node, name: impl AsRef<str>) -> Result<(), AttachError> {
        self.inspect = parent.create_child(name.as_ref());
        self.inspect.record_string("id", self.peer_id.to_string());
        self.local.iattach(&self.inspect, "local_streams")
    }
}

impl PeerInner {
    pub fn new(
        peer: avdtp::Peer,
        peer_id: PeerId,
        local: Streams,
        avrcp: Option<avrcp::PeerManagerProxy>,
        metrics: bt_metrics::MetricsLogger,
    ) -> Self {
        Self {
            peer,
            peer_id,
            opening: None,
            local,
            permits: None,
            started: HashMap::new(),
            waiting_start_tasks: HashMap::new(),
            inspect: Default::default(),
            remote_endpoints: None,
            remote_inspect: Default::default(),
            metrics,
            avrcp,
            self_weak: Weak::new(),
            volume_relay_task: None,
        }
    }

    pub fn maybe_start_volume_relay(this: &Arc<Mutex<Self>>) {
        let mut lock = this.lock();
        let Some(avrcp) = lock.avrcp.clone() else {
            if lock.volume_relay_task.take().is_some() {
                let peer_id = lock.peer_id;
                trace!(peer_id:%; "Stopping volume relay task (AVRCP proxy missing)");
            }
            return;
        };

        // Check if any of our local streams are Open or Streaming, AND they are of type Source.
        let active_source_stream = lock
            .local
            .streaming()
            .any(|s| s.endpoint().endpoint_type() == &avdtp::EndpointType::Source)
            || lock
                .local
                .open()
                .any(|s| s.endpoint().endpoint_type() == &avdtp::EndpointType::Source);

        if !active_source_stream {
            if lock.volume_relay_task.take().is_some() {
                let peer_id = lock.peer_id;
                trace!(peer_id:%; "Stopping volume relay task (No active local Source streams)");
            }
            return;
        }

        if let Some(task) = lock.volume_relay_task.as_mut() {
            if let Some(res) = task.now_or_never() {
                let peer_id = lock.peer_id;
                trace!(peer_id:%; "Volume relay task completed: {res:?}");
                lock.volume_relay_task = None;
            }
        }

        if lock.volume_relay_task.is_some() {
            return;
        }

        let peer_id = lock.peer_id;
        trace!(peer_id:%; "Spawning volume relay task for A2DP sink peer");
        let task = fasync::Task::spawn(run_avrcp_volume_relay(peer_id, avrcp));
        lock.volume_relay_task = Some(task);
    }

    /// Returns an endpoint from the local set or a BadAcpSeid error if it doesn't exist.
    fn get_mut(&mut self, local_id: &StreamEndpointId) -> Result<&mut Stream, avdtp::ErrorCode> {
        self.local.get_mut(&local_id).ok_or(avdtp::ErrorCode::BadAcpSeid)
    }

    fn set_remote_endpoints(&mut self, endpoints: &[StreamEndpoint]) {
        self.remote_inspect = self.inspect.create_child("remote_endpoints");
        for endpoint in endpoints {
            self.remote_inspect.record_child(inspect::unique_name("remote_"), |node| {
                node.record_string("endpoint_id", endpoint.local_id().debug());
                node.record_string("capabilities", endpoint.capabilities().debug());
                node.record_string("type", endpoint.endpoint_type().debug());
            });
        }
        self.remote_endpoints = Some(endpoints.iter().map(StreamEndpoint::as_new).collect());
    }

    /// If the remote endpoints have been set, returns a copy of the endpoints.
    fn remote_endpoints(&self) -> Option<Vec<StreamEndpoint>> {
        self.remote_endpoints.as_ref().map(|v| v.iter().map(StreamEndpoint::as_new).collect())
    }

    /// If the remote endpoint with endpoint `id` exists, return a copy of the endpoint.
    fn remote_endpoint(&self, id: &StreamEndpointId) -> Option<StreamEndpoint> {
        self.remote_endpoints
            .as_ref()
            .and_then(|v| v.iter().find(|v| v.local_id() == id).map(StreamEndpoint::as_new))
    }

    /// Returns true if there is at least one stream that has started or is starting for this peer.
    fn is_streaming(&self) -> bool {
        self.is_streaming_now() || self.opening.is_some() || !self.waiting_start_tasks.is_empty()
    }

    /// Returns true if there is at least one stream in the started state for this peer.
    fn is_streaming_now(&self) -> bool {
        self.local.streaming().next().is_some()
    }

    fn set_opening(
        &mut self,
        local_id: &StreamEndpointId,
        remote_id: &StreamEndpointId,
        capabilities: Vec<ServiceCapability>,
    ) -> avdtp::Result<()> {
        if self.opening.is_some() {
            return Err(avdtp::Error::InvalidState);
        }
        let peer_id = self.peer_id;
        let stream = self.get_mut(&local_id).map_err(|e| avdtp::Error::RequestInvalid(e))?;
        stream
            .configure(&peer_id, &remote_id, capabilities)
            .map_err(|(cat, c)| avdtp::Error::RequestInvalidExtra(c, (&cat).into()))?;
        stream.endpoint_mut().establish().or(Err(avdtp::Error::InvalidState))?;
        self.opening = Some(local_id.clone());
        Ok(())
    }

    fn upgrade(weak: Weak<Mutex<Self>>) -> avdtp::Result<Arc<Mutex<Self>>> {
        weak.upgrade().ok_or(avdtp::Error::PeerDisconnected)
    }

    /// Schedule a task to start streaming on `local_id` when it should be started locally:
    /// when the audio becomes active for source streams, or after a dwell waiting for the peer to
    /// start for sink streams.
    fn schedule_start(weak: Weak<Mutex<Self>>, local_id: StreamEndpointId) {
        let Ok(peer) = Self::upgrade(weak.clone()) else {
            return;
        };
        let mut peer_lock = peer.lock();
        if peer_lock.started.contains_key(&local_id) {
            return;
        }
        if let Some(waiting_start_task) = peer_lock.waiting_start_tasks.get_mut(&local_id) {
            let mut noop_cx = Context::from_waker(futures::task::noop_waker_ref());
            if let Poll::Pending = waiting_start_task.poll_unpin(&mut noop_cx) {
                return;
            }
        }
        let task = fasync::Task::spawn(Self::wait_and_start(weak, local_id.clone()));
        let _ = peer_lock.waiting_start_tasks.insert(local_id, task);
    }

    /// Forget the task that is waiting to start `local_id`, which is no longer waiting to start it.
    /// The task handle is detached rather than dropped, since dropping it would cancel the task
    /// that is calling this.
    fn forget_waiting_start(weak: &Weak<Mutex<Self>>, local_id: &StreamEndpointId) {
        let Ok(peer) = Self::upgrade(weak.clone()) else {
            return;
        };
        let waiting_start_task = peer.lock().waiting_start_tasks.remove(local_id);
        if let Some(task) = waiting_start_task {
            task.detach();
        }
    }

    /// Wait until `local_id` should be started locally, then start it.
    ///
    /// Source streams are started when the audio becomes active, and are not started while the
    /// audio is inactive since there would be nothing to send.  If the audio goes inactive and
    /// becomes active again later, the stream is started again.
    ///
    /// Sink streams are normally started by the peer, which is the source of the audio.  If the
    /// peer hasn't started the stream `STREAM_DWELL` after it was opened, we start it once
    /// ourselves.  After that the peer is in control: if it suspends the stream, we wait for the
    /// peer to start it again instead of starting it for them.
    ///
    /// Either way, a start that fails is retried up to `START_ATTEMPTS` times, `START_RETRY_DELAY`
    /// apart, for as long as the reason to start the stream holds.
    async fn wait_and_start(weak: Weak<Mutex<Self>>, local_id: StreamEndpointId) {
        // Returns a future that resolves to true when the audio becomes active and false when it
        // becomes inactive, or None if the stream is gone.  Only changes are reported, so this can
        // stay pending forever: see `MediaTaskRunner::watch_active`.
        let watch_active = |weak: &Weak<Mutex<Self>>, local_id| {
            let Ok(peer) = Self::upgrade(weak.clone()) else {
                return None;
            };
            let mut peer = peer.lock();
            let Ok(stream) = peer.get_mut(local_id) else {
                return None;
            };
            Some(stream.watch_active())
        };

        let is_source = {
            let Ok(peer) = Self::upgrade(weak.clone()) else {
                return;
            };
            let mut peer = peer.lock();
            let Ok(stream) = peer.get_mut(&local_id) else {
                return;
            };
            stream.endpoint().endpoint_type() == &avdtp::EndpointType::Source
        };

        'wait_start: loop {
            if is_source {
                let Some(mut wait_start_fut) = watch_active(&weak, &local_id) else {
                    return;
                };
                while !wait_start_fut.await {
                    // The audio is inactive, there would be nothing to send.  Wait again.
                    let Some(new_wait_start_fut) = watch_active(&weak, &local_id) else {
                        return;
                    };
                    wait_start_fut = new_wait_start_fut;
                }
            } else {
                // Give the peer a chance to start the stream before we do.
                fasync::Timer::new(fasync::MonotonicInstant::after(STREAM_DWELL)).await;
            }

            for attempt in 1..=START_ATTEMPTS {
                let Err(e) = Self::start_stream(&weak, &local_id).await else {
                    return;
                };
                warn!(local_id:%, attempt, e:?; "Error starting stream");
                if attempt == START_ATTEMPTS {
                    break;
                }
                let mut retry_timer = std::pin::pin!(fasync::Timer::new(
                    fasync::MonotonicInstant::after(START_RETRY_DELAY)
                ));
                if !is_source {
                    retry_timer.await;
                    continue;
                }
                let Some(wait_stop_fut) = watch_active(&weak, &local_id) else {
                    return;
                };
                match futures::future::select(retry_timer.as_mut(), wait_stop_fut).await {
                    // The audio stopped: wait until it becomes active again to start.
                    Either::Right((false, _)) => continue 'wait_start,
                    // Still active.  Stop watching and wait out the rest of the delay: watching
                    // again here would spin for sources that are always active.
                    Either::Right((true, _)) => retry_timer.await,
                    // The retry delay elapsed, try again.
                    Either::Left(_) => {}
                }
            }

            if !is_source {
                // We only start a sink stream when it is opened.  The peer is the source of the
                // audio, so if it wants to stream later it can start the stream itself.
                Self::forget_waiting_start(&weak, &local_id);
                return;
            }
        }
    }

    /// Attempt to start the stream `local_id` locally.
    /// Returns Ok if the stream was started, or if no permit was available to stream, in which
    /// case a reservation has been made and the stream will be started when one is available.
    async fn start_stream(
        weak: &Weak<Mutex<Self>>,
        local_id: &StreamEndpointId,
    ) -> avdtp::Result<()> {
        let peer = Self::upgrade(weak.clone())?;
        let (avdtp, remote_id, permit_result) = {
            let mut peer = peer.lock();
            let stream = peer.get_mut(local_id).map_err(|e| avdtp::Error::RequestInvalid(e))?;
            let remote_id =
                stream.endpoint().remote_id().cloned().ok_or(avdtp::Error::InvalidState)?;
            let avdtp = peer.peer.clone();
            let permit_result = peer.get_permit_or_reserve(local_id);
            (avdtp, remote_id, permit_result)
        };
        let Ok(permit) = permit_result else {
            // A reservation was made, we will be started when a permit is available.
            return Ok(());
        };
        Self::initiated_start(avdtp, weak.clone(), permit, local_id, &remote_id).await
    }

    async fn start_permit(weak: Weak<Mutex<Self>>, permit: StreamPermit) -> avdtp::Result<()> {
        let local_id = permit.local_id().clone();
        let (avdtp, remote_id) = {
            let peer = Self::upgrade(weak.clone())?;
            let mut peer = peer.lock();
            let remote_id = peer
                .get_mut(&local_id)
                .map_err(|e| avdtp::Error::RequestInvalid(e))?
                .endpoint()
                .remote_id()
                .ok_or(avdtp::Error::InvalidState)?
                .clone();
            (peer.peer.clone(), remote_id)
        };
        Self::initiated_start(avdtp, weak, Some(permit), &local_id, &remote_id).await
    }

    /// Start a stream for a local reason.  Requires a Permit to start streaming for the local stream.
    async fn initiated_start(
        avdtp: avdtp::Peer,
        weak: Weak<Mutex<Self>>,
        permit: Option<StreamPermit>,
        local_id: &StreamEndpointId,
        remote_id: &StreamEndpointId,
    ) -> avdtp::Result<()> {
        trace!(permit:?, local_id:?, remote_id:?; "Making outgoing start request");
        let to_start = std::slice::from_ref(remote_id);
        avdtp.start(to_start).await?;
        trace!("Start response received: {permit:?}");
        let peer = Self::upgrade(weak.clone())?;
        let (peer_id, start_result) = {
            let mut peer = peer.lock();
            (peer.peer_id, peer.start_local_stream(permit, &local_id))
        };
        if let Err(e) = start_result {
            warn!(peer_id:%, local_id:%, remote_id:%, e:?; "Failed to start local stream, suspending");
            avdtp.suspend(to_start).await?;
        } else {
            Self::maybe_start_volume_relay(&peer);
        }
        Ok(())
    }

    /// Suspend a stream locally, returning a future to get the result from the peer.
    fn suspend(
        weak: Weak<Mutex<Self>>,
        local_id: StreamEndpointId,
    ) -> impl Future<Output = avdtp::Result<()>> {
        let res = (move || {
            let peer = Self::upgrade(weak.clone())?;
            let mut peer = peer.lock();
            Ok((peer.peer.clone(), peer.suspend_local_stream(&local_id)?))
        })();
        let (avdtp, remote_id) = match res {
            Err(e) => return futures::future::err(e).left_future(),
            Ok(r) => r,
        };
        let to_suspend = &[remote_id];
        avdtp.suspend(to_suspend).right_future()
    }

    /// Finds a stream in the local stream set which is compatible with the remote_id given the codec config.
    /// Returns the local stream ID and capabilities if found, or OutOfRange if one could not be found.
    pub fn find_compatible_local_capabilities(
        &self,
        codec_params: &ServiceCapability,
        remote_id: &StreamEndpointId,
    ) -> avdtp::Result<(StreamEndpointId, Vec<ServiceCapability>)> {
        let config = codec_params.try_into()?;
        let our_direction = self.remote_endpoint(remote_id).map(|e| e.endpoint_type().opposite());
        debug!(codec_params:?, local:? = self.local; "Looking for compatible local stream");
        self.local
            .compatible(config)
            .find_map(|s| {
                let endpoint = s.endpoint();
                if let Some(d) = our_direction {
                    if &d != endpoint.endpoint_type() {
                        return None;
                    }
                }
                Some((endpoint.local_id().clone(), endpoint.capabilities().clone()))
            })
            .ok_or(avdtp::Error::OutOfRange)
    }

    /// Attempts to acquire a permit for streaming, if the permits are set.
    /// Returns Ok if is is okay to stream, and Err if the permit was not available and a
    /// reservation was made.
    fn get_permit_or_reserve(
        &self,
        local_id: &StreamEndpointId,
    ) -> Result<Option<StreamPermit>, ()> {
        let Some(permits) = self.permits.as_ref() else {
            return Ok(None);
        };
        if let Some(permit) = permits.get(local_id.clone()) {
            return Ok(Some(permit));
        }
        info!(peer_id:% = self.peer_id, local_id:%; "No permit to start stream, adding a reservation");
        permits.setup_reservation_for(local_id.clone());
        Err(())
    }

    /// Starts the stream which is in the local Streams with `local_id`.
    /// Requires a permit to stream.
    fn start_local_stream(
        &mut self,
        permit: Option<StreamPermit>,
        local_id: &StreamEndpointId,
    ) -> avdtp::Result<()> {
        let peer_id = self.peer_id;
        let stream = self.get_mut(&local_id).map_err(|e| avdtp::Error::RequestInvalid(e))?;
        // The streaming permit can be revoked while stream setup is in progress. If so, return
        // without starting the local stream.
        if permit.as_ref().is_some_and(|p| !p.is_held()) {
            return Err(avdtp::Error::Other(anyhow::format_err!(
                "streaming permit revoked during setup"
            )));
        }

        info!(peer_id:%, stream:?; "Starting");
        let stream_finished = stream.start().map_err(|c| avdtp::Error::RequestInvalid(c))?;
        let watched_stream =
            WatchedStream::new(permit, stream_finished, self.self_weak.clone(), local_id.clone());
        if self.started.insert(local_id.clone(), watched_stream).is_some() {
            warn!(peer_id:%, local_id:%; "Stream that was already started");
        }
        let _ = self.waiting_start_tasks.remove(local_id);
        Ok(())
    }

    /// Suspend a stream on the local side. Returns the remote StreamEndpointId if the stream was suspended,
    /// or a RequestInvalid error with the error code otherwise.
    fn suspend_local_stream(
        &mut self,
        local_id: &StreamEndpointId,
    ) -> avdtp::Result<StreamEndpointId> {
        let peer_id = self.peer_id;
        let stream = self.get_mut(&local_id).map_err(|e| avdtp::Error::RequestInvalid(e))?;
        let remote_id = stream.endpoint().remote_id().ok_or(avdtp::Error::InvalidState)?.clone();
        info!(peer_id:%; "Suspend stream local {local_id} <-> {remote_id} remote");
        stream.suspend().map_err(|c| avdtp::Error::RequestInvalid(c))?;
        let _ = self.started.remove(local_id);
        Ok(remote_id)
    }

    /// Provide a new established L2CAP channel to this remote peer.
    /// This function should be called whenever the remote associated with this peer opens an
    /// L2CAP channel after the first.
    /// Returns Some(stream_id) if this channel completed the opening sequence.
    fn receive_channel(&mut self, channel: Channel) -> avdtp::Result<Option<StreamEndpointId>> {
        let stream_id = self.opening.as_ref().cloned().ok_or(avdtp::Error::InvalidState)?;
        let stream = self.get_mut(&stream_id).map_err(|e| avdtp::Error::RequestInvalid(e))?;
        let done = !stream.endpoint_mut().receive_channel(channel)?;
        if done {
            self.opening = None;
        }
        info!(peer_id:% = self.peer_id, stream_id:%; "Transport connected");
        Ok(done.then_some(stream_id))
    }

    /// Handle a single request event from the avdtp peer.
    fn handle_request(
        &mut self,
        request: avdtp::Request,
    ) -> Either<avdtp::Result<()>, impl Future<Output = avdtp::Result<()>> + use<>> {
        use avdtp::ErrorCode;
        use avdtp::Request::*;
        trace!("Handling {request:?} from peer..");
        let immediate_result = 'result: {
            match request {
                Discover { responder } => responder.send(&self.local.information()),
                GetCapabilities { responder, stream_id }
                | GetAllCapabilities { responder, stream_id } => match self.local.get(&stream_id) {
                    None => responder.reject(ErrorCode::BadAcpSeid),
                    Some(stream) => responder.send(stream.endpoint().capabilities()),
                },
                Open { responder, stream_id } => {
                    if self.opening.is_none() {
                        break 'result responder.reject(ErrorCode::BadState);
                    }
                    let Ok(stream) = self.get_mut(&stream_id) else {
                        break 'result responder.reject(ErrorCode::BadAcpSeid);
                    };
                    match stream.endpoint_mut().establish() {
                        Ok(()) => responder.send(),
                        Err(_) => responder.reject(ErrorCode::BadState),
                    }
                }
                Close { responder, stream_id } => {
                    let _ = self.waiting_start_tasks.remove(&stream_id);
                    let peer = self.peer.clone();
                    let Ok(stream) = self.get_mut(&stream_id) else {
                        break 'result responder.reject(ErrorCode::BadAcpSeid);
                    };
                    stream.release(responder, &peer)
                }
                SetConfiguration { responder, local_stream_id, remote_stream_id, capabilities } => {
                    if self.opening.is_some() {
                        break 'result responder.reject(ServiceCategory::None, ErrorCode::BadState);
                    }
                    let peer_id = self.peer_id;
                    let Ok(stream) = self.get_mut(&local_stream_id) else {
                        break 'result responder
                            .reject(ServiceCategory::None, ErrorCode::BadAcpSeid);
                    };
                    match stream.configure(&peer_id, &remote_stream_id, capabilities) {
                        Ok(_) => {
                            self.opening = Some(local_stream_id);
                            responder.send()
                        }
                        Err((category, code)) => responder.reject(category, code),
                    }
                }
                GetConfiguration { stream_id, responder } => {
                    let Ok(stream) = self.get_mut(&stream_id) else {
                        break 'result responder.reject(ErrorCode::BadAcpSeid);
                    };
                    let Some(vec_capabilities) = stream.endpoint().get_configuration() else {
                        break 'result responder.reject(ErrorCode::BadState);
                    };
                    responder.send(vec_capabilities.as_slice())
                }
                Reconfigure { responder, local_stream_id, capabilities } => {
                    let Ok(stream) = self.get_mut(&local_stream_id) else {
                        break 'result responder
                            .reject(ServiceCategory::None, ErrorCode::BadAcpSeid);
                    };
                    match stream.reconfigure(capabilities) {
                        Ok(_) => responder.send(),
                        Err((cat, code)) => responder.reject(cat, code),
                    }
                }
                Start { responder, stream_ids } => {
                    let mut immediate_suspend = Vec::new();
                    // Fail on the first failed endpoint, as per the AVDTP spec 8.13 Note 5
                    let result = stream_ids.into_iter().try_for_each(|seid| {
                        let Some(stream) = self.local.get_mut(&seid) else {
                            return Err((seid, ErrorCode::BadAcpSeid));
                        };
                        let remote_id = stream.endpoint().remote_id().cloned();
                        let Some(remote_id) = remote_id else {
                            return Err((seid, ErrorCode::BadState));
                        };
                        let Ok(permit) = self.get_permit_or_reserve(&seid) else {
                            // Happens when we cannot start because of permits.
                            // Accept this one, then queue up for suspend.
                            // We are already reserved for a permit.
                            immediate_suspend.push(remote_id);
                            return Ok(());
                        };
                        match self.start_local_stream(permit, &seid) {
                            Ok(()) => Ok(()),
                            Err(avdtp::Error::RequestInvalid(code)) => Err((seid, code)),
                            Err(_) => Err((seid, ErrorCode::BadState)),
                        }
                    });
                    let response_result = match result {
                        Ok(()) => responder.send(),
                        Err((seid, code)) => responder.reject(&seid, code),
                    };
                    {
                        let peer = self.peer.clone();
                        return Either::Right(async move {
                            if !immediate_suspend.is_empty() {
                                peer.suspend(immediate_suspend.as_slice()).await?;
                            }
                            response_result
                        });
                    }
                }
                Suspend { responder, stream_ids } => {
                    for seid in stream_ids {
                        match self.suspend_local_stream(&seid) {
                            Ok(_remote_id) => {}
                            Err(avdtp::Error::RequestInvalid(code)) => {
                                break 'result responder.reject(&seid, code);
                            }
                            Err(_e) => break 'result responder.reject(&seid, ErrorCode::BadState),
                        }
                    }
                    responder.send()
                }
                Abort { responder, stream_id } => {
                    let _ = self.waiting_start_tasks.remove(&stream_id);
                    let Ok(stream) = self.get_mut(&stream_id) else {
                        // No response is sent on an invalid ID for an Abort
                        break 'result Ok(());
                    };
                    stream.abort();
                    self.opening = self.opening.take().filter(|local_id| local_id != &stream_id);
                    responder.send()
                }
                DelayReport { responder, delay, stream_id } => {
                    // Delay is in 1/10 ms
                    let delay_ns = delay as u64 * 100000;
                    // Record delay to cobalt.
                    self.metrics.log_integer(
                        bt_metrics::AVDTP_DELAY_REPORT_IN_NANOSECONDS_METRIC_ID,
                        delay_ns.try_into().unwrap_or(-1),
                        vec![],
                    );
                    // Report should only come after a stream is configured
                    let Some(stream) = self.local.get_mut(&stream_id) else {
                        break 'result responder.reject(avdtp::ErrorCode::BadAcpSeid);
                    };
                    let delay_str = format!("delay {}.{} ms", delay / 10, delay % 10);
                    let peer = self.peer_id;
                    match stream.set_delay(std::time::Duration::from_nanos(delay_ns)) {
                        Ok(()) => info!(peer:%, stream_id:%; "reported {delay_str}"),
                        Err(avdtp::ErrorCode::BadState) => {
                            info!(peer:%, stream_id:%; "bad state {delay_str}");
                            break 'result responder.reject(avdtp::ErrorCode::BadState);
                        }
                        Err(e) => info!(peer:%, stream_id:%, e:?; "failed {delay_str}"),
                    };
                    // Can't really respond with an Error
                    responder.send()
                }
            }
        };
        Either::Left(immediate_result)
    }
}

/// A WatchedStream holds a task tracking a started stream and ensures actions are performed when
/// the stream media task finishes.
struct WatchedStream {
    _permit_task: fasync::Task<()>,
}

impl WatchedStream {
    fn new(
        permit: Option<StreamPermit>,
        finish_fut: BoxFuture<'static, Result<MediaTaskStatus, anyhow::Error>>,
        weak: Weak<Mutex<PeerInner>>,
        local_id: StreamEndpointId,
    ) -> Self {
        let permit_task = fasync::Task::spawn(async move {
            let finish_result = finish_fut.await;
            drop(permit);
            let Some(peer) = weak.upgrade() else {
                return;
            };
            // Handle the stream finished in waiting_start_tasks, since this task will be
            // dropped when the stream is removed from `started`
            let finished_task = fasync::Task::spawn(Self::handle_stream_finished(
                finish_result,
                weak,
                local_id.clone(),
            ));
            let _ = peer.lock().waiting_start_tasks.insert(local_id, finished_task);
        });
        Self { _permit_task: permit_task }
    }

    async fn handle_stream_finished(
        finish_result: Result<MediaTaskStatus, anyhow::Error>,
        weak: Weak<Mutex<PeerInner>>,
        local_id: StreamEndpointId,
    ) {
        let is_audio_disabled = matches!(finish_result, Ok(MediaTaskStatus::AudioDisabled));
        let is_stopped = matches!(finish_result, Ok(MediaTaskStatus::Stopped));

        let stream_id = local_id.clone();
        info!("Audio stopped {finish_result:?} - suspending A2DP stream for {stream_id:?}");
        if !is_stopped {
            if let Err(e) = PeerInner::suspend(weak.clone(), stream_id.clone()).await {
                warn!("Error suspending stream after audio stopped: {e:?}");
            }
        }
        // Forget (detached) the current task before maybe scheduling a new start.
        PeerInner::forget_waiting_start(&weak, &local_id);
        if is_audio_disabled {
            PeerInner::schedule_start(weak, stream_id);
        }
    }
}

fn codectype_to_availability_metric(
    codec_type: &MediaCodecType,
) -> bt_metrics::A2dpCodecAvailabilityMigratedMetricDimensionCodec {
    match codec_type {
        &MediaCodecType::AUDIO_SBC => {
            bt_metrics::A2dpCodecAvailabilityMigratedMetricDimensionCodec::Sbc
        }
        &MediaCodecType::AUDIO_MPEG12 => {
            bt_metrics::A2dpCodecAvailabilityMigratedMetricDimensionCodec::Mpeg12
        }
        &MediaCodecType::AUDIO_AAC => {
            bt_metrics::A2dpCodecAvailabilityMigratedMetricDimensionCodec::Aac
        }
        &MediaCodecType::AUDIO_ATRAC => {
            bt_metrics::A2dpCodecAvailabilityMigratedMetricDimensionCodec::Atrac
        }
        &MediaCodecType::AUDIO_NON_A2DP => {
            bt_metrics::A2dpCodecAvailabilityMigratedMetricDimensionCodec::VendorSpecific
        }
        _ => bt_metrics::A2dpCodecAvailabilityMigratedMetricDimensionCodec::Unknown,
    }
}

fn capability_to_metric(
    cap: &ServiceCapability,
) -> Option<bt_metrics::A2dpRemotePeerCapabilitiesMetricDimensionCapability> {
    match cap {
        ServiceCapability::DelayReporting => {
            Some(bt_metrics::A2dpRemotePeerCapabilitiesMetricDimensionCapability::DelayReport)
        }
        ServiceCapability::Reporting => {
            Some(bt_metrics::A2dpRemotePeerCapabilitiesMetricDimensionCapability::Reporting)
        }
        ServiceCapability::Recovery { .. } => {
            Some(bt_metrics::A2dpRemotePeerCapabilitiesMetricDimensionCapability::Recovery)
        }
        ServiceCapability::ContentProtection { .. } => {
            Some(bt_metrics::A2dpRemotePeerCapabilitiesMetricDimensionCapability::ContentProtection)
        }
        ServiceCapability::HeaderCompression { .. } => {
            Some(bt_metrics::A2dpRemotePeerCapabilitiesMetricDimensionCapability::HeaderCompression)
        }
        ServiceCapability::Multiplexing { .. } => {
            Some(bt_metrics::A2dpRemotePeerCapabilitiesMetricDimensionCapability::Multiplexing)
        }
        // We ignore capabilities that we don't care to track.
        other => {
            trace!("untracked remote peer capability: {:?}", other);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use async_utils::PollExt;
    use bt_channel_test_support::{Transport, create_test_channels};
    use bt_metrics::respond_to_metrics_req_for_test;
    use fidl::endpoints::create_proxy_and_stream;
    use fidl_fuchsia_bluetooth::ErrorCode;

    use fidl_fuchsia_bluetooth_bredr::{
        ProfileMarker, ProfileRequest, ProfileRequestStream, ServiceClassProfileIdentifier,
    };
    use fidl_fuchsia_metrics::{MetricEvent, MetricEventPayload};
    use futures::{SinkExt, StreamExt};
    use std::pin::pin;
    use test_case::test_case;

    use crate::media_task::tests::{TestMediaTask, TestMediaTaskBuilder};
    use crate::media_types::*;
    use crate::stream::tests::{make_sbc_endpoint, sbc_mediacodec_capability};

    fn fake_metrics()
    -> (bt_metrics::MetricsLogger, fidl_fuchsia_metrics::MetricEventLoggerRequestStream) {
        let (c, s) = fidl::endpoints::create_proxy_and_stream::<
            fidl_fuchsia_metrics::MetricEventLoggerMarker,
        >();
        (bt_metrics::MetricsLogger::from_proxy(c), s)
    }

    fn setup_avdtp_peer(transport: Transport) -> (avdtp::Peer, Channel) {
        let (signaling, remote) = create_test_channels(transport);
        let peer = avdtp::Peer::new(signaling);
        (peer, remote)
    }

    fn build_test_streams() -> Streams {
        let mut streams = Streams::default();
        let source = Stream::build(
            make_sbc_endpoint(1, avdtp::EndpointType::Source),
            TestMediaTaskBuilder::new_delayable().builder(),
        );
        streams.insert(source);
        let sink = Stream::build(
            make_sbc_endpoint(2, avdtp::EndpointType::Sink),
            TestMediaTaskBuilder::new().builder(),
        );
        streams.insert(sink);
        streams
    }

    fn build_test_streams_delayable() -> Streams {
        fn with_delay(seid: u8, direction: avdtp::EndpointType) -> StreamEndpoint {
            StreamEndpoint::new(
                seid,
                avdtp::MediaType::Audio,
                direction,
                vec![
                    avdtp::ServiceCapability::MediaTransport,
                    avdtp::ServiceCapability::DelayReporting,
                    sbc_mediacodec_capability(),
                ],
            )
            .expect("endpoint creation should succeed")
        }
        let mut streams = Streams::default();
        let source = Stream::build(
            with_delay(1, avdtp::EndpointType::Source),
            TestMediaTaskBuilder::new_delayable().builder(),
        );
        streams.insert(source);
        let sink = Stream::build(
            with_delay(2, avdtp::EndpointType::Sink),
            TestMediaTaskBuilder::new().builder(),
        );
        streams.insert(sink);
        streams
    }

    #[track_caller]
    pub(crate) fn recv_remote(
        exec: &mut fasync::TestExecutor,
        remote: &mut Channel,
    ) -> Result<Vec<u8>, zx::Status> {
        let mut fut = remote.next();
        match exec.run_until_stalled(&mut fut) {
            Poll::Ready(Some(res)) => res,
            Poll::Ready(None) => Err(zx::Status::PEER_CLOSED),
            Poll::Pending => Err(zx::Status::SHOULD_WAIT),
        }
    }

    /// Creates a Peer object, returning a channel connected ot the remote end, a
    /// ProfileRequestStream connected to the profile_proxy, and the Peer object.
    fn setup_test_peer(
        transport: Transport,
        use_cobalt: bool,
        streams: Streams,
        permits: Option<Permits>,
    ) -> (
        Channel,
        ProfileRequestStream,
        Option<fidl_fuchsia_metrics::MetricEventLoggerRequestStream>,
        Peer,
    ) {
        let (avdtp, remote) = setup_avdtp_peer(transport);
        let (metrics_logger, cobalt_receiver) = if use_cobalt {
            let (l, r) = fake_metrics();
            (l, Some(r))
        } else {
            (bt_metrics::MetricsLogger::default(), None)
        };
        let (profile_proxy, requests) = create_proxy_and_stream::<ProfileMarker>();
        let peer =
            Peer::create(PeerId(1), avdtp, streams, permits, profile_proxy, None, metrics_logger);

        (remote, requests, cobalt_receiver, peer)
    }

    #[track_caller]
    fn expect_send(exec: &mut fasync::TestExecutor, remote: &mut Channel, data: Vec<u8>) {
        exec.run_until_stalled(&mut remote.send(data))
            .expect("poll is ready")
            .expect("write successful");
    }

    fn expect_get_capabilities_and_respond(
        exec: &mut fasync::TestExecutor,
        remote: &mut Channel,
        expected_seid: u8,
        response_capabilities: &[u8],
    ) {
        let received = recv_remote(exec, remote).unwrap();
        // Last half of header must be Single (0b00) and Command (0b00)
        assert_eq!(0x00, received[0] & 0xF);
        assert_eq!(0x02, received[1]); // 0x02 = Get Capabilities
        assert_eq!(expected_seid << 2, received[2]);

        let txlabel_raw = received[0] & 0xF0;

        // Expect a get capabilities and respond.
        #[rustfmt::skip]
        let mut get_capabilities_rsp = vec![
            txlabel_raw << 4 | 0x2, // TxLabel (same) + ResponseAccept (0x02)
            0x02 // GetCapabilities
        ];

        get_capabilities_rsp.extend_from_slice(response_capabilities);

        expect_send(exec, remote, get_capabilities_rsp);
    }

    fn expect_get_all_capabilities_and_respond(
        exec: &mut fasync::TestExecutor,
        remote: &mut Channel,
        expected_seid: u8,
        response_capabilities: &[u8],
    ) {
        let received = recv_remote(exec, remote).unwrap();
        // Last half of header must be Single (0b00) and Command (0b00)
        assert_eq!(0x00, received[0] & 0xF);
        assert_eq!(0x0C, received[1]); // 0x0C = Get All Capabilities
        assert_eq!(expected_seid << 2, received[2]);

        let txlabel_raw = received[0] & 0xF0;

        // Expect a get capabilities and respond.
        #[rustfmt::skip]
        let mut get_capabilities_rsp = vec![
            txlabel_raw << 4 | 0x2, // TxLabel (same) + ResponseAccept (0x02)
            0x0C // GetAllCapabilities
        ];

        get_capabilities_rsp.extend_from_slice(response_capabilities);

        expect_send(exec, remote, get_capabilities_rsp);
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    fn disconnected(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (proxy, _stream) = create_proxy_and_stream::<ProfileMarker>();
        let (signaling, remote) = create_test_channels(transport);

        let id = PeerId(1);

        let avdtp = avdtp::Peer::new(signaling);
        let peer = Peer::create(
            id,
            avdtp,
            Streams::default(),
            None,
            proxy,
            None,
            bt_metrics::MetricsLogger::default(),
        );

        let closed_fut = peer.closed();

        let mut closed_fut = pin!(closed_fut);

        assert!(exec.run_until_stalled(&mut closed_fut).is_pending());

        // Close the remote channel
        drop(remote);

        assert!(exec.run_until_stalled(&mut closed_fut).is_ready());
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    fn peer_collect_capabilities_success(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();

        let (mut remote, _, cobalt_receiver, peer) =
            setup_test_peer(transport, true, build_test_streams(), None);

        let p: ProfileDescriptor = ProfileDescriptor {
            profile_id: Some(ServiceClassProfileIdentifier::AdvancedAudioDistribution),
            major_version: Some(1),
            minor_version: Some(2),
            ..Default::default()
        };
        let _ = peer.set_descriptor(p);

        let collect_future = peer.collect_capabilities();
        let mut collect_future = pin!(collect_future);

        assert!(exec.run_until_stalled(&mut collect_future).is_pending());

        // Expect a discover command.
        let received = recv_remote(&mut exec, &mut remote).unwrap();
        // Last half of header must be Single (0b00) and Command (0b00)
        assert_eq!(0x00, received[0] & 0xF);
        assert_eq!(0x01, received[1]); // 0x01 = Discover

        let txlabel_raw = received[0] & 0xF0;

        // Respond with a set of streams.
        let response: &[u8] = &[
            txlabel_raw << 4 | 0x0 << 2 | 0x2, // txlabel (same), Single (0b00), Response Accept (0b10)
            0x01,                              // Discover
            0x3E << 2 | 0x0 << 1,              // SEID (3E), Not In Use (0b0)
            0x00 << 4 | 0x1 << 3,              // Audio (0x00), Sink (0x01)
            0x01 << 2 | 0x1 << 1,              // SEID (1), In Use (0b1)
            0x00 << 4 | 0x1 << 3,              // Audio (0x00), Sink (0x01)
        ];
        expect_send(&mut exec, &mut remote, response.to_vec());

        assert!(exec.run_until_stalled(&mut collect_future).is_pending());

        // Expect a get capabilities and respond.
        #[rustfmt::skip]
        let capabilities_rsp = &[
            // MediaTransport (Length of Service Capability = 0)
            0x01, 0x00,
            // Media Codec (LOSC = 2 + 4), Media Type Audio (0x00), Codec type (0x04), Codec specific 0xF09F9296
            0x07, 0x06, 0x00, 0x04, 0xF0, 0x9F, 0x92, 0x96
        ];
        expect_get_capabilities_and_respond(&mut exec, &mut remote, 0x3E, capabilities_rsp);

        assert!(exec.run_until_stalled(&mut collect_future).is_pending());

        // Expect a get capabilities and respond.
        #[rustfmt::skip]
        let capabilities_rsp = &[
            // MediaTransport (Length of Service Capability = 0)
            0x01, 0x00,
            // Media Codec (LOSC = 2 + 2), Media Type Audio (0x00), Codec type (0x00), Codec specific 0xC0DE
            0x07, 0x04, 0x00, 0x00, 0xC0, 0xDE
        ];
        expect_get_capabilities_and_respond(&mut exec, &mut remote, 0x01, capabilities_rsp);

        match exec.run_until_stalled(&mut collect_future) {
            Poll::Pending => panic!("collect capabilities should be complete"),
            Poll::Ready(Err(e)) => panic!("collect capabilities should have succeeded: {}", e),
            Poll::Ready(Ok(endpoints)) => {
                let first_seid: StreamEndpointId = 0x3E_u8.try_into().unwrap();
                let second_seid: StreamEndpointId = 0x01_u8.try_into().unwrap();
                for stream in endpoints {
                    if stream.local_id() == &first_seid {
                        let expected_caps = vec![
                            ServiceCapability::MediaTransport,
                            ServiceCapability::MediaCodec {
                                media_type: avdtp::MediaType::Audio,
                                codec_type: avdtp::MediaCodecType::new(0x04),
                                codec_extra: vec![0xF0, 0x9F, 0x92, 0x96],
                            },
                        ];
                        assert_eq!(&expected_caps, stream.capabilities());
                    } else if stream.local_id() == &second_seid {
                        let expected_codec_type = avdtp::MediaCodecType::new(0x00);
                        assert_eq!(Some(&expected_codec_type), stream.codec_type());
                    } else {
                        panic!("Unexpected endpoint in the streams collected");
                    }
                }
            }
        }

        // Collect reported cobalt logs.
        let mut recv = cobalt_receiver.expect("should have receiver");
        let mut log_events = Vec::new();
        while let Poll::Ready(Some(Ok(req))) = exec.run_until_stalled(&mut recv.next()) {
            log_events.push(respond_to_metrics_req_for_test(req));
        }

        // Should have sent two metric events for codec and one for capability.
        assert_eq!(3, log_events.len());
        assert!(log_events.contains(&MetricEvent {
            metric_id: bt_metrics::A2DP_CODEC_AVAILABILITY_MIGRATED_METRIC_ID,
            event_codes: vec![
                bt_metrics::A2dpCodecAvailabilityMigratedMetricDimensionCodec::Sbc as u32
            ],
            payload: MetricEventPayload::Count(1),
        }));
        assert!(log_events.contains(&MetricEvent {
            metric_id: bt_metrics::A2DP_CODEC_AVAILABILITY_MIGRATED_METRIC_ID,
            event_codes: vec![
                bt_metrics::A2dpCodecAvailabilityMigratedMetricDimensionCodec::Atrac as u32
            ],
            payload: MetricEventPayload::Count(1),
        }));
        assert!(log_events.contains(&MetricEvent {
            metric_id: bt_metrics::A2DP_REMOTE_PEER_CAPABILITIES_METRIC_ID,
            event_codes: vec![
                bt_metrics::A2dpRemotePeerCapabilitiesMetricDimensionCapability::Basic as u32
            ],
            payload: MetricEventPayload::Count(1),
        }));

        // The second time, we don't expect to ask the peer again.
        let collect_future = peer.collect_capabilities();
        let mut collect_future = pin!(collect_future);

        match exec.run_until_stalled(&mut collect_future) {
            Poll::Ready(Ok(endpoints)) => assert_eq!(2, endpoints.len()),
            x => panic!("Expected get remote capabilities to be done, got {:?}", x),
        };
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    fn peer_collect_all_capabilities_success(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();

        let (mut remote, _, cobalt_receiver, peer) =
            setup_test_peer(transport, true, build_test_streams(), None);
        let p: ProfileDescriptor = ProfileDescriptor {
            profile_id: Some(ServiceClassProfileIdentifier::AdvancedAudioDistribution),
            major_version: Some(1),
            minor_version: Some(3),
            ..Default::default()
        };
        let _ = peer.set_descriptor(p);

        let collect_future = peer.collect_capabilities();
        let mut collect_future = pin!(collect_future);

        assert!(exec.run_until_stalled(&mut collect_future).is_pending());

        // Expect a discover command.
        let received = recv_remote(&mut exec, &mut remote).unwrap();
        // Last half of header must be Single (0b00) and Command (0b00)
        assert_eq!(0x00, received[0] & 0xF);
        assert_eq!(0x01, received[1]); // 0x01 = Discover

        let txlabel_raw = received[0] & 0xF0;

        // Respond with a set of streams.
        let response: &[u8] = &[
            txlabel_raw << 4 | 0x0 << 2 | 0x2, // txlabel (same), Single (0b00), Response Accept (0b10)
            0x01,                              // Discover
            0x3E << 2 | 0x0 << 1,              // SEID (3E), Not In Use (0b0)
            0x00 << 4 | 0x1 << 3,              // Audio (0x00), Sink (0x01)
            0x01 << 2 | 0x1 << 1,              // SEID (1), In Use (0b1)
            0x00 << 4 | 0x1 << 3,              // Audio (0x00), Sink (0x01)
        ];
        expect_send(&mut exec, &mut remote, response.to_vec());

        assert!(exec.run_until_stalled(&mut collect_future).is_pending());

        // Expect a get all capabilities and respond.
        #[rustfmt::skip]
        let capabilities_rsp = &[
            // MediaTransport (Length of Service Capability = 0)
            0x01, 0x00,
            // Media Codec (LOSC = 2 + 4), Media Type Audio (0x00), Codec type (0x40), Codec specific 0xF09F9296
            0x07, 0x06, 0x00, 0x40, 0xF0, 0x9F, 0x92, 0x96,
            // Delay Reporting (LOSC = 0)
            0x08, 0x00
        ];
        expect_get_all_capabilities_and_respond(&mut exec, &mut remote, 0x3E, capabilities_rsp);

        assert!(exec.run_until_stalled(&mut collect_future).is_pending());

        // Expect a get all capabilities and respond.
        #[rustfmt::skip]
        let capabilities_rsp = &[
            // MediaTransport (Length of Service Capability = 0)
            0x01, 0x00,
            // Media Codec (LOSC = 2 + 2), Media Type Audio (0x00), Codec type (0x00), Codec specific 0xC0DE
            0x07, 0x04, 0x00, 0x00, 0xC0, 0xDE
        ];
        expect_get_all_capabilities_and_respond(&mut exec, &mut remote, 0x01, capabilities_rsp);

        match exec.run_until_stalled(&mut collect_future) {
            Poll::Pending => panic!("collect capabilities should be complete"),
            Poll::Ready(Err(e)) => panic!("collect capabilities should have succeeded: {}", e),
            Poll::Ready(Ok(endpoints)) => {
                let first_seid: StreamEndpointId = 0x3E_u8.try_into().unwrap();
                let second_seid: StreamEndpointId = 0x01_u8.try_into().unwrap();
                for stream in endpoints {
                    if stream.local_id() == &first_seid {
                        let expected_caps = vec![
                            ServiceCapability::MediaTransport,
                            ServiceCapability::MediaCodec {
                                media_type: avdtp::MediaType::Audio,
                                codec_type: avdtp::MediaCodecType::new(0x40),
                                codec_extra: vec![0xF0, 0x9F, 0x92, 0x96],
                            },
                            ServiceCapability::DelayReporting,
                        ];
                        assert_eq!(&expected_caps, stream.capabilities());
                    } else if stream.local_id() == &second_seid {
                        let expected_codec_type = avdtp::MediaCodecType::new(0x00);
                        assert_eq!(Some(&expected_codec_type), stream.codec_type());
                    } else {
                        panic!("Unexpected endpoint in the streams collected");
                    }
                }
            }
        }

        // Collect reported cobalt logs.
        let mut recv = cobalt_receiver.expect("should have receiver");
        let mut log_events = Vec::new();
        while let Poll::Ready(Some(Ok(req))) = exec.run_until_stalled(&mut recv.next()) {
            log_events.push(respond_to_metrics_req_for_test(req));
        }

        // Should have sent two metric events for codec and two for capability.
        assert_eq!(4, log_events.len());
        assert!(log_events.contains(&MetricEvent {
            metric_id: bt_metrics::A2DP_CODEC_AVAILABILITY_MIGRATED_METRIC_ID,
            event_codes: vec![
                bt_metrics::A2dpCodecAvailabilityMigratedMetricDimensionCodec::Unknown as u32
            ],
            payload: MetricEventPayload::Count(1),
        }));
        assert!(log_events.contains(&MetricEvent {
            metric_id: bt_metrics::A2DP_CODEC_AVAILABILITY_MIGRATED_METRIC_ID,
            event_codes: vec![
                bt_metrics::A2dpCodecAvailabilityMigratedMetricDimensionCodec::Sbc as u32
            ],
            payload: MetricEventPayload::Count(1),
        }));
        assert!(log_events.contains(&MetricEvent {
            metric_id: bt_metrics::A2DP_REMOTE_PEER_CAPABILITIES_METRIC_ID,
            event_codes: vec![
                bt_metrics::A2dpRemotePeerCapabilitiesMetricDimensionCapability::Basic as u32
            ],
            payload: MetricEventPayload::Count(1),
        }));
        assert!(log_events.contains(&MetricEvent {
            metric_id: bt_metrics::A2DP_REMOTE_PEER_CAPABILITIES_METRIC_ID,
            event_codes: vec![
                bt_metrics::A2dpRemotePeerCapabilitiesMetricDimensionCapability::DelayReport as u32
            ],
            payload: MetricEventPayload::Count(1),
        }));

        // The second time, we don't expect to ask the peer again.
        let collect_future = peer.collect_capabilities();
        let mut collect_future = pin!(collect_future);

        match exec.run_until_stalled(&mut collect_future) {
            Poll::Ready(Ok(endpoints)) => assert_eq!(2, endpoints.len()),
            x => panic!("Expected get remote capabilities to be done, got {:?}", x),
        };
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    fn peer_collect_capabilities_discovery_fails(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();

        let (mut remote, _, _, peer) =
            setup_test_peer(transport, false, build_test_streams(), None);

        let collect_future = peer.collect_capabilities();
        let mut collect_future = pin!(collect_future);

        // Shouldn't finish yet.
        assert!(exec.run_until_stalled(&mut collect_future).is_pending());

        // Expect a discover command.
        let received = recv_remote(&mut exec, &mut remote).unwrap();
        // Last half of header must be Single (0b00) and Command (0b00)
        assert_eq!(0x00, received[0] & 0xF);
        assert_eq!(0x01, received[1]); // 0x01 = Discover

        let txlabel_raw = received[0] & 0xF0;

        // Respond with an error.
        let response: &[u8] = &[
            txlabel_raw | 0x0 << 2 | 0x3, // txlabel (same), Single (0b00), Response Reject (0b11)
            0x01,                         // Discover
            0x31,                         // BAD_STATE
        ];
        expect_send(&mut exec, &mut remote, response.to_vec());

        // Should be done with an error.
        // Should finish!
        match exec.run_until_stalled(&mut collect_future) {
            Poll::Pending => panic!("Should be ready after discovery failure"),
            Poll::Ready(Ok(x)) => panic!("Should be an error but returned {x:?}"),
            Poll::Ready(Err(avdtp::Error::RemoteRejected(e))) => {
                assert_eq!(Some(Ok(avdtp::ErrorCode::BadState)), e.error_code());
            }
            Poll::Ready(Err(e)) => panic!("Should have been a RemoteRejected was was {e:?}"),
        }
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    fn peer_collect_capabilities_get_capability_fails(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();

        let (mut remote, _, _, peer) = setup_test_peer(transport, true, build_test_streams(), None);

        let collect_future = peer.collect_capabilities();
        let mut collect_future = pin!(collect_future);

        // Shouldn't finish yet.
        assert!(exec.run_until_stalled(&mut collect_future).is_pending());

        // Expect a discover command.
        let received = recv_remote(&mut exec, &mut remote).unwrap();
        // Last half of header must be Single (0b00) and Command (0b00)
        assert_eq!(0x00, received[0] & 0xF);
        assert_eq!(0x01, received[1]); // 0x01 = Discover

        let txlabel_raw = received[0] & 0xF0;

        // Respond with a set of streams.
        let response: &[u8] = &[
            txlabel_raw << 4 | 0x0 << 2 | 0x2, // txlabel (same), Single (0b00), Response Accept (0b10)
            0x01,                              // Discover
            0x3E << 2 | 0x0 << 1,              // SEID (3E), Not In Use (0b0)
            0x00 << 4 | 0x1 << 3,              // Audio (0x00), Sink (0x01)
            0x01 << 2 | 0x1 << 1,              // SEID (1), In Use (0b1)
            0x00 << 4 | 0x1 << 3,              // Audio (0x00), Sink (0x01)
        ];
        expect_send(&mut exec, &mut remote, response.to_vec());

        assert!(exec.run_until_stalled(&mut collect_future).is_pending());

        // Expect a get capabilities request
        let expected_seid = 0x3E;
        let received = recv_remote(&mut exec, &mut remote).unwrap();
        // Last half of header must be Single (0b00) and Command (0b00)
        assert_eq!(0x00, received[0] & 0xF);
        assert_eq!(0x02, received[1]); // 0x02 = Get Capabilities
        assert_eq!(expected_seid << 2, received[2]);

        let txlabel_raw = received[0] & 0xF0;

        let response: &[u8] = &[
            txlabel_raw | 0x0 << 2 | 0x3, // txlabel (same), Single (0b00), Response Reject (0b11)
            0x02,                         // Get Capabilities
            0x12,                         // BAD_ACP_SEID
        ];
        expect_send(&mut exec, &mut remote, response.to_vec());

        assert!(exec.run_until_stalled(&mut collect_future).is_pending());

        // Expect a get capabilities request (skipped the last one)
        let expected_seid = 0x01;
        let received = recv_remote(&mut exec, &mut remote).unwrap();
        // Last half of header must be Single (0b00) and Command (0b00)
        assert_eq!(0x00, received[0] & 0xF);
        assert_eq!(0x02, received[1]); // 0x02 = Get Capabilities
        assert_eq!(expected_seid << 2, received[2]);

        let txlabel_raw = received[0] & 0xF0;

        let response: &[u8] = &[
            txlabel_raw | 0x0 << 2 | 0x3, // txlabel (same), Single (0b00), Response Reject (0b11)
            0x02,                         // Get Capabilities
            0x12,                         // BAD_ACP_SEID
        ];
        expect_send(&mut exec, &mut remote, response.to_vec());

        // Should be done without an error, but with no streams.
        match exec.run_until_stalled(&mut collect_future) {
            Poll::Pending => panic!("Should be ready after discovery failure"),
            Poll::Ready(Err(e)) => panic!("Shouldn't be an error but returned {:?}", e),
            Poll::Ready(Ok(map)) => assert_eq!(0, map.len()),
        }
    }

    fn receive_simple_accept(exec: &mut fasync::TestExecutor, remote: &mut Channel, signal_id: u8) {
        let received = recv_remote(exec, remote).expect("expected a packet");
        // Last half of header must be Single (0b00) and Command (0b00)
        assert_eq!(0x00, received[0] & 0xF);
        assert_eq!(signal_id, received[1]);

        let txlabel_raw = received[0] & 0xF0;

        let response: &[u8] = &[
            txlabel_raw | 0x0 << 2 | 0x2, // txlabel (same), Single (0b00), Response Accept (0b10)
            signal_id,
        ];
        expect_send(exec, remote, response.to_vec());
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    fn peer_stream_start_success(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();

        let (mut remote, mut profile_request_stream, _, peer) =
            setup_test_peer(transport, false, build_test_streams(), None);

        let remote_seid = 2_u8.try_into().unwrap();

        let codec_params = ServiceCapability::MediaCodec {
            media_type: avdtp::MediaType::Audio,
            codec_type: avdtp::MediaCodecType::AUDIO_SBC,
            codec_extra: vec![0x11, 0x45, 51, 51],
        };

        // Set the remote endpoint so the compatible local source endpoint is selected.
        let remote_endpoint = avdtp::StreamEndpoint::new(
            2,
            avdtp::MediaType::Audio,
            avdtp::EndpointType::Sink,
            vec![codec_params.clone()],
        )
        .expect("valid endpoint");
        peer.inner.lock().set_remote_endpoints(&[remote_endpoint]);

        let start_future = peer.stream_start(remote_seid, vec![codec_params]);
        let mut start_future = pin!(start_future);

        match exec.run_until_stalled(&mut start_future) {
            Poll::Pending => {}
            x => panic!("Expected pending, but got {x:?}"),
        };

        receive_simple_accept(&mut exec, &mut remote, 0x03); // Set Configuration

        assert!(exec.run_until_stalled(&mut start_future).is_pending());

        receive_simple_accept(&mut exec, &mut remote, 0x06); // Open

        match exec.run_until_stalled(&mut start_future) {
            Poll::Pending => {}
            Poll::Ready(Err(e)) => panic!("Expected to be pending but error: {:?}", e),
            Poll::Ready(Ok(_)) => panic!("Expected to be pending but finished!"),
        };

        // Should connect the media channel after open.
        let (transport_chan, _remote_chan) = create_test_channels(transport);

        let request = exec.run_until_stalled(&mut profile_request_stream.next());
        match request {
            Poll::Ready(Some(Ok(ProfileRequest::Connect { peer_id, connection, responder }))) => {
                assert_eq!(PeerId(1), peer_id.into());
                assert_eq!(connection, ConnectParameters::L2cap(Peer::transport_channel_params()));
                let channel = transport_chan.try_into().unwrap();
                responder.send(Ok(channel)).expect("responder sends");
            }
            x => panic!("Should have sent a open l2cap request, but got {:?}", x),
        };

        // Setup is complete once the media transport is connected. The start is sent by the task
        // that starts the stream when the audio is active.
        exec.run_until_stalled(&mut start_future)
            .expect("start setup finished")
            .expect("stream setup is ok");

        receive_simple_accept(&mut exec, &mut remote, 0x07); // Start

        // The stream should be started, with the media stream connected.
        // TODO: confirm the stream is usable
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());
        assert!(peer.is_streaming_now());
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    fn peer_stream_start_picks_correct_direction(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();

        let (remote, _, _, peer) = setup_test_peer(transport, false, build_test_streams(), None);
        let remote = avdtp::Peer::new(remote);
        let mut remote_events = remote.take_request_stream();

        // Respond as if we have a single SBC Source Stream
        fn remote_handle_request(req: avdtp::Request) {
            let expected_stream_id: StreamEndpointId = 4_u8.try_into().unwrap();
            let res = match req {
                avdtp::Request::Discover { responder } => {
                    let infos = [avdtp::StreamInformation::new(
                        expected_stream_id,
                        false,
                        avdtp::MediaType::Audio,
                        avdtp::EndpointType::Source,
                    )];
                    responder.send(&infos)
                }
                avdtp::Request::GetAllCapabilities { stream_id, responder }
                | avdtp::Request::GetCapabilities { stream_id, responder } => {
                    assert_eq!(expected_stream_id, stream_id);
                    let caps = vec![
                        ServiceCapability::MediaTransport,
                        ServiceCapability::MediaCodec {
                            media_type: avdtp::MediaType::Audio,
                            codec_type: avdtp::MediaCodecType::AUDIO_SBC,
                            codec_extra: vec![0x11, 0x45, 51, 250],
                        },
                    ];
                    responder.send(&caps[..])
                }
                avdtp::Request::Open { responder, stream_id } => {
                    assert_eq!(expected_stream_id, stream_id);
                    responder.send()
                }
                avdtp::Request::SetConfiguration {
                    responder,
                    local_stream_id,
                    remote_stream_id,
                    ..
                } => {
                    assert_eq!(local_stream_id, expected_stream_id);
                    // This is the "sink" local stream id.
                    assert_eq!(remote_stream_id, 2_u8.try_into().unwrap());
                    responder.send()
                }
                x => panic!("Unexpected request: {:?}", x),
            };
            res.expect("should be able to respond");
        }

        // Need to discover the remote streams first, or the stream start will not work.
        let collect_capabilities_fut = peer.collect_capabilities();
        let mut collect_capabilities_fut = pin!(collect_capabilities_fut);

        assert!(exec.run_until_stalled(&mut collect_capabilities_fut).is_pending());

        let request = exec.run_singlethreaded(&mut remote_events.next());
        remote_handle_request(request.expect("should have a discovery request").unwrap());

        assert!(exec.run_until_stalled(&mut collect_capabilities_fut).is_pending());
        let request = exec.run_singlethreaded(&mut remote_events.next());
        remote_handle_request(request.expect("should have a get_capabilities request").unwrap());

        assert!(exec.run_until_stalled(&mut collect_capabilities_fut).is_ready());

        // Try to start the stream.  It should continue to configure and connect.
        let remote_seid = 4_u8.try_into().unwrap();

        let codec_params = ServiceCapability::MediaCodec {
            media_type: avdtp::MediaType::Audio,
            codec_type: avdtp::MediaCodecType::AUDIO_SBC,
            codec_extra: vec![0x11, 0x45, 51, 51],
        };
        let start_future = peer.stream_start(remote_seid, vec![codec_params]);
        let mut start_future = pin!(start_future);

        assert!(exec.run_until_stalled(&mut start_future).is_pending());
        let request = exec.run_singlethreaded(&mut remote_events.next());
        remote_handle_request(request.expect("should have a set_capabilities request").unwrap());

        assert!(exec.run_until_stalled(&mut start_future).is_pending());
        let request = exec.run_singlethreaded(&mut remote_events.next());
        remote_handle_request(request.expect("should have an open request").unwrap());
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    fn peer_stream_start_strips_unsupported_local_capabilities(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();

        let (remote, _, _, peer) = setup_test_peer(transport, false, build_test_streams(), None);
        let remote = avdtp::Peer::new(remote);
        let mut remote_events = remote.take_request_stream();

        // Respond as if we have a single SBC Source Stream
        fn remote_handle_request(req: avdtp::Request) {
            let expected_stream_id: StreamEndpointId = 4_u8.try_into().unwrap();
            let res = match req {
                avdtp::Request::Discover { responder } => {
                    let infos = [avdtp::StreamInformation::new(
                        expected_stream_id,
                        false,
                        avdtp::MediaType::Audio,
                        avdtp::EndpointType::Source,
                    )];
                    responder.send(&infos)
                }
                avdtp::Request::GetAllCapabilities { stream_id, responder }
                | avdtp::Request::GetCapabilities { stream_id, responder } => {
                    assert_eq!(expected_stream_id, stream_id);
                    let caps = vec![
                        ServiceCapability::MediaTransport,
                        // We don't have a local delay-reporting, so this shouldn't be requested.
                        ServiceCapability::DelayReporting,
                        ServiceCapability::MediaCodec {
                            media_type: avdtp::MediaType::Audio,
                            codec_type: avdtp::MediaCodecType::AUDIO_AAC,
                            codec_extra: vec![128, 0, 132, 134, 0, 0],
                        },
                    ];
                    responder.send(&caps[..])
                }
                avdtp::Request::Open { responder, stream_id } => {
                    assert_eq!(expected_stream_id, stream_id);
                    responder.send()
                }
                avdtp::Request::SetConfiguration {
                    responder,
                    local_stream_id,
                    remote_stream_id,
                    capabilities,
                } => {
                    assert_eq!(local_stream_id, expected_stream_id);
                    // This is the "sink" local stream id.
                    assert_eq!(remote_stream_id, 2_u8.try_into().unwrap());
                    // Make sure we didn't request a DelayReport since the local Sink doesn't
                    // support it.
                    assert!(!capabilities.contains(&ServiceCapability::DelayReporting));
                    responder.send()
                }
                x => panic!("Unexpected request: {:?}", x),
            };
            res.expect("should be able to respond");
        }

        // Need to discover the remote streams first, or the stream start will not work.
        let collect_capabilities_fut = peer.collect_capabilities();
        let mut collect_capabilities_fut = pin!(collect_capabilities_fut);

        assert!(exec.run_until_stalled(&mut collect_capabilities_fut).is_pending());

        let request = exec.run_singlethreaded(&mut remote_events.next());
        remote_handle_request(request.expect("should have a discovery request").unwrap());

        assert!(exec.run_until_stalled(&mut collect_capabilities_fut).is_pending());
        let request = exec.run_singlethreaded(&mut remote_events.next());
        remote_handle_request(request.expect("should have a get_capabilities request").unwrap());

        assert!(exec.run_until_stalled(&mut collect_capabilities_fut).is_ready());

        // Try to start the stream.  It should continue to configure and connect.
        let remote_seid = 4_u8.try_into().unwrap();

        let codec_params = ServiceCapability::MediaCodec {
            media_type: avdtp::MediaType::Audio,
            codec_type: avdtp::MediaCodecType::AUDIO_SBC,
            codec_extra: vec![0x11, 0x45, 51, 51],
        };
        let start_future =
            peer.stream_start(remote_seid, vec![codec_params, ServiceCapability::DelayReporting]);
        let mut start_future = pin!(start_future);

        assert!(exec.run_until_stalled(&mut start_future).is_pending());
        let request = exec.run_singlethreaded(&mut remote_events.next());
        remote_handle_request(request.expect("should have a set_configuration request").unwrap());

        assert!(exec.run_until_stalled(&mut start_future).is_pending());
        let request = exec.run_singlethreaded(&mut remote_events.next());
        remote_handle_request(request.expect("should have an open request").unwrap());
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    fn peer_stream_start_orders_local_capabilities(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();

        let (remote, _, _, peer) =
            setup_test_peer(transport, false, build_test_streams_delayable(), None);
        let remote = avdtp::Peer::new(remote);
        let mut remote_events = remote.take_request_stream();

        // Respond as if we have a single SBC Source Stream
        fn remote_handle_request(req: avdtp::Request) {
            let expected_stream_id: StreamEndpointId = 4_u8.try_into().unwrap();
            let res = match req {
                avdtp::Request::Discover { responder } => {
                    let infos = [avdtp::StreamInformation::new(
                        expected_stream_id,
                        false,
                        avdtp::MediaType::Audio,
                        avdtp::EndpointType::Source,
                    )];
                    responder.send(&infos)
                }
                avdtp::Request::GetAllCapabilities { stream_id, responder }
                | avdtp::Request::GetCapabilities { stream_id, responder } => {
                    assert_eq!(expected_stream_id, stream_id);
                    let caps = &[
                        ServiceCapability::MediaTransport,
                        ServiceCapability::MediaCodec {
                            media_type: avdtp::MediaType::Audio,
                            codec_type: avdtp::MediaCodecType::AUDIO_SBC,
                            codec_extra: vec![0x11, 0x45, 51, 250],
                        },
                        ServiceCapability::DelayReporting,
                    ];
                    responder.send(caps)
                }
                avdtp::Request::Open { responder, stream_id } => {
                    assert_eq!(expected_stream_id, stream_id);
                    responder.send()
                }
                avdtp::Request::SetConfiguration {
                    responder,
                    local_stream_id,
                    remote_stream_id,
                    capabilities,
                } => {
                    assert_eq!(local_stream_id, expected_stream_id);
                    // This is the "sink" local stream id.
                    assert_eq!(remote_stream_id, 2_u8.try_into().unwrap());
                    // The capabilities should be in order.
                    let mut capabilities_ordered = capabilities.clone();
                    capabilities_ordered.sort_by_key(ServiceCapability::category);
                    assert_eq!(capabilities, capabilities_ordered);
                    responder.send()
                }
                x => panic!("Unexpected request: {:?}", x),
            };
            res.expect("should be able to respond");
        }

        // Need to discover the remote streams first, or the stream start will not work.
        let collect_capabilities_fut = peer.collect_capabilities();
        let mut collect_capabilities_fut = pin!(collect_capabilities_fut);

        assert!(exec.run_until_stalled(&mut collect_capabilities_fut).is_pending());

        let request = exec.run_singlethreaded(&mut remote_events.next());
        remote_handle_request(request.expect("should have a discovery request").unwrap());

        assert!(exec.run_until_stalled(&mut collect_capabilities_fut).is_pending());
        let request = exec.run_singlethreaded(&mut remote_events.next());
        remote_handle_request(request.expect("should have a get_capabilities request").unwrap());

        assert!(exec.run_until_stalled(&mut collect_capabilities_fut).is_ready());

        // Try to start the stream.  It should continue to configure and connect.
        let remote_seid = 4_u8.try_into().unwrap();

        let codec_params = ServiceCapability::MediaCodec {
            media_type: avdtp::MediaType::Audio,
            codec_type: avdtp::MediaCodecType::AUDIO_SBC,
            codec_extra: vec![0x11, 0x45, 51, 51],
        };
        let start_future = peer.stream_start(
            remote_seid,
            vec![
                ServiceCapability::MediaTransport,
                ServiceCapability::DelayReporting,
                codec_params,
            ],
        );
        let mut start_future = pin!(start_future);

        assert!(exec.run_until_stalled(&mut start_future).is_pending());
        let request = exec.run_singlethreaded(&mut remote_events.next());
        remote_handle_request(request.expect("should have a set_configuration request").unwrap());

        assert!(exec.run_until_stalled(&mut start_future).is_pending());
        let request = exec.run_singlethreaded(&mut remote_events.next());
        remote_handle_request(request.expect("should have an open request").unwrap());
    }

    /// Tests that A2DP streaming does not start if the streaming permit is revoked during streaming
    /// setup.
    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    fn peer_stream_start_permit_revoked(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();

        let test_permits = Permits::new(1);
        let (mut remote, mut profile_request_stream, _, peer) =
            setup_test_peer(transport, false, build_test_streams(), Some(test_permits.clone()));

        let remote_seid = 2_u8.try_into().unwrap();

        let codec_params = ServiceCapability::MediaCodec {
            media_type: avdtp::MediaType::Audio,
            codec_type: avdtp::MediaCodecType::AUDIO_SBC,
            codec_extra: vec![0x11, 0x45, 51, 51],
        };

        // Set the remote endpoint so the compatible local source endpoint is selected.
        let remote_endpoint = avdtp::StreamEndpoint::new(
            2,
            avdtp::MediaType::Audio,
            avdtp::EndpointType::Sink,
            vec![codec_params.clone()],
        )
        .expect("valid endpoint");
        peer.inner.lock().set_remote_endpoints(&[remote_endpoint]);

        let start_future = peer.stream_start(remote_seid, vec![codec_params]);
        let mut start_future = pin!(start_future);

        let _ = exec
            .run_until_stalled(&mut start_future)
            .expect_pending("waiting for set config response");
        receive_simple_accept(&mut exec, &mut remote, 0x03); // Set Configuration
        exec.run_until_stalled(&mut start_future).expect_pending("waiting for open response");
        receive_simple_accept(&mut exec, &mut remote, 0x06); // Open
        exec.run_until_stalled(&mut start_future).expect_pending("waiting for media transport");
        assert!(!peer.is_streaming_now());

        // Should connect the media channel after open.
        let (transport_chan, _remote_chan) = create_test_channels(transport);

        let request = exec.run_until_stalled(&mut profile_request_stream.next());
        match request {
            Poll::Ready(Some(Ok(ProfileRequest::Connect { peer_id, connection, responder }))) => {
                assert_eq!(PeerId(1), peer_id.into());
                assert_eq!(connection, ConnectParameters::L2cap(Peer::transport_channel_params()));
                let channel = transport_chan.try_into().unwrap();
                responder.send(Ok(channel)).expect("responder sends");
            }
            x => panic!("Should have sent a open l2cap request, but got {:?}", x),
        };

        // Setup finishes when the media transport is connected, and the start is sent by the
        // task which takes the permit and starts the stream.
        exec.run_until_stalled(&mut start_future)
            .expect("start setup finished")
            .expect("stream setup is ok");
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());
        assert!(!peer.is_streaming_now());

        // Before peer responds to start, the permit gets taken.
        let seized_permits = test_permits.seize();
        assert_eq!(seized_permits.len(), 1);
        receive_simple_accept(&mut exec, &mut remote, 0x07); // Start

        // Streaming should not locally begin because there is no available permit. The Start
        // response is handled gracefully.
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());
        assert!(!peer.is_streaming_now());
        // We should issue an outgoing suspend request to synchronize state with the remote peer.
        receive_simple_accept(&mut exec, &mut remote, 0x09); // Suspend

        // A2DP should not have started streaming.
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());
        assert!(!peer.is_streaming_now());
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    fn peer_stream_start_fails_wrong_direction(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();

        // Setup peers with only one Source Stream.
        let mut streams = Streams::default();
        let source = Stream::build(
            make_sbc_endpoint(1, avdtp::EndpointType::Source),
            TestMediaTaskBuilder::new().builder(),
        );
        streams.insert(source);

        let (remote, _requests, _, peer) = setup_test_peer(transport, false, streams, None);
        let remote = avdtp::Peer::new(remote);
        let mut remote_events = remote.take_request_stream();

        // Respond as if we have a single SBC Source Stream
        fn remote_handle_request(req: avdtp::Request) {
            let expected_stream_id: StreamEndpointId = 2_u8.try_into().unwrap();
            let res = match req {
                avdtp::Request::Discover { responder } => {
                    let infos = [avdtp::StreamInformation::new(
                        expected_stream_id,
                        false,
                        avdtp::MediaType::Audio,
                        avdtp::EndpointType::Source,
                    )];
                    responder.send(&infos)
                }
                avdtp::Request::GetAllCapabilities { stream_id, responder }
                | avdtp::Request::GetCapabilities { stream_id, responder } => {
                    assert_eq!(expected_stream_id, stream_id);
                    let caps = vec![
                        ServiceCapability::MediaTransport,
                        ServiceCapability::MediaCodec {
                            media_type: avdtp::MediaType::Audio,
                            codec_type: avdtp::MediaCodecType::AUDIO_SBC,
                            codec_extra: vec![0x11, 0x45, 51, 250],
                        },
                    ];
                    responder.send(&caps[..])
                }
                avdtp::Request::Open { responder, .. } => responder.send(),
                avdtp::Request::SetConfiguration { responder, .. } => responder.send(),
                x => panic!("Unexpected request: {:?}", x),
            };
            res.expect("should be able to respond");
        }

        // Need to discover the remote streams first, or the stream start will always work.
        let collect_capabilities_fut = peer.collect_capabilities();
        let mut collect_capabilities_fut = pin!(collect_capabilities_fut);

        assert!(exec.run_until_stalled(&mut collect_capabilities_fut).is_pending());

        let request = exec.run_singlethreaded(&mut remote_events.next());
        remote_handle_request(request.expect("should have a discovery request").unwrap());

        assert!(exec.run_until_stalled(&mut collect_capabilities_fut).is_pending());
        let request = exec.run_singlethreaded(&mut remote_events.next());
        remote_handle_request(request.expect("should have a get_capabilities request").unwrap());

        assert!(exec.run_until_stalled(&mut collect_capabilities_fut).is_ready());

        // Try to start the stream.  It should fail with OutOfRange because we can't connect a Source to a Source.
        let remote_seid = 2_u8.try_into().unwrap();

        let codec_params = ServiceCapability::MediaCodec {
            media_type: avdtp::MediaType::Audio,
            codec_type: avdtp::MediaCodecType::AUDIO_SBC,
            codec_extra: vec![0x11, 0x45, 51, 51],
        };
        let start_future = peer.stream_start(remote_seid, vec![codec_params]);
        let mut start_future = pin!(start_future);

        match exec.run_until_stalled(&mut start_future) {
            Poll::Ready(Err(avdtp::Error::OutOfRange)) => {}
            x => panic!("Expected a ready OutOfRange error but got {:?}", x),
        };
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    fn peer_stream_start_fails_to_connect(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();

        let (mut remote, mut profile_request_stream, _, peer) =
            setup_test_peer(transport, false, build_test_streams(), None);

        let remote_seid = 2_u8.try_into().unwrap();

        let codec_params = ServiceCapability::MediaCodec {
            media_type: avdtp::MediaType::Audio,
            codec_type: avdtp::MediaCodecType::AUDIO_SBC,
            codec_extra: vec![0x11, 0x45, 51, 51],
        };

        let start_future = peer.stream_start(remote_seid, vec![codec_params]);
        let mut start_future = pin!(start_future);

        match exec.run_until_stalled(&mut start_future) {
            Poll::Pending => {}
            x => panic!("was expecting pending but got {x:?}"),
        };

        receive_simple_accept(&mut exec, &mut remote, 0x03); // Set Configuration

        assert!(exec.run_until_stalled(&mut start_future).is_pending());

        receive_simple_accept(&mut exec, &mut remote, 0x06); // Open

        match exec.run_until_stalled(&mut start_future) {
            Poll::Pending => {}
            Poll::Ready(x) => panic!("Expected to be pending but {x:?}"),
        };

        // Should connect the media channel after open.
        let request = exec.run_until_stalled(&mut profile_request_stream.next());
        match request {
            Poll::Ready(Some(Ok(ProfileRequest::Connect { peer_id, responder, .. }))) => {
                assert_eq!(PeerId(1), peer_id.into());
                responder.send(Err(ErrorCode::Failed)).expect("responder sends");
            }
            x => panic!("Should have sent a open l2cap request, but got {:?}", x),
        };

        // Should return an error.
        // Should be done without an error, but with no streams.
        match exec.run_until_stalled(&mut start_future) {
            Poll::Pending => panic!("Should be ready after start fails"),
            Poll::Ready(Ok(_stream)) => panic!("Shouldn't have succeeded stream here"),
            Poll::Ready(Err(_)) => {}
        }
    }

    /// Test that the delay reports get acknowledged and they are sent to cobalt.
    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    async fn peer_delay_report(transport: Transport) {
        let (remote, _profile_requests, cobalt_recv, peer) =
            setup_test_peer(transport, true, build_test_streams(), None);
        let remote_peer = avdtp::Peer::new(remote);
        let mut remote_events = remote_peer.take_request_stream();

        // Respond as if we have a single SBC Sink Stream
        async fn remote_handle_request(req: avdtp::Request, peer: &avdtp::Peer) {
            let expected_stream_id: StreamEndpointId = 4_u8.try_into().unwrap();
            // "peer" in this case is the test code Peer stream
            let expected_peer_stream_id: StreamEndpointId = 1_u8.try_into().unwrap();
            use avdtp::Request::*;
            match req {
                Discover { responder } => {
                    let infos = [avdtp::StreamInformation::new(
                        expected_stream_id,
                        false,
                        avdtp::MediaType::Audio,
                        avdtp::EndpointType::Sink,
                    )];
                    responder.send(&infos).expect("response should succeed");
                }
                GetAllCapabilities { stream_id, responder }
                | GetCapabilities { stream_id, responder } => {
                    assert_eq!(expected_stream_id, stream_id);
                    let caps = vec![
                        ServiceCapability::MediaTransport,
                        ServiceCapability::MediaCodec {
                            media_type: avdtp::MediaType::Audio,
                            codec_type: avdtp::MediaCodecType::AUDIO_SBC,
                            codec_extra: vec![0x11, 0x45, 51, 250],
                        },
                    ];
                    responder.send(&caps[..]).expect("response should succeed");
                    // Sending a delayreport before the stream is configured is not allowed, it's a
                    // bad state.
                    assert!(peer.delay_report(&expected_peer_stream_id, 0xc0de).await.is_err());
                }
                Open { responder, stream_id } => {
                    // Configuration has happened but open not succeeded yet, send delay reports.
                    assert!(peer.delay_report(&expected_stream_id, 0xc0de).await.is_err());
                    // Send a delay report to the peer.
                    peer.delay_report(&expected_peer_stream_id, 0xc0de)
                        .await
                        .expect("should get acked correctly");
                    assert_eq!(expected_stream_id, stream_id);
                    responder.send().expect("response should succeed");
                }
                SetConfiguration { responder, local_stream_id, remote_stream_id, .. } => {
                    assert_eq!(local_stream_id, expected_stream_id);
                    assert_eq!(remote_stream_id, expected_peer_stream_id);
                    responder.send().expect("should send back response without issue");
                }
                x => panic!("Unexpected request: {:?}", x),
            };
        }

        let collect_fut = pin!(peer.collect_capabilities());

        // Discover then a GetCapabilities.
        let Either::Left((request, collect_fut)) =
            futures::future::select(remote_events.next(), collect_fut).await
        else {
            panic!("Collect future shouldn't finish first");
        };
        let collect_fut = pin!(collect_fut);
        remote_handle_request(request.expect("a request").unwrap(), &remote_peer).await;
        let Either::Left((request, collect_fut)) =
            futures::future::select(remote_events.next(), collect_fut).await
        else {
            panic!("Collect future shouldn't finish first");
        };
        remote_handle_request(request.expect("a request").unwrap(), &remote_peer).await;

        // Collect future should be able to finish now.
        assert_eq!(1, collect_fut.await.expect("should get the remote endpoints back").len());

        // Try to start the stream.  It should go through the normal motions,
        let remote_seid = 4_u8.try_into().unwrap();

        let codec_params = ServiceCapability::MediaCodec {
            media_type: avdtp::MediaType::Audio,
            codec_type: avdtp::MediaCodecType::AUDIO_SBC,
            codec_extra: vec![0x11, 0x45, 51, 51],
        };

        // We don't expect this task to finish before being dropped, since we never respond to the
        // request to open the transport channel.
        let _start_task = fasync::Task::spawn(async move {
            let _ = peer.stream_start(remote_seid, vec![codec_params]).await;
            panic!("stream start task finished");
        });

        let request = remote_events.next().await.expect("should have set_config").unwrap();
        remote_handle_request(request, &remote_peer).await;

        let request = remote_events.next().await.expect("should have open").unwrap();
        remote_handle_request(request, &remote_peer).await;

        let mut cobalt = cobalt_recv.expect("should have receiver");

        let mut got_ids = HashMap::new();
        let delay_metric_id = bt_metrics::AVDTP_DELAY_REPORT_IN_NANOSECONDS_METRIC_ID;
        while got_ids.len() < 3 || *got_ids.get(&delay_metric_id).unwrap_or(&0) < 3 {
            let report = respond_to_metrics_req_for_test(cobalt.next().await.unwrap().unwrap());
            let _ = got_ids.entry(report.metric_id).and_modify(|x| *x += 1).or_insert(1);
            // All the delay reports should report the same value correctly.
            if report.metric_id == delay_metric_id {
                assert_eq!(MetricEventPayload::IntegerValue(0xc0de * 100000), report.payload);
            }
        }
        assert!(got_ids.contains_key(&bt_metrics::A2DP_CODEC_AVAILABILITY_MIGRATED_METRIC_ID));
        assert!(got_ids.contains_key(&bt_metrics::A2DP_REMOTE_PEER_CAPABILITIES_METRIC_ID));
        assert!(got_ids.contains_key(&delay_metric_id));
        // There should have been three reports.
        // We report the delay amount even if it fails to work.
        assert_eq!(got_ids.get(&delay_metric_id).cloned(), Some(3));
    }

    fn sbc_capabilities() -> Vec<ServiceCapability> {
        let sbc_codec_info = SbcCodecInfo::new(
            SbcSamplingFrequency::FREQ48000HZ,
            SbcChannelMode::JOINT_STEREO,
            SbcBlockCount::SIXTEEN,
            SbcSubBands::EIGHT,
            SbcAllocation::LOUDNESS,
            /* min_bpv= */ 53,
            /* max_bpv= */ 53,
        )
        .expect("sbc codec info");

        vec![avdtp::ServiceCapability::MediaTransport, sbc_codec_info.into()]
    }

    /// Test that the remote end can configure and start a stream.
    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    fn peer_as_acceptor(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();

        let mut streams = Streams::default();
        let mut test_builder = TestMediaTaskBuilder::new_inactive();
        streams.insert(Stream::build(
            make_sbc_endpoint(1, avdtp::EndpointType::Source),
            test_builder.builder(),
        ));

        let (remote, _requests, _, peer) = setup_test_peer(transport, false, streams, None);
        let remote_peer = avdtp::Peer::new(remote);

        let discover_fut = remote_peer.discover();
        let mut discover_fut = pin!(discover_fut);

        let expected = vec![make_sbc_endpoint(1, avdtp::EndpointType::Source).information()];
        match exec.run_until_stalled(&mut discover_fut) {
            Poll::Ready(Ok(res)) => assert_eq!(res, expected),
            x => panic!("Expected discovery to complete and got {:?}", x),
        };

        let sbc_endpoint_id = 1_u8.try_into().expect("should be able to get sbc endpointid");
        let unknown_endpoint_id = 2_u8.try_into().expect("should be able to get sbc endpointid");

        let get_caps_fut = remote_peer.get_capabilities(&sbc_endpoint_id);
        let mut get_caps_fut = pin!(get_caps_fut);

        match exec.run_until_stalled(&mut get_caps_fut) {
            // There are two caps (mediatransport, mediacodec) in the sbc endpoint.
            Poll::Ready(Ok(caps)) => assert_eq!(2, caps.len()),
            x => panic!("Get capabilities should be ready but got {:?}", x),
        };

        let get_caps_fut = remote_peer.get_capabilities(&unknown_endpoint_id);
        let mut get_caps_fut = pin!(get_caps_fut);

        match exec.run_until_stalled(&mut get_caps_fut) {
            Poll::Ready(Err(avdtp::Error::RemoteRejected(e))) => {
                assert_eq!(Some(Ok(avdtp::ErrorCode::BadAcpSeid)), e.error_code())
            }
            x => panic!("Get capabilities should be a ready error but got {:?}", x),
        };

        let get_caps_fut = remote_peer.get_all_capabilities(&sbc_endpoint_id);
        let mut get_caps_fut = pin!(get_caps_fut);

        match exec.run_until_stalled(&mut get_caps_fut) {
            // There are two caps (mediatransport, mediacodec) in the sbc endpoint.
            Poll::Ready(Ok(caps)) => assert_eq!(2, caps.len()),
            x => panic!("Get capabilities should be ready but got {:?}", x),
        };

        let sbc_caps = sbc_capabilities();
        let set_config_fut =
            remote_peer.set_configuration(&sbc_endpoint_id, &sbc_endpoint_id, &sbc_caps);
        let mut set_config_fut = pin!(set_config_fut);

        match exec.run_until_stalled(&mut set_config_fut) {
            Poll::Ready(Ok(())) => {}
            x => panic!("Set capabilities should be ready but got {:?}", x),
        };

        let open_fut = remote_peer.open(&sbc_endpoint_id);
        let mut open_fut = pin!(open_fut);
        match exec.run_until_stalled(&mut open_fut) {
            Poll::Ready(Ok(())) => {}
            x => panic!("Open should be ready but got {:?}", x),
        };

        // Establish a media transport stream
        let (transport_chan, _remote_transport) = create_test_channels(transport);

        assert_eq!(Some(()), peer.receive_channel(transport_chan).ok());

        let stream_ids = vec![sbc_endpoint_id.clone()];
        let start_fut = remote_peer.start(&stream_ids);
        let mut start_fut = pin!(start_fut);
        match exec.run_until_stalled(&mut start_fut) {
            Poll::Ready(Ok(())) => {}
            x => panic!("Start should be ready but got {:?}", x),
        };

        // The task should be created locally and started.
        let media_task = test_builder.expect_task();
        assert!(media_task.is_started());

        let suspend_fut = remote_peer.suspend(&stream_ids);
        let mut suspend_fut = pin!(suspend_fut);
        match exec.run_until_stalled(&mut suspend_fut) {
            Poll::Ready(Ok(())) => {}
            x => panic!("Start should be ready but got {:?}", x),
        };

        // Should have stopped the media task on suspend.
        assert!(!media_task.is_started());
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    fn peer_source_stream_suspends_and_resumes_on_audio_disabled(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();

        let mut streams = Streams::default();
        let mut test_builder = TestMediaTaskBuilder::new_inactive();
        let _ = test_builder.with_direction(avdtp::EndpointType::Source);
        streams.insert(Stream::build(
            make_sbc_endpoint(1, avdtp::EndpointType::Source),
            test_builder.builder(),
        ));

        let (remote, _requests, _, peer) = setup_test_peer(transport, false, streams, None);
        let remote_peer = avdtp::Peer::new(remote);

        let sbc_endpoint_id = 1_u8.try_into().expect("sbc endpoint id");
        let sbc_caps = sbc_capabilities();

        // Configure and open from remote
        let mut set_config_fut =
            pin!(remote_peer.set_configuration(&sbc_endpoint_id, &sbc_endpoint_id, &sbc_caps));
        assert!(exec.run_until_stalled(&mut set_config_fut).is_ready());

        let mut open_fut = pin!(remote_peer.open(&sbc_endpoint_id));
        assert!(exec.run_until_stalled(&mut open_fut).is_ready());

        // Establish media transport
        let (transport_chan, _remote_transport) = create_test_channels(transport);
        assert!(peer.receive_channel(transport_chan).is_ok());

        // Start stream
        let stream_ids = vec![sbc_endpoint_id.clone()];
        let mut start_fut = pin!(remote_peer.start(&stream_ids));
        assert!(exec.run_until_stalled(&mut start_fut).is_ready());

        // Media task is running
        let media_task = test_builder.expect_task();
        assert!(media_task.is_started());

        // End media task prematurely with AudioDisabled (channels went silent)
        media_task.end_prematurely(Some(Ok(MediaTaskStatus::AudioDisabled)));

        // Remote peer should receive an AVDTP Suspend request from local peer
        let mut remote_events = remote_peer.take_request_stream();
        let mut req_fut = remote_events.next();
        let Poll::Ready(Some(Ok(avdtp::Request::Suspend { responder, stream_ids }))) =
            exec.run_until_stalled(&mut req_fut)
        else {
            panic!("Expected Suspend request from peer");
        };
        assert_eq!(stream_ids, vec![sbc_endpoint_id.clone()]);
        responder.send().expect("suspend response should send");

        // Media task should now be stopped
        assert!(!media_task.is_started());

        // Reactivate audio channels
        test_builder.set_active(true);

        // Remote peer should now receive an AVDTP Start request from local peer
        let mut req_fut = remote_events.next();
        let Poll::Ready(Some(Ok(avdtp::Request::Start { responder, stream_ids }))) =
            exec.run_until_stalled(&mut req_fut)
        else {
            panic!("Expected Start request from peer");
        };
        assert_eq!(stream_ids, vec![sbc_endpoint_id.clone()]);
        responder.send().expect("start response should send");

        // A new media task should be created and started
        let new_media_task =
            exec.run_until_stalled(&mut test_builder.next_task()).expect("ready").unwrap();
        assert!(new_media_task.is_started());
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    fn peer_set_config_reject_first(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();

        let mut streams = Streams::default();
        let test_builder = TestMediaTaskBuilder::new();
        streams.insert(Stream::build(
            make_sbc_endpoint(1, avdtp::EndpointType::Source),
            test_builder.builder(),
        ));

        let (remote, _requests, _, _peer) = setup_test_peer(transport, false, streams, None);
        let remote_peer = avdtp::Peer::new(remote);

        let sbc_endpoint_id = 1_u8.try_into().expect("should be able to get sbc endpointid");

        let wrong_freq_sbc = &[SbcCodecInfo::new(
            SbcSamplingFrequency::FREQ44100HZ, // 44.1 is not supported by the caps from above.
            SbcChannelMode::JOINT_STEREO,
            SbcBlockCount::SIXTEEN,
            SbcSubBands::EIGHT,
            SbcAllocation::LOUDNESS,
            /* min_bpv= */ 53,
            /* max_bpv= */ 53,
        )
        .expect("sbc codec info")
        .into()];

        let set_config_fut =
            remote_peer.set_configuration(&sbc_endpoint_id, &sbc_endpoint_id, wrong_freq_sbc);
        let mut set_config_fut = pin!(set_config_fut);

        match exec.run_until_stalled(&mut set_config_fut) {
            Poll::Ready(Err(avdtp::Error::RemoteRejected(e))) => {
                assert!(e.service_category().is_some())
            }
            x => panic!("Set capabilities should have been rejected but got {:?}", x),
        };

        let sbc_caps = sbc_capabilities();
        let set_config_fut =
            remote_peer.set_configuration(&sbc_endpoint_id, &sbc_endpoint_id, &sbc_caps);
        let mut set_config_fut = pin!(set_config_fut);

        match exec.run_until_stalled(&mut set_config_fut) {
            Poll::Ready(Ok(())) => {}
            x => panic!("Set capabilities should be ready but got {:?}", x),
        };
    }

    /// When a start that we initiate because the audio became active fails, we should try again
    /// while the audio is still active.
    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    fn peer_retries_failed_start_while_audio_active(transport_mode: Transport) {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(5_000_000_000));

        let mut streams = Streams::default();
        let mut test_builder = TestMediaTaskBuilder::new_inactive();
        let _ = test_builder.with_direction(avdtp::EndpointType::Source);
        streams.insert(Stream::build(
            make_sbc_endpoint(1, avdtp::EndpointType::Source),
            test_builder.builder(),
        ));

        let (remote, _requests, _, peer) = setup_test_peer(transport_mode, false, streams, None);
        let remote_peer = avdtp::Peer::new(remote);

        let sbc_endpoint_id: StreamEndpointId = 1_u8.try_into().expect("sbc endpoint id");
        let sbc_caps = sbc_capabilities();

        let mut set_config_fut =
            pin!(remote_peer.set_configuration(&sbc_endpoint_id, &sbc_endpoint_id, &sbc_caps));
        match exec.run_until_stalled(&mut set_config_fut) {
            Poll::Ready(Ok(())) => {}
            x => panic!("Set capabilities should be ready but got {:?}", x),
        };

        let mut open_fut = pin!(remote_peer.open(&sbc_endpoint_id));
        match exec.run_until_stalled(&mut open_fut) {
            Poll::Ready(Ok(())) => {}
            x => panic!("Open should be ready but got {:?}", x),
        };

        // Establish a media transport stream
        let (transport, _remote_transport) = create_test_channels(transport_mode);
        assert_eq!(Some(()), peer.receive_channel(transport).ok());

        let mut remote_requests = remote_peer.take_request_stream();

        // Signal that audio has become active, which should start the stream.
        test_builder.set_active(true);

        // Reject the start request, as if the peer was in a state where it couldn't start.
        let mut next_remote_request_fut = pin!(remote_requests.next());
        match exec.run_until_stalled(&mut next_remote_request_fut) {
            Poll::Ready(Some(Ok(avdtp::Request::Start { responder, stream_ids }))) => {
                assert_eq!(stream_ids, vec![sbc_endpoint_id.clone()]);
                responder
                    .reject(&sbc_endpoint_id, avdtp::ErrorCode::BadState)
                    .expect("reject response should send");
            }
            x => panic!("Expected to receive a start request for the stream, got {:?}", x),
        };

        // The stream shouldn't have been started.
        assert!(exec.run_until_stalled(&mut test_builder.next_task()).is_pending());
        assert!(!peer.is_streaming_now());

        // The audio is still active, so we should try to start again after the retry delay.
        let mut next_remote_request_fut = pin!(remote_requests.next());
        assert!(exec.run_until_stalled(&mut next_remote_request_fut).is_pending());

        exec.set_fake_time(fasync::MonotonicInstant::after(
            START_RETRY_DELAY + zx::MonotonicDuration::from_micros(1),
        ));
        assert!(exec.wake_expired_timers());

        match exec.run_until_stalled(&mut next_remote_request_fut) {
            Poll::Ready(Some(Ok(avdtp::Request::Start { responder, stream_ids }))) => {
                assert_eq!(stream_ids, vec![sbc_endpoint_id.clone()]);
                responder.send().expect("start response should send");
            }
            x => panic!("Expected to receive a second start request, got {:?}", x),
        };

        // The second start succeeded, so the media task should be started now.
        let media_task =
            exec.run_until_stalled(&mut test_builder.next_task()).expect("ready").unwrap();
        assert!(media_task.is_started());
    }

    /// Builds a peer with a single SBC sink stream with the endpoint id `seid`, and configures and
    /// opens it from the remote peer, leaving the stream open but not started by anyone.
    /// Returns the endpoint id, the peer, the remote peer, and the remote end of the media
    /// transport, which keeps the transport open while it is held.
    fn setup_open_sink_stream(
        exec: &mut fasync::TestExecutor,
        test_builder: &TestMediaTaskBuilder,
        seid: u8,
        transport_mode: Transport,
    ) -> (StreamEndpointId, Peer, avdtp::Peer, Channel) {
        let mut streams = Streams::default();
        streams.insert(Stream::build(
            make_sbc_endpoint(seid, avdtp::EndpointType::Sink),
            test_builder.builder(),
        ));

        let (remote, _requests, _, peer) = setup_test_peer(transport_mode, false, streams, None);
        let remote_peer = avdtp::Peer::new(remote);

        let local_id: StreamEndpointId = seid.try_into().expect("sbc endpoint id");
        let sbc_caps = sbc_capabilities();
        let mut set_config_fut =
            pin!(remote_peer.set_configuration(&local_id, &local_id, &sbc_caps));
        match exec.run_until_stalled(&mut set_config_fut) {
            Poll::Ready(Ok(())) => {}
            x => panic!("Set configuration should be ready but got {:?}", x),
        };

        let mut open_fut = pin!(remote_peer.open(&local_id));
        match exec.run_until_stalled(&mut open_fut) {
            Poll::Ready(Ok(())) => {}
            x => panic!("Open should be ready but got {:?}", x),
        };

        // Establish a media transport stream, which opens the stream.
        let (transport, remote_transport) = create_test_channels(transport_mode);
        assert_eq!(Some(()), peer.receive_channel(transport).ok());

        // Let the task waiting to start the stream run, so that it is dwelling.
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());

        (local_id, peer, remote_peer, remote_transport)
    }

    /// Sink streams are normally started by the peer, since it is the source of the audio.  If the
    /// peer doesn't start the stream it opened, we start it ourselves once the dwell has expired.
    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    fn peer_starts_sink_stream_after_dwell(transport_mode: Transport) {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(5_000_000_000));

        let mut test_builder = TestMediaTaskBuilder::new();
        let (sbc_endpoint_id, _peer, remote_peer, _remote_transport) =
            setup_open_sink_stream(&mut exec, &test_builder, 1, transport_mode);
        let mut remote_requests = remote_peer.take_request_stream();

        // The peer gets a chance to start the stream itself first.
        let mut next_remote_request_fut = pin!(remote_requests.next());
        assert!(exec.run_until_stalled(&mut next_remote_request_fut).is_pending());

        // When the dwell expires without the peer starting the stream, we start it.
        exec.set_fake_time(fasync::MonotonicInstant::after(
            STREAM_DWELL + zx::MonotonicDuration::from_micros(1),
        ));
        assert!(exec.wake_expired_timers());

        match exec.run_until_stalled(&mut next_remote_request_fut) {
            Poll::Ready(Some(Ok(avdtp::Request::Start { responder, stream_ids }))) => {
                assert_eq!(stream_ids, vec![sbc_endpoint_id.clone()]);
                responder.send().expect("start response should send");
            }
            x => panic!("Expected to receive a start request for the stream, got {:?}", x),
        };

        let media_task =
            exec.run_until_stalled(&mut test_builder.next_task()).expect("ready").unwrap();
        assert!(media_task.is_started());
    }

    /// When the peer starts the sink stream it opened, we don't start it, and we leave the stream
    /// to the peer afterwards: if it suspends the stream we wait for it to start it again.
    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    fn peer_does_not_start_sink_stream_started_by_peer(transport_mode: Transport) {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(5_000_000_000));

        let mut test_builder = TestMediaTaskBuilder::new();
        let (sbc_endpoint_id, _peer, remote_peer, _remote_transport) =
            setup_open_sink_stream(&mut exec, &test_builder, 1, transport_mode);
        let mut remote_requests = remote_peer.take_request_stream();

        // The peer starts the stream before the dwell expires.
        let mut start_fut = pin!(remote_peer.start(&[sbc_endpoint_id.clone()]));
        match exec.run_until_stalled(&mut start_fut) {
            Poll::Ready(Ok(())) => {}
            x => panic!("Start should be ready but got {:?}", x),
        };

        let media_task = test_builder.expect_task();
        assert!(media_task.is_started());

        // We shouldn't send anything once the dwell would have expired.
        exec.set_fake_time(fasync::MonotonicInstant::after(
            STREAM_DWELL + zx::MonotonicDuration::from_micros(1),
        ));
        let _ = exec.wake_expired_timers();
        let mut next_remote_request_fut = pin!(remote_requests.next());
        assert!(exec.run_until_stalled(&mut next_remote_request_fut).is_pending());
        assert!(media_task.is_started());

        // The peer suspends the stream, which stops the media task.
        let mut suspend_fut = pin!(remote_peer.suspend(&[sbc_endpoint_id.clone()]));
        match exec.run_until_stalled(&mut suspend_fut) {
            Poll::Ready(Ok(())) => {}
            x => panic!("Suspend should be ready but got {:?}", x),
        };
        assert!(!media_task.is_started());

        // We don't start it again, the peer will start it when it has audio to send.
        exec.set_fake_time(fasync::MonotonicInstant::after(STREAM_DWELL + START_RETRY_DELAY));
        let _ = exec.wake_expired_timers();
        let mut next_remote_request_fut = pin!(remote_requests.next());
        assert!(exec.run_until_stalled(&mut next_remote_request_fut).is_pending());
    }

    /// When the peer refuses to start the sink stream we opened for it, we stop trying after a few
    /// attempts instead of asking forever.
    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    fn peer_stops_starting_sink_stream_after_attempts(transport_mode: Transport) {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(5_000_000_000));

        let test_builder = TestMediaTaskBuilder::new();
        let (sbc_endpoint_id, peer, remote_peer, _remote_transport) =
            setup_open_sink_stream(&mut exec, &test_builder, 1, transport_mode);
        let mut remote_requests = remote_peer.take_request_stream();

        // The peer doesn't start the stream, so we try to after the dwell.
        exec.set_fake_time(fasync::MonotonicInstant::after(
            STREAM_DWELL + zx::MonotonicDuration::from_micros(1),
        ));
        assert!(exec.wake_expired_timers());

        for attempt in 1..=START_ATTEMPTS {
            let mut next_remote_request_fut = pin!(remote_requests.next());
            match exec.run_until_stalled(&mut next_remote_request_fut) {
                Poll::Ready(Some(Ok(avdtp::Request::Start { responder, stream_ids }))) => {
                    assert_eq!(stream_ids, vec![sbc_endpoint_id.clone()]);
                    responder
                        .reject(&sbc_endpoint_id, avdtp::ErrorCode::BadState)
                        .expect("reject response should send");
                }
                x => panic!("Expected start request number {attempt}, got {:?}", x),
            };
            // Let the failed start be processed, which sets up the retry.
            let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());
            exec.set_fake_time(fasync::MonotonicInstant::after(
                START_RETRY_DELAY + zx::MonotonicDuration::from_micros(1),
            ));
            let retried = exec.wake_expired_timers();
            assert_eq!(retried, attempt < START_ATTEMPTS, "attempt {attempt} retry");
        }

        // We have given up: the peer can start the stream itself if it wants to stream.  Give any
        // dwell that was started again a chance to expire.
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());
        exec.set_fake_time(fasync::MonotonicInstant::after(STREAM_DWELL + START_RETRY_DELAY));
        assert!(!exec.wake_expired_timers());
        let mut next_remote_request_fut = pin!(remote_requests.next());
        assert!(exec.run_until_stalled(&mut next_remote_request_fut).is_pending());
        assert!(!peer.streaming_active());
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    fn peer_starts_waiting_streams(transport_mode: Transport) {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(5_000_000_000));

        let mut streams = Streams::default();
        let mut test_builder = TestMediaTaskBuilder::new_inactive();
        streams.insert(Stream::build(
            make_sbc_endpoint(1, avdtp::EndpointType::Source),
            test_builder.builder(),
        ));

        let (remote, _requests, _, peer) = setup_test_peer(transport_mode, false, streams, None);
        let remote_peer = avdtp::Peer::new(remote);

        let sbc_endpoint_id = 1_u8.try_into().expect("should be able to get sbc endpointid");

        let sbc_caps = sbc_capabilities();
        let set_config_fut =
            remote_peer.set_configuration(&sbc_endpoint_id, &sbc_endpoint_id, &sbc_caps);
        let mut set_config_fut = pin!(set_config_fut);

        match exec.run_until_stalled(&mut set_config_fut) {
            Poll::Ready(Ok(())) => {}
            x => panic!("Set capabilities should be ready but got {:?}", x),
        };

        let open_fut = remote_peer.open(&sbc_endpoint_id);
        let mut open_fut = pin!(open_fut);
        match exec.run_until_stalled(&mut open_fut) {
            Poll::Ready(Ok(())) => {}
            x => panic!("Open should be ready but got {:?}", x),
        };

        // Establish a media transport stream
        let (transport, _remote_transport) = create_test_channels(transport_mode);
        assert_eq!(Some(()), peer.receive_channel(transport).ok());

        // The remote end should get a start request after audio becomes active.
        let mut remote_requests = remote_peer.take_request_stream();
        let next_remote_request_fut = remote_requests.next();
        let mut next_remote_request_fut = pin!(next_remote_request_fut);

        // Nothing should happen while inactive.
        assert!(exec.run_until_stalled(&mut next_remote_request_fut).is_pending());

        // Signal that audio has become active.
        test_builder.set_active(true);

        let stream_ids = match exec.run_until_stalled(&mut next_remote_request_fut) {
            Poll::Ready(Some(Ok(avdtp::Request::Start { responder, stream_ids }))) => {
                responder.send().unwrap();
                stream_ids
            }
            x => panic!("Expected to receive a start request for the stream, got {:?}", x),
        };

        // We should start the media task, so the task should be created locally
        let media_task =
            exec.run_until_stalled(&mut test_builder.next_task()).expect("ready").unwrap();
        assert!(media_task.is_started());

        // Remote peer should still be able to suspend the stream.
        let suspend_fut = remote_peer.suspend(&stream_ids);
        let mut suspend_fut = pin!(suspend_fut);
        match exec.run_until_stalled(&mut suspend_fut) {
            Poll::Ready(Ok(())) => {}
            x => panic!("Suspend should be ready but got {:?}", x),
        };

        // Should have stopped the media task on suspend.
        assert!(!media_task.is_started());
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    fn needs_permit_to_start_streams(transport_mode: Transport) {
        let mut exec = fasync::TestExecutor::new();

        let mut streams = Streams::default();
        let mut test_builder = TestMediaTaskBuilder::new();
        streams.insert(Stream::build(
            make_sbc_endpoint(1, avdtp::EndpointType::Sink),
            test_builder.builder(),
        ));
        streams.insert(Stream::build(
            make_sbc_endpoint(2, avdtp::EndpointType::Sink),
            test_builder.builder(),
        ));
        let mut next_task_fut = test_builder.next_task();

        let permits = Permits::new(1);
        let taken_permit = permits.get().expect("permit taken");
        let (remote, _profile_request_stream, _, peer) =
            setup_test_peer(transport_mode, false, streams, Some(permits.clone()));
        let remote_peer = avdtp::Peer::new(remote);

        let sbc_endpoint_id = 1_u8.try_into().unwrap();

        let sbc_caps = sbc_capabilities();
        let mut set_config_fut =
            remote_peer.set_configuration(&sbc_endpoint_id, &sbc_endpoint_id, &sbc_caps);

        match exec.run_until_stalled(&mut set_config_fut) {
            Poll::Ready(Ok(())) => {}
            x => panic!("Set capabilities should be ready but got {:?}", x),
        };

        let mut open_fut = remote_peer.open(&sbc_endpoint_id);
        match exec.run_until_stalled(&mut open_fut) {
            Poll::Ready(Ok(())) => {}
            x => panic!("Open should be ready but got {:?}", x),
        };

        // Establish a media transport stream
        let (transport, _remote_transport) = create_test_channels(transport_mode);
        assert_eq!(Some(()), peer.receive_channel(transport).ok());

        // Do the same, but for the OTHER stream.
        let sbc_endpoint_two = 2_u8.try_into().unwrap();

        let mut set_config_fut =
            remote_peer.set_configuration(&sbc_endpoint_two, &sbc_endpoint_two, &sbc_caps);

        match exec.run_until_stalled(&mut set_config_fut) {
            Poll::Ready(Ok(())) => {}
            x => panic!("Set capabilities should be ready but got {:?}", x),
        };

        let mut open_fut = remote_peer.open(&sbc_endpoint_two);
        match exec.run_until_stalled(&mut open_fut) {
            Poll::Ready(Ok(())) => {}
            x => panic!("Open should be ready but got {:?}", x),
        };

        // Establish a media transport stream
        let (transport_two, _remote_transport_two) = create_test_channels(transport_mode);
        assert_eq!(Some(()), peer.receive_channel(transport_two).ok());

        // Remote peer should still be able to try to start the stream, and we will say yes, but
        // that last seid looks wonky.
        let unknown_endpoint_id: StreamEndpointId = 9_u8.try_into().unwrap();
        let stream_ids = [sbc_endpoint_id.clone(), unknown_endpoint_id.clone()];
        let mut start_fut = remote_peer.start(&stream_ids);
        match exec.run_until_stalled(&mut start_fut) {
            Poll::Ready(Err(avdtp::Error::RemoteRejected(rejection))) => {
                assert_eq!(avdtp::ErrorCode::BadAcpSeid, rejection.error_code().unwrap().unwrap());
                assert_eq!(unknown_endpoint_id, rejection.stream_id().unwrap());
            }
            x => panic!("Start should be ready but got {:?}", x),
        };

        // We can't get a permit (none are available) so we suspend the one we didn't error on.
        let mut remote_requests = remote_peer.take_request_stream();

        let suspended_stream_ids = match exec.run_singlethreaded(&mut remote_requests.next()) {
            Some(Ok(avdtp::Request::Suspend { responder, stream_ids })) => {
                responder.send().unwrap();
                stream_ids
            }
            x => panic!("Expected to receive a suspend request for the stream, got {:?}", x),
        };

        assert!(suspended_stream_ids.contains(&sbc_endpoint_id));
        assert_eq!(1, suspended_stream_ids.len());

        // And we should have not tried to start a task.
        match exec.run_until_stalled(&mut next_task_fut) {
            Poll::Pending => {}
            x => panic!("Local task should not have been created at this point: {:?}", x),
        };

        // No matter how many times they ask to start, we will still suspend (but not queue another
        // reservation for the same id)
        let mut start_fut = remote_peer.start(&[sbc_endpoint_id.clone()]);
        match exec.run_until_stalled(&mut start_fut) {
            Poll::Ready(Ok(())) => {}
            x => panic!("Start should be ready but got {:?}", x),
        }

        let suspended_stream_ids = match exec.run_until_stalled(&mut remote_requests.next()) {
            Poll::Ready(Some(Ok(avdtp::Request::Suspend { responder, stream_ids }))) => {
                responder.send().unwrap();
                stream_ids
            }
            x => panic!("Expected to receive a suspend request for the stream, got {:?}", x),
        };
        assert!(suspended_stream_ids.contains(&sbc_endpoint_id));

        // After a permit is available, should try to start the first endpoint that failed.
        drop(taken_permit);

        match exec.run_singlethreaded(&mut remote_requests.next()) {
            Some(Ok(avdtp::Request::Start { responder, stream_ids })) => {
                assert_eq!(stream_ids, &[sbc_endpoint_id.clone()]);
                responder.send().unwrap();
            }
            x => panic!("Expected start on permit available but got {x:?}"),
        };

        // And we should start a task.
        let media_task = match exec.run_until_stalled(&mut next_task_fut) {
            Poll::Ready(Some(task)) => task,
            x => panic!("Local task should be created at this point: {:?}", x),
        };

        assert!(media_task.is_started());

        // If the remote asks to start another one, we still suspend it immediately.
        let mut start_fut = remote_peer.start(&[sbc_endpoint_two.clone()]);
        match exec.run_until_stalled(&mut start_fut) {
            Poll::Ready(Ok(())) => {}
            x => panic!("Start should be ready but got {:?}", x),
        }

        let suspended_stream_ids = match exec.run_until_stalled(&mut remote_requests.next()) {
            Poll::Ready(Some(Ok(avdtp::Request::Suspend { responder, stream_ids }))) => {
                responder.send().unwrap();
                stream_ids
            }
            x => panic!("Expected to receive a suspend request for the stream, got {:?}", x),
        };

        assert!(suspended_stream_ids.contains(&sbc_endpoint_two));
        assert_eq!(1, suspended_stream_ids.len());

        // Once the first one is done, the second can start.
        let mut suspend_fut = remote_peer.suspend(&[sbc_endpoint_id.clone()]);
        match exec.run_until_stalled(&mut suspend_fut) {
            Poll::Ready(Ok(())) => {}
            x => panic!("Start should be ready but got {:?}", x),
        }

        match exec.run_singlethreaded(&mut remote_requests.next()) {
            Some(Ok(avdtp::Request::Start { responder, stream_ids })) => {
                assert_eq!(stream_ids, &[sbc_endpoint_two]);
                responder.send().unwrap();
            }
            x => panic!("Expected start on permit available but got {x:?}"),
        };
    }

    fn start_sbc_stream(
        exec: &mut fasync::TestExecutor,
        media_test_builder: &mut TestMediaTaskBuilder,
        peer: &Peer,
        remote_peer: &avdtp::Peer,
        local_id: &StreamEndpointId,
        remote_id: &StreamEndpointId,
        transport_mode: Transport,
    ) -> TestMediaTask {
        let sbc_caps = sbc_capabilities();
        let set_config_fut = remote_peer.set_configuration(&local_id, &remote_id, &sbc_caps);
        let mut set_config_fut = pin!(set_config_fut);

        match exec.run_until_stalled(&mut set_config_fut) {
            Poll::Ready(Ok(())) => {}
            x => panic!("Set capabilities should be ready but got {:?}", x),
        };

        let open_fut = remote_peer.open(&local_id);
        let mut open_fut = pin!(open_fut);
        match exec.run_until_stalled(&mut open_fut) {
            Poll::Ready(Ok(())) => {}
            x => panic!("Open should be ready but got {:?}", x),
        };

        // Establish a media transport stream
        let (transport, _remote_transport) = create_test_channels(transport_mode);
        assert_eq!(Some(()), peer.receive_channel(transport).ok());

        // Remote peer should still be able to try to start the stream, and we will say yes.
        let stream_ids = [local_id.clone()];
        let start_fut = remote_peer.start(&stream_ids);
        let mut start_fut = pin!(start_fut);
        match exec.run_until_stalled(&mut start_fut) {
            Poll::Ready(Ok(())) => {}
            x => panic!("Start should be ready but got {:?}", x),
        };

        // And we should start a media task.
        let media_task = media_test_builder.expect_task();
        assert!(media_task.is_started());

        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());
        media_task
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    fn permits_can_be_revoked_and_reinstated_all(transport_mode: Transport) {
        let mut exec = fasync::TestExecutor::new();

        let mut streams = Streams::default();
        let mut test_builder = TestMediaTaskBuilder::new();
        streams.insert(Stream::build(
            make_sbc_endpoint(1, avdtp::EndpointType::Sink),
            test_builder.builder(),
        ));
        let sbc_endpoint_id = 1_u8.try_into().unwrap();
        let remote_sbc_endpoint_id = 7_u8.try_into().unwrap();

        streams.insert(Stream::build(
            make_sbc_endpoint(2, avdtp::EndpointType::Sink),
            test_builder.builder(),
        ));
        let sbc2_endpoint_id = 2_u8.try_into().unwrap();
        let remote_sbc2_endpoint_id = 6_u8.try_into().unwrap();

        let permits = Permits::new(2);

        let (remote, _requests, _, peer) =
            setup_test_peer(transport_mode, false, streams, Some(permits.clone()));
        let remote_peer = avdtp::Peer::new(remote);

        let one_media_task = start_sbc_stream(
            &mut exec,
            &mut test_builder,
            &peer,
            &remote_peer,
            &sbc_endpoint_id,
            &remote_sbc_endpoint_id,
            transport_mode,
        );
        let two_media_task = start_sbc_stream(
            &mut exec,
            &mut test_builder,
            &peer,
            &remote_peer,
            &sbc2_endpoint_id,
            &remote_sbc2_endpoint_id,
            transport_mode,
        );

        // Someone comes along and revokes our permits.
        let taken_permits = permits.seize();

        let remote_endpoints: HashSet<_> =
            [&remote_sbc_endpoint_id, &remote_sbc2_endpoint_id].iter().cloned().collect();

        // We should send a suspend to the other end, for both of them.
        let mut remote_requests = remote_peer.take_request_stream();
        let mut expected_suspends = remote_endpoints.clone();
        while !expected_suspends.is_empty() {
            match exec.run_until_stalled(&mut remote_requests.next()) {
                Poll::Ready(Some(Ok(avdtp::Request::Suspend { responder, stream_ids }))) => {
                    for stream_id in stream_ids {
                        assert!(expected_suspends.remove(&stream_id));
                    }
                    responder.send().expect("send response okay");
                }
                x => panic!("Expected suspension and got {:?}", x),
            }
        }

        // And the media tasks should be stopped.
        assert!(!one_media_task.is_started());
        assert!(!two_media_task.is_started());

        // After the permits are available again, we send a start, and start the media stream.
        drop(taken_permits);

        let mut expected_starts = remote_endpoints.clone();
        while !expected_starts.is_empty() {
            match exec.run_singlethreaded(&mut remote_requests.next()) {
                Some(Ok(avdtp::Request::Start { responder, stream_ids })) => {
                    for stream_id in stream_ids {
                        assert!(expected_starts.remove(&stream_id));
                    }
                    responder.send().expect("send response okay");
                }
                x => panic!("Expected start and got {:?}", x),
            }
        }
        // And we should start two media tasks.

        let one_media_task = test_builder.expect_task();
        assert!(one_media_task.is_started());
        let two_media_task = match exec.run_until_stalled(&mut test_builder.next_task()) {
            Poll::Ready(Some(task)) => task,
            x => panic!("Expected another ready task but {x:?}"),
        };
        assert!(two_media_task.is_started());
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    fn permits_can_be_revoked_one_at_a_time(transport_mode: Transport) {
        let mut exec = fasync::TestExecutor::new();

        let mut streams = Streams::default();
        let mut test_builder = TestMediaTaskBuilder::new();
        streams.insert(Stream::build(
            make_sbc_endpoint(1, avdtp::EndpointType::Sink),
            test_builder.builder(),
        ));
        let sbc_endpoint_id = 1_u8.try_into().unwrap();
        let remote_sbc_endpoint_id = 7_u8.try_into().unwrap();

        streams.insert(Stream::build(
            make_sbc_endpoint(2, avdtp::EndpointType::Sink),
            test_builder.builder(),
        ));
        let sbc2_endpoint_id = 2_u8.try_into().unwrap();
        let remote_sbc2_endpoint_id = 6_u8.try_into().unwrap();

        let permits = Permits::new(2);

        let (remote, _requests, _, peer) =
            setup_test_peer(transport_mode, false, streams, Some(permits.clone()));
        let remote_peer = avdtp::Peer::new(remote);

        let one_media_task = start_sbc_stream(
            &mut exec,
            &mut test_builder,
            &peer,
            &remote_peer,
            &sbc_endpoint_id,
            &remote_sbc_endpoint_id,
            transport_mode,
        );
        let two_media_task = start_sbc_stream(
            &mut exec,
            &mut test_builder,
            &peer,
            &remote_peer,
            &sbc2_endpoint_id,
            &remote_sbc2_endpoint_id,
            transport_mode,
        );

        // Someone comes along and revokes one of our permits.
        let taken_permit = permits.take();

        let remote_endpoints: HashSet<_> =
            [&remote_sbc_endpoint_id, &remote_sbc2_endpoint_id].iter().cloned().collect();

        // We should send a suspend to the other end, for both of them.
        let mut remote_requests = remote_peer.take_request_stream();
        let suspended_id = match exec.run_until_stalled(&mut remote_requests.next()) {
            Poll::Ready(Some(Ok(avdtp::Request::Suspend { responder, stream_ids }))) => {
                assert!(stream_ids.len() == 1);
                assert!(remote_endpoints.contains(&stream_ids[0]));
                responder.send().expect("send response okay");
                stream_ids[0].clone()
            }
            x => panic!("Expected suspension and got {:?}", x),
        };

        // And the correct one of the media tasks should be stopped.
        if suspended_id == remote_sbc_endpoint_id {
            assert!(!one_media_task.is_started());
            assert!(two_media_task.is_started());
        } else {
            assert!(one_media_task.is_started());
            assert!(!two_media_task.is_started());
        }

        // After the permits are available again, we send a start, and start the media stream.
        drop(taken_permit);

        match exec.run_singlethreaded(&mut remote_requests.next()) {
            Some(Ok(avdtp::Request::Start { responder, stream_ids })) => {
                assert_eq!(stream_ids, &[suspended_id]);
                responder.send().expect("send response okay");
            }
            x => panic!("Expected start and got {:?}", x),
        }
        // And we should start another media task.
        let media_task = match exec.run_until_stalled(&mut test_builder.next_task()) {
            Poll::Ready(Some(task)) => task,
            x => panic!("Expected media task to start: {x:?}"),
        };
        assert!(media_task.is_started());
    }

    // Scenario: when we are waiting for a suspend response from the peer after a permit was not
    // available, we try to start the peer (because a dwell has expired)
    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    fn permit_suspend_start_while_suspending(transport_mode: Transport) {
        let mut exec = fasync::TestExecutor::new();

        let mut streams = Streams::default();
        let mut test_builder = TestMediaTaskBuilder::new();
        streams.insert(Stream::build(
            make_sbc_endpoint(1, avdtp::EndpointType::Sink),
            test_builder.builder(),
        ));
        streams.insert(Stream::build(
            make_sbc_endpoint(2, avdtp::EndpointType::Sink),
            test_builder.builder(),
        ));
        let mut next_task_fut = test_builder.next_task();

        let permits = Permits::new(1);
        let (remote, _profile_request_stream, _, peer) =
            setup_test_peer(transport_mode, false, streams, Some(permits.clone()));

        let remote_peer = avdtp::Peer::new(remote);
        let mut remote_requests = remote_peer.take_request_stream();

        let sbc_endpoint_id = 1_u8.try_into().unwrap();

        let sbc_caps = sbc_capabilities();
        let mut set_config_fut =
            remote_peer.set_configuration(&sbc_endpoint_id, &sbc_endpoint_id, &sbc_caps);

        match exec.run_until_stalled(&mut set_config_fut) {
            Poll::Ready(Ok(())) => {}
            x => panic!("Set capabilities should be ready but got {:?}", x),
        };

        let mut open_fut = remote_peer.open(&sbc_endpoint_id);
        match exec.run_until_stalled(&mut open_fut) {
            Poll::Ready(Ok(())) => {}
            x => panic!("Open should be ready but got {:?}", x),
        };

        // Establish a media transport stream
        let (_remote_transport, transport) = Channel::create_socket_pair();
        assert_eq!(Some(()), peer.receive_channel(transport).ok());

        // At this point, we are dwelling, waiting for the peer to start the stream. Skip the timer.
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());
        let Some(_deadline) = exec.wake_next_timer() else {
            panic!("Expected a timer to be waiting to run");
        };

        // We will try to start it ourselves, which will take the only permit and send a start.
        let start_responder = match exec.run_singlethreaded(&mut remote_requests.next()) {
            Some(Ok(avdtp::Request::Start { stream_ids, responder })) => {
                assert_eq!(stream_ids, vec![sbc_endpoint_id.clone()]);
                responder
            }
            x => panic!("Expected a Start request, got {x:?}"),
        };

        assert!(permits.get().is_none());

        // The peer doesn't notice. Instead try to start it from the peer side (bad timing)
        let mut start_fut = remote_peer.start(&[sbc_endpoint_id.clone()]);

        // We get an OK, and then immediately a suspend request because there are no
        // permits available.
        match exec.run_singlethreaded(&mut start_fut) {
            Ok(()) => {}
            x => panic!("Expected OK response from start future but got {x:?}"),
        }

        let suspend_responder = match exec.run_singlethreaded(&mut remote_requests.next()) {
            Some(Ok(avdtp::Request::Suspend { stream_ids, responder })) => {
                assert_eq!(stream_ids, vec![sbc_endpoint_id.clone()]);
                responder
            }
            x => panic!("Expected a suspend got {x:?}"),
        };

        // At this point, the peer notices the start request and responds.
        start_responder.send().unwrap();

        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());

        // Okay I guess..
        suspend_responder.send().unwrap();

        // And we should start a task.
        let media_task = match exec.run_until_stalled(&mut next_task_fut) {
            Poll::Ready(Some(task)) => task,
            x => panic!("Local task should be created at this point: {:?}", x),
        };

        assert!(media_task.is_started());
    }

    /// Test that the version check method correctly differentiates between newer
    /// and older A2DP versions.
    #[fuchsia::test]
    fn version_check() {
        let p1: ProfileDescriptor = ProfileDescriptor {
            profile_id: Some(ServiceClassProfileIdentifier::AdvancedAudioDistribution),
            major_version: Some(1),
            minor_version: Some(3),
            ..Default::default()
        };
        assert_eq!(true, a2dp_version_check(p1));

        let p1: ProfileDescriptor = ProfileDescriptor {
            profile_id: Some(ServiceClassProfileIdentifier::AdvancedAudioDistribution),
            major_version: Some(2),
            minor_version: Some(10),
            ..Default::default()
        };
        assert_eq!(true, a2dp_version_check(p1));

        let p1: ProfileDescriptor = ProfileDescriptor {
            profile_id: Some(ServiceClassProfileIdentifier::AdvancedAudioDistribution),
            major_version: Some(1),
            minor_version: Some(0),
            ..Default::default()
        };
        assert_eq!(false, a2dp_version_check(p1));

        let p1: ProfileDescriptor = ProfileDescriptor {
            profile_id: Some(ServiceClassProfileIdentifier::AdvancedAudioDistribution),
            major_version: None,
            minor_version: Some(9),
            ..Default::default()
        };
        assert_eq!(false, a2dp_version_check(p1));

        let p1: ProfileDescriptor = ProfileDescriptor {
            profile_id: Some(ServiceClassProfileIdentifier::AdvancedAudioDistribution),
            major_version: Some(2),
            minor_version: Some(2),
            ..Default::default()
        };
        assert_eq!(true, a2dp_version_check(p1));
    }

    fn setup_test_peer_with_avrcp(
        transport: Transport,
        streams: Streams,
        avrcp: Option<avrcp::PeerManagerProxy>,
    ) -> (Channel, ProfileRequestStream, Peer) {
        let (avdtp, remote) = setup_avdtp_peer(transport);
        let metrics_logger = bt_metrics::MetricsLogger::default();
        let (profile_proxy, requests) = create_proxy_and_stream::<ProfileMarker>();
        let peer =
            Peer::create(PeerId(1), avdtp, streams, None, profile_proxy, avrcp, metrics_logger);

        (remote, requests, peer)
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn test_volume_relay_starts_on_receive_channel_for_source(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (avrcp_proxy, mut avrcp_stream) =
            fidl::endpoints::create_proxy_and_stream::<avrcp::PeerManagerMarker>();

        let mut streams = Streams::default();
        let test_builder = TestMediaTaskBuilder::new_inactive();
        streams.insert(Stream::build(
            make_sbc_endpoint(1, avdtp::EndpointType::Source),
            test_builder.builder(),
        ));

        let (remote, _requests, peer) =
            setup_test_peer_with_avrcp(transport, streams, Some(avrcp_proxy));
        let remote_peer = avdtp::Peer::new(remote);

        let sbc_endpoint_id = 1_u8.try_into().expect("should be able to get sbc endpointid");
        let sbc_caps = sbc_capabilities();

        // 1. Remote peer configures the stream.
        let set_config_fut =
            remote_peer.set_configuration(&sbc_endpoint_id, &sbc_endpoint_id, &sbc_caps);
        let mut set_config_fut = pin!(set_config_fut);
        match exec.run_until_stalled(&mut set_config_fut) {
            Poll::Ready(Ok(())) => {}
            x => panic!("Set capabilities should be ready but got {:?}", x),
        }

        // 2. Remote peer opens the stream.
        let open_fut = remote_peer.open(&sbc_endpoint_id);
        let mut open_fut = pin!(open_fut);
        match exec.run_until_stalled(&mut open_fut) {
            Poll::Ready(Ok(())) => {}
            x => panic!("Open should be ready but got {:?}", x),
        }

        // Verify that before receiving the channel, the volume relay task has not started.
        assert!(peer.inner.lock().volume_relay_task.is_none());

        // 3. Establish the media transport channel.
        let (transport_chan, _remote_transport) = create_test_channels(transport);
        assert_eq!(Some(()), peer.receive_channel(transport_chan).ok());

        // Verify that the volume relay task starts!
        assert!(peer.inner.lock().volume_relay_task.is_some());

        // Verify that GetControllerForTarget request is sent on avrcp_stream.
        let mut get_controller_fut = avrcp_stream.select_next_some();
        let _controller_server = match exec.run_until_stalled(&mut get_controller_fut) {
            Poll::Ready(Ok(avrcp::PeerManagerRequest::GetControllerForTarget {
                peer_id: req_peer_id,
                client,
                responder,
            })) => {
                assert_eq!(req_peer_id, PeerId(1).into());
                responder.send(Ok(())).expect("should send response");
                client
            }
            x => panic!("Expected GetControllerForTarget request, got {:?}", x),
        };
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    fn test_volume_relay_starts_on_stream_start(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (avrcp_proxy, mut avrcp_stream) =
            fidl::endpoints::create_proxy_and_stream::<avrcp::PeerManagerMarker>();

        let (mut remote, mut profile_request_stream, peer) =
            setup_test_peer_with_avrcp(transport, build_test_streams(), Some(avrcp_proxy));

        let remote_seid: StreamEndpointId = 2_u8.try_into().unwrap();
        let codec_params = ServiceCapability::MediaCodec {
            media_type: avdtp::MediaType::Audio,
            codec_type: avdtp::MediaCodecType::AUDIO_SBC,
            codec_extra: vec![0x11, 0x45, 51, 51],
        };

        let remote_endpoint = avdtp::StreamEndpoint::new(
            2,
            avdtp::MediaType::Audio,
            avdtp::EndpointType::Sink,
            vec![codec_params.clone()],
        )
        .expect("valid endpoint");
        peer.inner.lock().set_remote_endpoints(&[remote_endpoint]);

        // Before starting the stream, volume relay task should not be started.
        assert!(peer.inner.lock().volume_relay_task.is_none());

        let start_future = peer.stream_start(remote_seid, vec![codec_params]);
        let mut start_future = pin!(start_future);

        assert!(exec.run_until_stalled(&mut start_future).is_pending());
        receive_simple_accept(&mut exec, &mut remote, 0x03); // Set Configuration
        assert!(exec.run_until_stalled(&mut start_future).is_pending());
        receive_simple_accept(&mut exec, &mut remote, 0x06); // Open
        assert!(exec.run_until_stalled(&mut start_future).is_pending());

        // Respond to connect request for transport channel.
        let (transport_chan, _remote_transport) = create_test_channels(transport);
        let request = exec.run_until_stalled(&mut profile_request_stream.next());
        match request {
            Poll::Ready(Some(Ok(ProfileRequest::Connect {
                peer_id,
                connection: _,
                responder,
            }))) => {
                assert_eq!(PeerId(1), peer_id.into());
                let channel = transport_chan.try_into().unwrap();
                responder.send(Ok(channel)).expect("responder sends");
            }
            x => panic!("Expected Connect request, got {:?}", x),
        };

        // Setup finishes when the media transport is connected, then the stream is started by
        // the task that starts the stream when the audio is active.
        exec.run_until_stalled(&mut start_future)
            .expect("start setup finished")
            .expect("stream setup is ok");
        receive_simple_accept(&mut exec, &mut remote, 0x07); // Start
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());

        // Verify that GetControllerForTarget request is sent on avrcp_stream.
        let mut get_controller_fut = avrcp_stream.select_next_some();
        let request = loop {
            match exec.run_until_stalled(&mut get_controller_fut) {
                Poll::Ready(r) => break r,
                Poll::Pending => {
                    if exec.wake_next_timer().is_some() {
                        continue;
                    }
                    panic!("Expected GetControllerForTarget request, but executor stalled");
                }
            }
        };
        let _controller_server = match request {
            Ok(avrcp::PeerManagerRequest::GetControllerForTarget {
                peer_id: req_peer_id,
                client,
                responder,
            }) => {
                assert_eq!(req_peer_id, PeerId(1).into());
                responder.send(Ok(())).expect("should send response");
                client
            }
            x => panic!("Expected GetControllerForTarget request, got {:?}", x),
        };

        // Verify that the volume relay task started.
        assert!(peer.inner.lock().volume_relay_task.is_some());

        // Abort the stream to transition it to Idle: volume relay task should be stopped.
        peer.inner.lock().local.get_mut(&1_u8.try_into().unwrap()).unwrap().abort();
        PeerInner::maybe_start_volume_relay(&peer.inner);

        // Verify that the volume relay task is stopped.
        assert!(peer.inner.lock().volume_relay_task.is_none());
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn test_volume_relay_does_not_start_when_we_are_sink(transport: Transport) {
        let _exec = fasync::TestExecutor::new();
        let (avrcp_proxy, _avrcp_stream) =
            fidl::endpoints::create_proxy_and_stream::<avrcp::PeerManagerMarker>();

        // Set up streams with only a local Sink stream.
        let mut sink_streams = build_test_streams();
        let remote_id = 2_u8.try_into().unwrap();
        {
            let sink_endpoint =
                sink_streams.get_mut(&2_u8.try_into().unwrap()).unwrap().endpoint_mut();
            sink_endpoint
                .configure(&remote_id, vec![avdtp::ServiceCapability::MediaTransport])
                .unwrap();
            sink_endpoint.establish().unwrap();
            let (c1, _c2) = create_test_channels(transport);
            let _ = sink_endpoint.receive_channel(c1).unwrap();
        }

        let (_remote, _profile_stream, peer_sink) =
            setup_test_peer_with_avrcp(transport, sink_streams, Some(avrcp_proxy));

        // Trigger channel reception on peer, which internally calls maybe_start_volume_relay.
        let (c1, _c2) = create_test_channels(transport);
        let _ = peer_sink.receive_channel(c1);
        assert!(peer_sink.inner.lock().volume_relay_task.is_none());
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn test_volume_relay_restarts_if_task_finished(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (avrcp_proxy, mut avrcp_stream) =
            fidl::endpoints::create_proxy_and_stream::<avrcp::PeerManagerMarker>();

        let mut streams = Streams::default();
        let test_builder = TestMediaTaskBuilder::new();
        streams.insert(Stream::build(
            make_sbc_endpoint(1, avdtp::EndpointType::Source),
            test_builder.builder(),
        ));

        let (remote, _requests, peer) =
            setup_test_peer_with_avrcp(transport, streams, Some(avrcp_proxy));
        let remote_peer = avdtp::Peer::new(remote);

        let sbc_endpoint_id = 1_u8.try_into().expect("should be able to get sbc endpointid");
        let sbc_caps = sbc_capabilities();

        // 1. Remote peer configures the stream.
        let set_config_fut =
            remote_peer.set_configuration(&sbc_endpoint_id, &sbc_endpoint_id, &sbc_caps);
        let mut set_config_fut = pin!(set_config_fut);
        assert!(exec.run_until_stalled(&mut set_config_fut).is_ready());

        // 2. Remote peer opens the stream.
        let open_fut = remote_peer.open(&sbc_endpoint_id);
        let mut open_fut = pin!(open_fut);
        assert!(exec.run_until_stalled(&mut open_fut).is_ready());

        // 3. Establish transport channel -> starts volume relay task.
        let (_remote_transport, transport_chan) = create_test_channels(transport);
        assert_eq!(Some(()), peer.receive_channel(transport_chan).ok());
        assert!(peer.inner.lock().volume_relay_task.is_some());

        // 4. Handle AVRCP GetControllerForTarget and reply with Error (causing task to complete/exit).
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());
        let mut get_controller_fut = avrcp_stream.select_next_some();
        match exec.run_until_stalled(&mut get_controller_fut) {
            Poll::Ready(Ok(avrcp::PeerManagerRequest::GetControllerForTarget {
                peer_id: _,
                client: _,
                responder,
            })) => {
                responder.send(Err(zx::Status::INTERNAL.into_raw())).expect("should send response");
            }
            x => panic!("Expected GetControllerForTarget request, got {:?}", x),
        }

        // Run until volume relay task completes in the background.
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());

        // 5. Calling maybe_start_volume_relay should detect that the task finished, clear it, and spawn a new task.
        PeerInner::maybe_start_volume_relay(&peer.inner);
        assert!(peer.inner.lock().volume_relay_task.is_some());

        // Verify a new GetControllerForTarget request was sent.
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());
        let mut get_controller_fut = avrcp_stream.select_next_some();
        match exec.run_until_stalled(&mut get_controller_fut) {
            Poll::Ready(Ok(avrcp::PeerManagerRequest::GetControllerForTarget {
                peer_id: req_peer_id,
                client: _,
                responder,
            })) => {
                assert_eq!(req_peer_id, PeerId(1).into());
                responder.send(Ok(())).expect("should send response");
            }
            x => panic!("Expected second GetControllerForTarget request, got {:?}", x),
        }
    }
}
