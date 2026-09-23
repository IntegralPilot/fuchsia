// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use self::task::PeerTask;

use async_trait::async_trait;
use async_utils::channel::TrySend;
use bt_hfp::{audio, sco};
use core::pin::Pin;
use core::task::{Context, Poll};
use fidl::endpoints::ServerEnd;
use fidl_fuchsia_bluetooth_bredr::ProfileProxy;
use fidl_fuchsia_bluetooth_hfp::{PeerHandlerMarker, PeerHandlerProxy};
use fuchsia_async::Task;
use fuchsia_bluetooth::types::PeerId;
use fuchsia_inspect as inspect;
use fuchsia_sync::Mutex;
use futures::channel::mpsc;
use futures::{Future, FutureExt, SinkExt, TryFutureExt};
use profile_client::ProfileEvent;
use std::sync::Arc;

use crate::config::AudioGatewayFeatureSupport;
use crate::error::Error;
use crate::hfp;

#[cfg(test)]
use async_utils::event::{Event, EventWaitResult};

pub mod calls;
pub mod gain_control;
pub mod indicators;
pub mod procedure;
mod ringer;
pub mod service_level_connection;
pub mod slc_request;
mod task;
pub mod update;

/// A request made to the Peer that should be passed along to the PeerTask
#[derive(Debug)]
pub enum PeerRequest {
    Profile(ProfileEvent),
    Audio(audio::ControlEvent),
    #[allow(dead_code)]
    Handle(PeerHandlerProxy),
    ManagerConnected {
        id: hfp::ManagerConnectionId,
    },
    BatteryLevel(u8),
    Behavior(ConnectionBehavior),
    // Only constructed in test
    #[cfg_attr(not(test), allow(unused))]
    Shutdown,
}

