// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl::endpoints::RequestStream;
use fidl_fuchsia_bluetooth_avrcp::*;
use fidl_fuchsia_bluetooth_avrcp_test::*;
use fuchsia_async as fasync;
use futures::future::{FutureExt, TryFutureExt};
use futures::stream::StreamExt;
use log::{debug, error, warn};
use std::collections::VecDeque;

use crate::packets::PlaybackStatus as PacketPlaybackStatus;
use crate::peer::{Controller, ControllerEvent as PeerControllerEvent};
use crate::types::PeerError;

impl From<PeerError> for ControllerError {
    fn from(e: PeerError) -> Self {
        match e {
            PeerError::PacketError(_) => ControllerError::PacketEncoding,
            PeerError::AvctpError(_) => ControllerError::ProtocolError,
            PeerError::RemoteNotFound => ControllerError::RemoteNotConnected,
            PeerError::CommandNotSupported => ControllerError::CommandNotImplemented,
            PeerError::ConnectionFailure(_) => ControllerError::ConnectionError,
            PeerError::UnexpectedResponse => ControllerError::UnexpectedResponse,
            _ => ControllerError::UnknownFailure,
        }
    }
}

/// FIDL wrapper for a internal PeerController for control-related tasks.
pub struct ControllerService {
    /// Handle to internal controller client for the remote peer.
    controller: Controller,

    /// Incoming FIDL request stream from the FIDL client.
    fidl_stream: ControllerRequestStream,

    /// List of subscribed notifications the FIDL controller client cares about.
    notification_filter: Notifications,

    /// The current count of outgoing notifications currently outstanding an not acknowledged by the
    /// FIDL client.
    /// Used as part of flow control for delivery of notifications to the client.
    notification_window_counter: u32,

    /// Current queue of outstanding notifications not received by the client. Used as part of flow
    /// control.
    // At some point this may change where we consolidate outgoing events if the FIDL client
    // can't keep up and falls behind instead of keeping a queue.
    notification_queue: VecDeque<(i64, PeerControllerEvent)>,

    /// Notification state cache. Current interim state for the remote target peer. Sent to the
    /// controller FIDL client when they set their notification filter.
    notification_state: Notification,

    /// Notification state last update timestamp.
    notification_state_timestamp: i64,
}

impl ControllerService {
    const EVENT_WINDOW_LIMIT: u32 = 3;

    pub fn new(controller: Controller, fidl_stream: ControllerRequestStream) -> Self {
        Self {
            controller,
            fidl_stream,
            notification_filter: Notifications::empty(),
            notification_window_counter: 0,
            notification_queue: VecDeque::new(),
            notification_state: Notification::default(),
            notification_state_timestamp: 0,
        }
    }

    fn browse_to_controller_error(src: BrowseControllerError) -> ControllerError {
        match src {
            BrowseControllerError::UnknownFailure => ControllerError::UnknownFailure,
            BrowseControllerError::TimedOut => ControllerError::TimedOut,
            BrowseControllerError::RemoteNotConnected => ControllerError::RemoteNotConnected,
            BrowseControllerError::CommandNotImplemented => ControllerError::CommandNotImplemented,
            BrowseControllerError::CommandRejected => ControllerError::CommandRejected,
            BrowseControllerError::CommandUnexpected => ControllerError::CommandUnexpected,
            BrowseControllerError::PacketEncoding => ControllerError::PacketEncoding,
            BrowseControllerError::ProtocolError => ControllerError::ProtocolError,
            BrowseControllerError::ConnectionError => ControllerError::ConnectionError,
            _ => ControllerError::CommandRejected,
        }
    }