impl From<ProfileEvent> for PeerRequest {
    fn from(event: ProfileEvent) -> Self {
        PeerRequest::Profile(event)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ConnectionBehavior {
    pub autoconnect: bool,
}

impl Default for ConnectionBehavior {
    fn default() -> Self {
        Self { autoconnect: true }
    }
}

impl From<fidl_fuchsia_bluetooth_hfp_test::ConnectionBehavior> for ConnectionBehavior {
    fn from(value: fidl_fuchsia_bluetooth_hfp_test::ConnectionBehavior) -> Self {
        let autoconnect = value.autoconnect.unwrap_or_else(|| Self::default().autoconnect);
        Self { autoconnect }
    }
}

/// Manages the Service Level Connection, Audio Connection, and FIDL APIs for a peer device.
#[async_trait]
pub trait Peer: Future<Output = PeerId> + Unpin + Send {
    #[allow(dead_code)]
    fn id(&self) -> PeerId;

    /// Pass a new profile event into the Peer. The Peer can then react to the event as it sees
    /// fit. This method will return once the peer accepts the event.
    async fn profile_event(&mut self, event: ProfileEvent) -> Result<(), Error>;

    /// Send a new audio event to the Peer.  The Peer can then react by performing some actions
    /// such as starting the audio connection or stopping it when an error occurs.
    /// The audio::ControlEvent should be for this peer (i.e. event.id() == self.id())
    async fn audio_event(&mut self, event: audio::ControlEvent) -> Result<(), Error>;

    /// Notify the `Peer` of a newly connected call manager.
    /// `id` is the unique identifier for the connection to this call manager.
    async fn call_manager_connected(&mut self, id: hfp::ManagerConnectionId) -> Result<(), Error>;

    /// Create a FIDL channel that can be used to manage this Peer and return the server end.
    ///
    /// Returns an error if the fidl endpoints cannot be built or the request cannot be processed
    /// by the Peer.
    #[allow(dead_code)]
    async fn build_handler(&mut self) -> Result<ServerEnd<PeerHandlerMarker>, Error>;

    /// Provide the `Peer` with the battery level of this device.
    /// `level` should be a value between 0-5 inclusive.
    async fn report_battery_level(&mut self, level: u8);

    /// Set the behavior used when connecting to remote peers.
    async fn set_connection_behavior(&mut self, behavior: ConnectionBehavior);
}

/// Concrete implementation for `Peer`.
pub struct PeerImpl {
    id: PeerId,
    local_config: AudioGatewayFeatureSupport,
    profile_proxy: ProfileProxy,
    connection_behavior: ConnectionBehavior,
    task: Task<()>,
    // A queue of all events destined for the peer.
    // Peer exposes methods for dealing with these various fidl requests in an easier fashion.
    // Under the hood, a queue is used to send these messages to the `task` which represents the
    // Peer.
    queue: mpsc::Sender<PeerRequest>,
    /// A handle to the audio control interface.
    audio_control: Arc<Mutex<Box<dyn audio::Control>>>,
    a2dp_control: bt_hfp::a2dp::Control,
    hfp_sender: mpsc::Sender<hfp::Event>,
    sco_connector: sco::Connector,
    /// The last call manager connected.  Used on re-connect to a peer
    last_call_manager: Option<hfp::ManagerConnectionId>,
    inspect_node: inspect::Node,
}

impl PeerImpl {
    pub fn new(
        id: PeerId,
        profile_proxy: ProfileProxy,
        audio_control: Arc<Mutex<Box<dyn audio::Control>>>,
        a2dp_control: bt_hfp::a2dp::Control,
        local_config: AudioGatewayFeatureSupport,
        connection_behavior: ConnectionBehavior,
        hfp_sender: mpsc::Sender<hfp::Event>,
        sco_connector: sco::Connector,
        inspect_node: inspect::Node,
    ) -> Result<Self, Error> {
        let (task, queue) = PeerTask::spawn(
            id,
            profile_proxy.clone(),
            audio_control.clone(),
            a2dp_control.clone(),
            local_config,
            connection_behavior,
            hfp_sender.clone(),
            sco_connector.clone(),
            &inspect_node,
        )?;
        Ok(Self {
            id,
            local_config,
            profile_proxy,
            task,
            audio_control,
            a2dp_control,
            queue,
            connection_behavior,
            hfp_sender,
            sco_connector,
            last_call_manager: None,
            inspect_node,
        })
    }

    /// Spawn a new peer task.
    fn spawn_task(&mut self) -> Result<(), Error> {
        let (task, queue) = PeerTask::spawn(
            self.id,
            self.profile_proxy.clone(),
            self.audio_control.clone(),
            self.a2dp_control.clone(),
            self.local_config,
            self.connection_behavior,
            self.hfp_sender.clone(),
            self.sco_connector.clone(),
            &self.inspect_node,
        )?;

        self.task = task;
        self.queue = queue;
        Ok(())
    }

    /// Method completes when a peer task accepts the request.
    ///
    /// Panics if the PeerTask was not able to receive the request.
    /// This should only be used when it is expected that the peer can receive the request and
    /// it is a critical failure when it does not receive the request.
    fn expect_send_request(&mut self, request: PeerRequest) -> impl Future<Output = ()> + '_ {
        self.queue
            .send(request)
            .unwrap_or_else(|e| panic!("PeerTask should be running and processing: got {:?}", e))
    }
}

#[async_trait]
impl Peer for PeerImpl {
    fn id(&self) -> PeerId {
        self.id
    }

    /// This method will panic if the peer cannot accept a profile event. This is not expected to
    /// happen under normal operation and likely indicates a bug or unrecoverable failure condition
    /// in the system.
    async fn profile_event(&mut self, event: ProfileEvent) -> Result<(), Error> {
        // The fuchsia.bluetooth.bredr Profile APIs ultimately control the creation of Peers.
        // Therefore, they will recreate the peer task if it is not running.
        if let Err(request) = self.queue.try_send_fut(PeerRequest::Profile(event)).await {
            // Task ended, so let's spin it back up since somebody wants it.
            self.spawn_task()?;
            // If a call manager is set and we respawned, send the connect message
            if let Some(id) = self.last_call_manager {
                self.call_manager_connected(id).await?;
            }
            self.expect_send_request(request).await;
        }
        Ok(())
    }

    /// This method will panic if the peer cannot accept an audio::ControlEvent. This is
    /// not expected to happen under normal operation and likely indicates a bug or unrecoverable
    /// failure condition in the system.
    async fn audio_event(&mut self, event: audio::ControlEvent) -> Result<(), Error> {
        if let Err(request) = self.queue.try_send_fut(PeerRequest::Audio(event)).await {
            // Task ended, so let's spin it back up since somebody wants it.
            self.spawn_task()?;
            // If a call manager is set and we respawned, send the connect message
            if let Some(id) = self.last_call_manager {
                self.call_manager_connected(id).await?;
            }
            self.expect_send_request(request).await;
        }
        Ok(())
    }

    /// This method will panic if the peer cannot accept a call manager connected event. This is
    /// not expected to happen under normal operation and likely indicates a bug or unrecoverable
    /// failure condition in the system.
    async fn call_manager_connected(&mut self, id: hfp::ManagerConnectionId) -> Result<(), Error> {
        self.last_call_manager = Some(id);
        if let Err(request) = self.queue.try_send_fut(PeerRequest::ManagerConnected { id }).await {
            // Task ended, so let's spin it back up since somebody wants it.
            self.spawn_task()?;
            self.expect_send_request(request).await;
        }
        Ok(())
    }
    async fn build_handler(&mut self) -> Result<ServerEnd<PeerHandlerMarker>, Error> {
        let (proxy, server_end) = fidl::endpoints::create_proxy();
        self.queue
            .try_send_fut(PeerRequest::Handle(proxy))
            .await
            .map_err(|_| Error::PeerRemoved)?;
        Ok(server_end)
    }

    async fn report_battery_level(&mut self, level: u8) {
        let _ = self.queue.try_send_fut(PeerRequest::BatteryLevel(level)).await;
    }

    async fn set_connection_behavior(&mut self, behavior: ConnectionBehavior) {
        self.connection_behavior = behavior;
        let _ = self.queue.try_send_fut(PeerRequest::Behavior(behavior)).await;
    }
}

impl Future for PeerImpl {
    type Output = PeerId;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.task.poll_unpin(cx).map(|()| self.id)
    }
}

#[cfg(test)]
pub(crate) mod fake {
    use super::*;

    /// Receives data from the fake peer's channel. Notifies PeerFake when dropped.
    pub struct PeerFakeReceiver {
        /// Use `receiver` to receive messages.
        pub receiver: mpsc::Receiver<PeerRequest>,
        _close: Event,
    }

    /// A fake Peer implementation which sends messages on the channel.
    /// PeerFake as a Future completes when PeerFakeReceiver is dropped.
    pub struct PeerFake {
        id: PeerId,
        queue: mpsc::Sender<PeerRequest>,
        closed: EventWaitResult,
    }

    impl PeerFake {
        pub fn new(id: PeerId) -> (PeerFakeReceiver, Self) {
            let (queue, receiver) = mpsc::channel(1);
            let close = Event::new();
            let closed = close.wait_or_dropped();
            (PeerFakeReceiver { receiver, _close: close }, Self { id, queue, closed })
        }

        async fn expect_send_request(&mut self, request: PeerRequest) {
            self.queue
                .send(request)
                .await
                .expect("PeerTask to be running and able to process requests");
        }
    }

    #[async_trait]
    impl Peer for PeerFake {
        fn id(&self) -> PeerId {
            self.id
        }

        async fn profile_event(&mut self, event: ProfileEvent) -> Result<(), Error> {
            self.expect_send_request(PeerRequest::Profile(event)).await;
            Ok(())
        }

        async fn audio_event(&mut self, event: audio::ControlEvent) -> Result<(), Error> {
            self.expect_send_request(PeerRequest::Audio(event)).await;
            Ok(())
        }

        async fn build_handler(&mut self) -> Result<ServerEnd<PeerHandlerMarker>, Error> {
            unimplemented!("Not needed for any currently written tests");
        }

        async fn call_manager_connected(
            &mut self,
            _: hfp::ManagerConnectionId,
        ) -> Result<(), Error> {
            unimplemented!("Not needed for any currently written tests");
        }

        async fn report_battery_level(&mut self, level: u8) {
            self.expect_send_request(PeerRequest::BatteryLevel(level)).await;
        }

        async fn set_connection_behavior(&mut self, behavior: ConnectionBehavior) {
            self.expect_send_request(PeerRequest::Behavior(behavior)).await;
        }
    }

    impl Future for PeerFake {
        type Output = PeerId;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            self.closed.poll_unpin(cx).map(|_| self.id)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use async_utils::PollExt;
    use bt_channel_test_support::{Transport, create_test_channels};
    use fidl_fuchsia_bluetooth_bredr::ProfileMarker;
    use fuchsia_async as fasync;
    use futures::StreamExt;
    use std::collections::HashSet;
    use std::pin::pin;
    use test_case::test_case;

    fn new_audio_control() -> Arc<Mutex<Box<dyn audio::Control>>> {
        Arc::new(Mutex::new(Box::new(audio::TestControl::default())))
    }