    async fn handle_fidl_request(&mut self, request: ControllerRequest) -> Result<(), Error> {
        match request {
            ControllerRequest::GetPlayerApplicationSettings { attribute_ids, responder } => {
                let ids = attribute_ids.into_iter().map(Into::into).collect();
                match self.controller.get_player_application_settings(ids).await {
                    Ok(response) => responder.send(Ok(&response.into())),
                    Err(e) => responder.send(Err(e.into())),
                }?;
            }
            ControllerRequest::SetPlayerApplicationSettings { requested_settings, responder } => {
                let settings = crate::packets::PlayerApplicationSettings::from(&requested_settings);
                match self.controller.set_player_application_settings(settings).await {
                    Ok(response) => responder.send(Ok(&response.into())),
                    Err(e) => responder.send(Err(e.into())),
                }?;
            }
            ControllerRequest::GetMediaAttributes { responder } => {
                if self.controller.get_browsed_player().is_err() {
                    match self.controller.get_media_attributes().await {
                        Ok(r) => return Ok(responder.send(Ok(&r))?),
                        Err(e) => return Ok(responder.send(Err(e.into()))?),
                    }
                }

                let result = if let Some(id) = self.notification_state.track_id {
                    self.controller
                        .get_item_attributes(id)
                        .await
                        .map_err(Self::browse_to_controller_error)
                } else {
                    error!(
                        "Cannot get media attributes because currently playing track UID is unknown."
                    );
                    Err(ControllerError::InvalidArguments)
                };
                match result {
                    Ok(r) => responder.send(Ok(&r)),
                    Err(e) => responder.send(Err(e.into())),
                }?;
            }
            ControllerRequest::GetPlayStatus { responder } => {
                match self.controller.get_play_status().await {
                    Ok(response) => responder.send(Ok(&response)),
                    Err(e) => responder.send(Err(e.into())),
                }?;
            }
            ControllerRequest::InformBatteryStatus { battery_status, responder } => {
                responder.send(
                    self.controller
                        .inform_battery_status(battery_status)
                        .await
                        .map_err(ControllerError::from),
                )?;
            }
            ControllerRequest::SetNotificationFilter {
                notifications,
                // TODO(https://fxbug.dev/42120735): coalesce position change intervals and notify on schedule
                position_change_interval: _,
                control_handle: _,
            } => {
                self.notification_filter = notifications;
                self.send_notification_cache()?;
            }
            ControllerRequest::NotifyNotificationHandled { control_handle: _ } => {
                debug_assert!(self.notification_window_counter != 0);
                self.notification_window_counter -= 1;
                if self.notification_window_counter < Self::EVENT_WINDOW_LIMIT {
                    match self.notification_queue.pop_front() {
                        Some((timestamp, event)) => {
                            self.handle_controller_event(timestamp, event).await?;
                        }
                        None => {}
                    }
                }
            }
            ControllerRequest::SetAddressedPlayer { player_id: _, responder } => {
                responder.send(Err(ControllerError::CommandNotImplemented))?;
            }
            ControllerRequest::SetAbsoluteVolume { requested_volume, responder } => {
                responder.send(
                    self.controller
                        .set_absolute_volume(requested_volume)
                        .await
                        .map_err(ControllerError::from),
                )?;
            }
            ControllerRequest::SendCommand { command, responder } => {
                responder.send(
                    self.controller
                        .send_keypress(command.into_primitive())
                        .await
                        .map_err(ControllerError::from),
                )?;
            }
        };
        Ok(())
    }