    fn make_peer(id: PeerId) -> (PeerImpl, mpsc::Receiver<hfp::Event>) {
        let proxy = fidl::endpoints::create_proxy_and_stream::<ProfileMarker>().0;
        let (send, recv) = mpsc::channel(1);
        let sco_connector = sco::Connector::build(proxy.clone(), HashSet::new());
        let audio_control = new_audio_control();
        let peer = PeerImpl::new(
            id,
            proxy,
            audio_control,
            bt_hfp::a2dp::Control::default(),
            AudioGatewayFeatureSupport::default(),
            ConnectionBehavior::default(),
            send,
            sco_connector,
            Default::default(),
        )
        .expect("valid peer");
        (peer, recv)
    }

    #[fuchsia::test]
    fn peer_id_returns_expected_id() {
        // TestExecutor must exist in order to create fidl endpoints
        let _exec = fasync::TestExecutor::new();

        let id = PeerId(1);
        let peer = make_peer(id).0;
        assert_eq!(peer.id(), id);
    }

    #[fuchsia::test]
    fn profile_event_request_respawns_task_successfully() {
        let mut exec = fasync::TestExecutor::new();

        let id = PeerId(1);
        let mut peer = make_peer(id).0;

        // Replace the task with a no-op task which will cancel the task.
        peer.task = fasync::Task::local(std::future::ready(()));

        // create profile_event_fut in a block to limit its lifetime
        {
            let event =
                ProfileEvent::SearchResult { id: PeerId(1), protocol: None, attributes: vec![] };
            let profile_event_fut = peer.profile_event(event);
            let mut profile_event_fut = pin!(profile_event_fut);
            exec.run_until_stalled(&mut profile_event_fut)
                .expect("Profile Event to complete")
                .expect("Profile Event to succeed");
        }

        // A new task has been spun up and is actively running.
        let task = std::mem::replace(&mut peer.task, fasync::Task::local(async move {}));
        let mut task = pin!(task);
        assert!(exec.run_until_stalled(&mut task).is_pending());
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn profile_event_request_resets_call_manager_when_respawning_task(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();

        let id = PeerId(1);
        let (mut peer, mut receiver) = make_peer(id);

        // Set up the call manager
        let sent_manager_id = 1.into();
        exec.run_singlethreaded(&mut peer.call_manager_connected(sent_manager_id))
            .expect("success");

        let (local, remote) = create_test_channels(transport);
        let event =
            ProfileEvent::PeerConnected { id: PeerId(1), protocol: vec![], channel: local.into() };
        exec.run_singlethreaded(peer.profile_event(event)).expect("success");

        // Should get PeerConnected with the right handler id.
        let message = exec.run_singlethreaded(&mut receiver.next());
        let Some(hfp::Event::PeerConnected { manager_id, handle, .. }) = message else {
            panic!("Expected PeerConnected after restarting task, got {message:?}");
        };
        assert_eq!(sent_manager_id, manager_id);
        drop(handle);

        // Close the remote connection, which should cause the peer task to stop.
        drop(remote);

        // Wait until it has fully stopped
        // The inner task is replaced by a no-op task so that it can be waited on.
        let mut task = std::mem::replace(&mut peer.task, fasync::Task::local(async move {}));
        exec.run_singlethreaded(&mut task);

        let (local, _remote) = create_test_channels(transport);
        let event =
            ProfileEvent::PeerConnected { id: PeerId(1), protocol: vec![], channel: local.into() };
        exec.run_singlethreaded(peer.profile_event(event)).expect("success");

        // The new task should notify the previously-connected CallManager of its
        // existence with a new peer when connected.
        let message = exec.run_singlethreaded(&mut receiver.next());
        let Some(hfp::Event::PeerConnected { manager_id, handle, .. }) = message else {
            panic!("Expected PeerConnected after restarting task, got {message:?}");
        };
        assert_eq!(sent_manager_id, manager_id);
        drop(handle);

        // A new task has been spun up and is actively running (the remote hasn't been dropped)
        let task = std::mem::replace(&mut peer.task, fasync::Task::local(async move {}));
        let mut task = pin!(task);
        assert!(exec.run_until_stalled(&mut task).is_pending());
    }

    #[fuchsia::test]
    fn manager_request_returns_error_when_task_is_stopped() {
        let mut exec = fasync::TestExecutor::new();

        let id = PeerId(1);
        let mut peer = make_peer(id).0;

        // Replace the task with a no-op task which will cancel the task.
        peer.task = fasync::Task::local(std::future::ready(()));

        let build_handler_fut = peer.build_handler();
        let mut build_handler_fut = pin!(build_handler_fut);
        let result = exec
            .run_until_stalled(&mut build_handler_fut)
            .expect("Manager handler registration to complete successfully");
        assert_matches!(result, Err(Error::PeerRemoved));
    }
}