    fn update_notification_from_controller_event(
        notification: &mut Notification,
        event: &PeerControllerEvent,
    ) {
        match event {
            PeerControllerEvent::PlaybackStatusChanged(playback_status) => {
                notification.status = Some(match playback_status {
                    PacketPlaybackStatus::Stopped => PlaybackStatus::Stopped,
                    PacketPlaybackStatus::Playing => PlaybackStatus::Playing,
                    PacketPlaybackStatus::Paused => PlaybackStatus::Paused,
                    PacketPlaybackStatus::FwdSeek => PlaybackStatus::FwdSeek,
                    PacketPlaybackStatus::RevSeek => PlaybackStatus::RevSeek,
                    PacketPlaybackStatus::Error => PlaybackStatus::Error,
                });
            }
            PeerControllerEvent::TrackIdChanged(track_id) => {
                notification.track_id = Some(*track_id);
            }
            PeerControllerEvent::PlaybackPosChanged(pos) => {
                notification.pos = Some(*pos);
            }
            PeerControllerEvent::VolumeChanged(volume) => {
                notification.volume = Some(*volume);
            }
            PeerControllerEvent::AvailablePlayersChanged => {
                notification.available_players_changed = Some(true)
            }
            PeerControllerEvent::AddressedPlayerChanged(player_id) => {
                notification.addressed_player = Some(*player_id);
            }
        }
    }

    async fn handle_controller_event(
        &mut self,
        timestamp: i64,
        event: PeerControllerEvent,
    ) -> Result<(), Error> {
        match event {
            PeerControllerEvent::AvailablePlayersChanged => {
                // TODO(https://fxbug.dev/42081024): get all the available players instead of just 10.
                if let Err(e) = self.controller.get_media_player_items(0, 9, true).await {
                    debug!(e:?; "Failed to get updated media player items");
                }
            }
            PeerControllerEvent::AddressedPlayerChanged(id) => {
                if let Err(e) = self.controller.set_browsed_player(id).await {
                    debug!(e:?; "Failed to set browsed player to addressed player {id:?}. Clearing previously set browsed player");
                }
            }
            _ => {}
        };
        self.notification_window_counter += 1;
        let control_handle: ControllerControlHandle = self.fidl_stream.control_handle();
        let mut notification = Notification::default();
        Self::update_notification_from_controller_event(&mut notification, &event);
        control_handle.send_on_notification(timestamp, &notification).map_err(Error::from)
    }

    fn cache_controller_notification_state(&mut self, event: &PeerControllerEvent) {
        self.notification_state_timestamp = fuchsia_runtime::utc_time().into_nanos();
        Self::update_notification_from_controller_event(&mut self.notification_state, &event);
    }

    fn send_notification_cache(&mut self) -> Result<(), Error> {
        if self.notification_state_timestamp > 0 {
            let control_handle: ControllerControlHandle = self.fidl_stream.control_handle();

            let mut notification = Notification::default();

            if self.notification_filter.contains(Notifications::PLAYBACK_STATUS) {
                notification.status = self.notification_state.status;
            }

            if self.notification_filter.contains(Notifications::TRACK) {
                notification.track_id = self.notification_state.track_id;
            }

            if self.notification_filter.contains(Notifications::TRACK_POS) {
                notification.pos = self.notification_state.pos;
            }

            if self.notification_filter.contains(Notifications::VOLUME) {
                notification.volume = self.notification_state.volume;
            }

            if self.notification_filter.contains(Notifications::AVAILABLE_PLAYERS) {
                notification.available_players_changed =
                    self.notification_state.available_players_changed;
            }

            self.notification_window_counter += 1;
            return control_handle
                .send_on_notification(self.notification_state_timestamp, &notification)
                .map_err(Error::from);
        }
        Ok(())
    }

    /// Returns true if the event should be dispatched.
    fn filter_controller_event(&self, event: &PeerControllerEvent) -> bool {
        match *event {
            PeerControllerEvent::PlaybackStatusChanged(_) => {
                self.notification_filter.contains(Notifications::PLAYBACK_STATUS)
            }
            PeerControllerEvent::TrackIdChanged(_) => {
                self.notification_filter.contains(Notifications::TRACK)
            }
            PeerControllerEvent::PlaybackPosChanged(_) => {
                self.notification_filter.contains(Notifications::TRACK_POS)
            }
            PeerControllerEvent::VolumeChanged(_) => {
                self.notification_filter.contains(Notifications::VOLUME)
            }
            PeerControllerEvent::AvailablePlayersChanged => {
                self.notification_filter.contains(Notifications::AVAILABLE_PLAYERS)
            }
            PeerControllerEvent::AddressedPlayerChanged(_) => {
                self.notification_filter.contains(Notifications::ADDRESSED_PLAYER)
            }
        }
    }

    pub async fn run(&mut self) -> Result<(), Error> {
        let mut controller_events = self.controller.add_event_listener();
        loop {
            futures::select! {
                req = self.fidl_stream.select_next_some() => {
                    self.handle_fidl_request(req?).await?;
                }
                event = controller_events.select_next_some() => {
                    self.cache_controller_notification_state(&event);
                    if self.filter_controller_event(&event) {
                        let timestamp = fuchsia_runtime::utc_time().into_nanos();
                        if self.notification_window_counter > Self::EVENT_WINDOW_LIMIT {
                            self.notification_queue.push_back((timestamp, event));
                        } else {
                            self.handle_controller_event(timestamp, event).await?;
                        }
                    }
                }
                complete => { return Ok(()); }
            }
        }
    }
}

/// FIDL wrapper for a internal PeerController for the test (ControllerExt) interface methods.
pub struct ControllerExtService {
    pub controller: Controller,
    pub fidl_stream: ControllerExtRequestStream,
}

impl ControllerExtService {
    async fn handle_fidl_request(&self, request: ControllerExtRequest) -> Result<(), Error> {
        match request {
            ControllerExtRequest::IsConnected { responder } => {
                responder.send(self.controller.is_control_connected())?;
            }
            ControllerExtRequest::GetEventsSupported { responder } => {
                let res = self.controller.get_supported_events().await;
                match res {
                    Ok(events) => responder.send(Ok(&events
                        .iter()
                        .filter_map(|e| NotificationEvent::from_primitive(u8::from(e)))
                        .collect::<Vec<_>>()))?,
                    Err(peer_error) => responder.send(Err(ControllerError::from(peer_error)))?,
                }
            }
            ControllerExtRequest::SendRawVendorDependentCommand { pdu_id, command, responder } => {
                responder.send(
                    match self.controller.send_raw_vendor_command(pdu_id, &command[..]).await {
                        Ok(ref response) => Ok(response),
                        Err(e) => Err(ControllerError::from(e)),
                    },
                )?;
            }
        };
        Ok(())
    }

    pub async fn run(&mut self) -> Result<(), Error> {
        loop {
            futures::select! {
                req = self.fidl_stream.select_next_some() => {
                    self.handle_fidl_request(req?).await?;
                }
                complete => { return Ok(()); }
            }
        }
    }
}

/// Spawns a future that facilitates communication between a PeerController and a FIDL client.
pub fn spawn_service(
    controller: Controller,
    fidl_stream: ControllerRequestStream,
) -> fasync::Task<()> {
    fasync::Task::spawn(
        async move {
            let mut acc = ControllerService::new(controller, fidl_stream);
            acc.run().await?;
            Ok(())
        }
        .boxed()
        .unwrap_or_else(|e: anyhow::Error| warn!("AVRCP client controller finished: {:?}", e)),
    )
}

/// Spawns a future that facilitates communication between a PeerController and a test FIDL client.
pub fn spawn_ext_service(
    controller: Controller,
    fidl_stream: ControllerExtRequestStream,
) -> fasync::Task<()> {
    fasync::Task::spawn(
        async move {
            let mut acc = ControllerExtService { controller, fidl_stream };
            let res = acc.run().await;
            res?;
            Ok(())
        }
        .boxed()
        .unwrap_or_else(|e: anyhow::Error| warn!("AVRCP test client controller finished: {:?}", e)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packets::*;
    use crate::peer::RemotePeerHandle;
    use crate::peer::tests::*;
    use crate::peer_manager::TargetDelegate;
    use assert_matches::assert_matches;
    use async_test_helpers::run_while;
    use async_utils::PollExt;
    use bt_avctp::{AvcPeer, AvctpPeer};
    use bt_channel_test_support::{Transport, create_test_channels};
    use fidl::endpoints::create_proxy_and_stream;
    use fidl_fuchsia_bluetooth_bredr::ProfileMarker;
    use fuchsia_bluetooth::profile::Psm;
    use fuchsia_bluetooth::profile::avrcp::{
        AvrcpProtocolVersion, AvrcpService, AvrcpTargetFeatures,
    };
    use fuchsia_bluetooth::types::PeerId;
    use packet_encoding::Decodable;
    use std::pin::pin;
    use std::sync::Arc;
    use test_case::test_case;

    /// Sets up control and browse connections for a peer acting as AVRCP Target role.
    fn set_up(transport: Transport) -> (Controller, AvcPeer, AvctpPeer) {
        let (profile_proxy, mut _profile_requests) = create_proxy_and_stream::<ProfileMarker>();
        let peer = RemotePeerHandle::spawn_peer(
            PeerId(0x1),
            Arc::new(TargetDelegate::new()),
            profile_proxy,
        );
        peer.set_target_descriptor(AvrcpService::Target {
            features: AvrcpTargetFeatures::CATEGORY1 | AvrcpTargetFeatures::SUPPORTSBROWSING,
            psm: Psm::AVCTP,
            protocol_version: AvrcpProtocolVersion(1, 6),
        });

        let (local_avc, remote_avc) = create_test_channels(transport);
        let (local_avctp, remote_avctp) = create_test_channels(transport);

        let remote_avc_peer = AvcPeer::new(remote_avc);
        let remote_avctp_peer = AvctpPeer::new(remote_avctp);

        peer.set_control_connection(AvcPeer::new(local_avc));
        peer.set_browse_connection(AvctpPeer::new(local_avctp));

        let controller = Controller::new(peer);

        (controller, remote_avc_peer, remote_avctp_peer)
    }

    /// Tests that the client stream handler will spawn a controller when a controller request
    /// successfully sets up a controller.
    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn run_client(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();

        // Set up for testing.
        let controller = set_up(transport).0;

        // Initialize client.
        let (proxy, server) = create_proxy_and_stream::<ControllerMarker>();
        let mut client = ControllerService::new(controller, server);
        let run_fut = pin!(client.run());

        // Verify that the client can process a request.
        let request_fut = proxy.set_addressed_player(1);
        let (_, mut run_fut) = run_while(&mut exec, run_fut, request_fut);

        // Verify that the client is still running.
        exec.run_until_stalled(&mut run_fut).expect_pending("should be pending");
    }

    /// Tests that the client stream handler will spawn a test controller when a
    /// controller request successfully sets up a controller.
    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn run_ext_client(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();

        // Set up testing.
        let controller = set_up(transport).0;

        // Initialize client.
        let (proxy, server) = create_proxy_and_stream::<ControllerExtMarker>();
        let mut client = ControllerExtService { controller, fidl_stream: server };

        let run_fut = pin!(client.run());

        // Verify that the client can process a request.
        let request_fut = proxy.is_connected();
        let (_, mut run_fut) = run_while(&mut exec, run_fut, request_fut);

        // Verify that the client is still running.
        exec.run_until_stalled(&mut run_fut).expect_pending("should be pending");
    }

    /// Tests that the notification object is updated based on the controller
    /// event value.
    #[fuchsia::test]
    fn update_notification_from_controller_event() {
        // Available players changed.
        let mut notification = Notification::default();
        let some_event = PeerControllerEvent::AvailablePlayersChanged;
        ControllerService::update_notification_from_controller_event(
            &mut notification,
            &some_event,
        );
        assert_eq!(
            notification,
            Notification { available_players_changed: Some(true), ..Default::default() }
        );

        // Addressed player changed.
        let mut notification = Notification::default();
        let some_event = PeerControllerEvent::AddressedPlayerChanged(4);
        ControllerService::update_notification_from_controller_event(
            &mut notification,
            &some_event,
        );
        assert_eq!(notification, Notification { addressed_player: Some(4), ..Default::default() });
    }

    /// Tests that the notification object is updated based on the controller
    /// event value.
    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn handle_controller_event(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();

        let (controller, remote_avc_peer, remote_avctp_peer) = set_up(transport);
        let mut avctp_cmd_stream = remote_avctp_peer.take_command_stream();
        let mut avc_cmd_stream = remote_avc_peer.take_command_stream();

        // Initially expect some outgoing commands as part of peer spawn.
        // Expect get folder items command with media player scope.
        expect_outgoing_commands(&mut exec, &mut avc_cmd_stream, &mut avctp_cmd_stream);

        // Initialize client.
        let (_proxy, server) = create_proxy_and_stream::<ControllerMarker>();
        let mut client = ControllerService::new(controller, server);

        // When available players changed notification is sent, available players are fetched.
        {
            let handle_available_players = client.handle_controller_event(
                fuchsia_runtime::utc_time().into_nanos(),
                PeerControllerEvent::AvailablePlayersChanged,
            );
            let mut handle_available_players = pin!(handle_available_players);
            exec.run_until_stalled(&mut handle_available_players)
                .expect_pending("should be pending");

            let command = get_next_avctp_command(&mut exec, &mut avctp_cmd_stream);
            let params = decode_avctp_command(&command, PduId::GetFolderItems);
            let cmd =
                GetFolderItemsCommand::decode(&params).expect("should have received valid command");
            assert_matches!(cmd.scope(), Scope::MediaPlayerList);
            let mock_resp = GetFolderItemsResponse::new_success(1, vec![]);
            send_avctp_response(PduId::GetFolderItems, &mock_resp, &command);

            let _ = exec.run_until_stalled(&mut handle_available_players).expect("should be ready");
        }

        // When addressed player is changed, browsed player is updated.
        {
            let handle_addressed_player = client.handle_controller_event(
                fuchsia_runtime::utc_time().into_nanos(),
                PeerControllerEvent::AddressedPlayerChanged(1004),
            );
            let mut handle_addressed_player = pin!(handle_addressed_player);
            exec.run_until_stalled(&mut handle_addressed_player)
                .expect_pending("should be pending");

            let command = get_next_avctp_command(&mut exec, &mut avctp_cmd_stream);
            let params = decode_avctp_command(&command, PduId::SetBrowsedPlayer);
            let cmd = SetBrowsedPlayerCommand::decode(&params)
                .expect("should have received valid command");
            assert_matches!(cmd.player_id(), 1004);
            let mock_resp =
                SetBrowsedPlayerResponse::new_success(0, 1, vec![]).expect("should not fail");
            send_avctp_response(PduId::SetBrowsedPlayer, &mock_resp, &command);

            let _ = exec.run_until_stalled(&mut handle_addressed_player).expect("should be ready");
        }
    }

    /// Tests that controller events are filtered based on the notification
    /// filter set on the server.
    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn filter_controller_event(transport: Transport) {
        let _exec = fasync::TestExecutor::new();

        // Set up for testing.
        let controller = set_up(transport).0;

        // Initialize client.
        let (_proxy, server) = create_proxy_and_stream::<ControllerMarker>();
        let mut client = ControllerService::new(controller, server);

        // Set test filter.
        client.notification_filter = Notifications::PLAYBACK_STATUS
            | Notifications::TRACK
            | Notifications::CONNECTION
            | Notifications::AVAILABLE_PLAYERS;

        // Since notification filter includes available players changed event, should have returned true.
        // {
        let included_event = PeerControllerEvent::AvailablePlayersChanged;
        assert!(client.filter_controller_event(&included_event));
        // }

        // Since notification filter does not include volume changed event, should have returned false.
        let excluded_event = PeerControllerEvent::VolumeChanged(2);
        assert!(!client.filter_controller_event(&excluded_event));
    }

    /// Tests getting media attributes through the control channel, which happens.
    /// when the browsed player is not set.
    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn get_media_attributes_control_channel(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();

        let (controller, remote_avc_peer, remote_avctp_peer) = set_up(transport);
        let mut avc_cmd_stream = remote_avc_peer.take_command_stream();
        let mut avctp_cmd_stream = remote_avctp_peer.take_command_stream();

        // Initially expect some outgoing commands as part of peer spawn.
        // Expect get folder items command with media player scope.
        expect_outgoing_commands(&mut exec, &mut avc_cmd_stream, &mut avctp_cmd_stream);

        // Initialize client.
        let (proxy, server) = create_proxy_and_stream::<ControllerMarker>();
        let mut client = ControllerService::new(controller, server);
        let run_fut = client.run();
        let mut run_fut = pin!(run_fut);
        exec.run_until_stalled(&mut run_fut).expect_pending("should be pending");

        // Verify that `GetElementAttributes` command is sent when browsed player is not set.
        let mut request_fut = proxy.get_media_attributes();
        exec.run_until_stalled(&mut run_fut).expect_pending("should be pending");
        exec.run_until_stalled(&mut request_fut).expect_pending("should be pending");

        let command = get_next_avc_command(&mut exec, &mut avc_cmd_stream);
        let (pdu_id, _) = decode_avc_vendor_command(&command).expect("should be ok");
        assert_eq!(pdu_id, PduId::GetElementAttributes);
    }

    /// Tests getting media attributes through the browse channel, which happens.
    /// when the browsed player and track ID are set.
    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn get_media_attributes_browse_channel(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();

        let (controller, remote_avc_peer, remote_avctp_peer) = set_up(transport);
        let mut avc_cmd_stream = remote_avc_peer.take_command_stream();
        let mut avctp_cmd_stream = remote_avctp_peer.take_command_stream();

        // Initially expect some outgoing commands as part of peer spawn.
        // Expect get folder items command with media player scope.
        expect_outgoing_commands(&mut exec, &mut avc_cmd_stream, &mut avctp_cmd_stream);

        // Set the browsed player for test.
        {
            let setup_fut = controller.set_browsed_player(1);
            let mut setup_fut = pin!(setup_fut);
            exec.run_until_stalled(&mut setup_fut).expect_pending("should be pending");
            let command = get_next_avctp_command(&mut exec, &mut avctp_cmd_stream);
            let _ = decode_avctp_command(&command, PduId::SetBrowsedPlayer);
            let mock_resp =
                SetBrowsedPlayerResponse::new_success(0, 0, vec![]).expect("should be ok");
            send_avctp_response(PduId::SetBrowsedPlayer, &mock_resp, &command);
            let _ = exec.run_until_stalled(&mut setup_fut).expect("should be ready");
        }

        // Initialize client.
        let (proxy, server) = create_proxy_and_stream::<ControllerMarker>();
        let mut client = ControllerService::new(controller, server);
        client.cache_controller_notification_state(&PeerControllerEvent::TrackIdChanged(2));
        let run_fut = client.run();
        let mut run_fut = pin!(run_fut);
        exec.run_until_stalled(&mut run_fut).expect_pending("should be pending");

        {
            // Verify that `GetElementAttributes` command is sent when browsed player is not set.
            let mut request_fut = proxy.get_media_attributes();
            exec.run_until_stalled(&mut run_fut).expect_pending("should be pending");
            exec.run_until_stalled(&mut request_fut).expect_pending("should be pending");

            let command = get_next_avctp_command(&mut exec, &mut avctp_cmd_stream);
            let _ = decode_avctp_command(&command, PduId::GetItemAttributes);
        }
    }
}
