// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::format_err;
use fuchsia_async as fasync;
use fuchsia_async::DurationExt;
use fuchsia_sync::RwLock;
use futures::future::FutureExt;
use futures::stream::{SelectAll, StreamExt, TryStreamExt};
use log::{debug, error, info, trace, warn};
use notification_stream::NotificationStream;
use packet_encoding::{Decodable, Encodable};
use std::collections::HashSet;
use std::sync::Arc;

mod notification_stream;

use crate::packets::{
    AddressedPlayerChangedNotificationResponse, AvailablePlayersChangedNotificationResponse,
    BrowseableItem, Error as PacketError, GetFolderItemsCommand, GetFolderItemsResponse,
    MediaPlayerItem, NotificationEventId, PduId, PlaybackPosChangedNotificationResponse,
    PlaybackStatusChangedNotificationResponse, SetBrowsedPlayerCommand, SetBrowsedPlayerResponse,
    TrackChangedNotificationResponse, VendorDependentPreamble, VolumeChangedNotificationResponse,
};
use crate::peer::{
    AVCTPConnectionType, BrowsablePlayer, ControllerEvent, MAX_CONNECTION_EST_TIME,
    MIN_CONNECTION_EST_TIME, PeerChannelState, RemotePeer, get_supported_events_internal,
    send_browse_command_internal,
};
use crate::profile::{CONTROLLER_SUPPORTED_FEATURES, TARGET_SUPPORTED_FEATURES};
use crate::types::PeerError as Error;
use fuchsia_bluetooth::profile::avrcp::{AvrcpControllerFeatures, AvrcpTargetFeatures};

async fn get_available_players(
    peer: Arc<RwLock<RemotePeer>>,
) -> Result<Vec<MediaPlayerItem>, Error> {
    // TODO(https://fxbug.dev/42081024): get all the available players instead of just 10.
    let command = GetFolderItemsCommand::new_media_player_list(0, 9);
    let mut payload = vec![0; command.encoded_len()];
    let _ = command.encode(&mut payload[..])?;
    let pdu_id = PduId::GetFolderItems;
    let peer_id = peer.read().id();

    debug!(peer_id:%; "Fetching available players for target peer");
    let resp_buf: Vec<u8> =
        { send_browse_command_internal(peer.clone(), u8::from(&pdu_id), &payload).await? };
    let response = GetFolderItemsResponse::decode(&resp_buf[..])?;
    match response {
        GetFolderItemsResponse::Failure(status) => {
            error!(peer_id:%, status:?; "GetFolderItems command failed");
            Err(Error::CommandFailed)
        }
        GetFolderItemsResponse::Success(r) => {
            let players = r
                .item_list()
                .into_iter()
                .map(|i: BrowseableItem| i.try_into_media_player())
                .collect::<Result<Vec<MediaPlayerItem>, PacketError>>()
                .map_err(|e| Error::PacketError(e))?;
            Ok(players)
        }
    }
}

// TODO(https://fxbug.dev/42056696): consider calling this function on available players
// changed notification event instead of on browse connection setup completion.
async fn set_browsed_player(peer: Arc<RwLock<RemotePeer>>) -> Result<(), Error> {
    if peer.read().target_descriptor.is_none() {
        return Ok(());
    }
    let players = get_available_players(peer.clone()).await?;
    let peer_id = peer.read().id();

    // Record player features.
    {
        let mut lock = peer.write();
        debug!(peer_id:%; "Recording player capabilities for target peer");
        lock.inspect.metrics().target_player_features(peer_id, &players);
        lock.update_available_players(&players);
    }

    if players.len() == 0 {
        return Err(Error::GenericError(format_err!("No available players")));
    }

    // Set browsed player to addressed player or player with highest browse support.
    let player_id = peer
        .read()
        .get_candidate_browse_player()
        .ok_or(Error::GenericError(format_err!("No player available for browsing")))?;
    let command = SetBrowsedPlayerCommand::new(player_id);
    let mut payload = vec![0; command.encoded_len()];
    let _ = command.encode(&mut payload[..])?;

    let resp_buf: Vec<u8> = {
        send_browse_command_internal(peer.clone(), u8::from(&PduId::SetBrowsedPlayer), &payload)
            .await?
    };
    let response = SetBrowsedPlayerResponse::decode(&resp_buf[..]);
    let response = response?;
    match response {
        SetBrowsedPlayerResponse::Success(params) => {
            info!(peer_id:%; "Setting initial browsable player to {player_id}");
            peer.write().browsable_player = Some(BrowsablePlayer::new(player_id, params));
            Ok(())
        }
        SetBrowsedPlayerResponse::Failure(status) => {
            error!(peer_id:%, status:?; "SetBrowsedPlayer command failed");
            Err(Error::CommandFailed)
        }
    }
}

/// Processes incoming commands from the control stream and dispatches them to the control command
/// handler. This is started only when we have a connection and when we have either a target or
/// controller SDP profile record for the current peer.
async fn process_control_stream(peer: Arc<RwLock<RemotePeer>>) {
    let (connection, peer_id) = {
        let peer_guard = peer.read();
        match peer_guard.control_channel.connection() {
            Some(connection) => (connection.clone(), peer_guard.id()),
            None => return,
        }
    };

    let command_stream = connection.take_command_stream();

    // Limit to 16 since that is the max number of transactions we can process at any one time per
    // AVCTP
    match command_stream
        .map_err(Into::into)
        .try_for_each_concurrent(16, |command| async {
            let fut = peer.read().control_command_handler.handle_command(command);
            fut.await
        })
        .await
    {
        Ok(_) => info!(peer_id:%; "Peer command stream closed"),
        Err(e) => error!(peer_id:%, e:?; "Peer command returned error"),
    }

    // Command stream closed/errored. Disconnect the peer.
    peer.write().reset_connections(false);
}

/// Processes incoming commands from the browse stream and dispatches them to a
/// browse channel handler.
async fn process_browse_stream(peer: Arc<RwLock<RemotePeer>>) {
    let (connection, peer_id) = {
        let peer_guard = peer.read();
        match peer_guard.browse_channel.connection() {
            Some(connection) => (connection.clone(), peer_guard.id()),
            None => return,
        }
    };

    let browse_command_stream = connection.take_command_stream();

    // Limit to 16 since that is the max number of transactions we can process at any one time per
    // AVCTP.
    match browse_command_stream
        .map_err(Into::into)
        .try_for_each_concurrent(16, |command| async {
            let fut = peer.read().browse_command_handler.handle_command(command);
            fut.await
        })
        .await
    {
        Ok(_) => info!(peer_id:%; "Peer browse command stream closed"),
        Err(e) => error!(peer_id:%, e:?; "Peer browse command stream returned error"),
    }

    // Browse channel closed or errored. Only reset the browse connection since the control channel can still exist.
    peer.write().reset_browse_connection(false);
}

/// Handles received notifications from the peer from the subscribed notifications streams and
/// dispatches the notifications back to the controller listeners.
/// Returns a boolean that indicates whether or not we should stop the notification
/// streaming. If the function encounters a notification event that it does not
/// recognize, it returns true to stop notification streaming.
fn handle_notification(
    notif: &NotificationEventId,
    peer: &Arc<RwLock<RemotePeer>>,
    data: &[u8],
) -> Result<bool, Error> {
    trace!("received notification for {:?} {:?}", notif, data);

    let preamble = VendorDependentPreamble::decode(data)?;

    let data = &data[preamble.encoded_len()..];

    if data.len() < preamble.parameter_length as usize {
        return Err(Error::PacketError(PacketError::InvalidMessageLength));
    }

    match notif {
        NotificationEventId::EventPlaybackStatusChanged => {
            let response = PlaybackStatusChangedNotificationResponse::decode(data)?;
            peer.write().handle_new_controller_notification_event(
                ControllerEvent::PlaybackStatusChanged(response.playback_status()),
            );
            Ok(false)
        }
        NotificationEventId::EventTrackChanged => {
            let response = TrackChangedNotificationResponse::decode(data)?;
            peer.write().handle_new_controller_notification_event(ControllerEvent::TrackIdChanged(
                response.identifier(),
            ));
            Ok(false)
        }
        NotificationEventId::EventPlaybackPosChanged => {
            let response = PlaybackPosChangedNotificationResponse::decode(data)?;
            peer.write().handle_new_controller_notification_event(
                ControllerEvent::PlaybackPosChanged(response.position()),
            );
            Ok(false)
        }
        NotificationEventId::EventVolumeChanged => {
            let response = VolumeChangedNotificationResponse::decode(data)?;
            peer.write().handle_new_controller_notification_event(ControllerEvent::VolumeChanged(
                response.volume(),
            ));
            Ok(false)
        }
        NotificationEventId::EventAvailablePlayersChanged => {
            let _ = AvailablePlayersChangedNotificationResponse::decode(data)?;
            peer.write()
                .handle_new_controller_notification_event(ControllerEvent::AvailablePlayersChanged);
            Ok(false)
        }
        NotificationEventId::EventAddressedPlayerChanged => {
            let response = AddressedPlayerChangedNotificationResponse::decode(data)?;
            peer.write().handle_new_controller_notification_event(
                ControllerEvent::AddressedPlayerChanged(response.player_id()),
            );
            Ok(false)
        }
        _ => Ok(true),
    }
}

/// Attempt an outgoing L2CAP connection to remote's AVRCP control channel.
/// TODO(https://fxbug.dev/42166696): Refactor logic into RemotePeer to avoid multiple lock accesses.
async fn make_connection(peer: Arc<RwLock<RemotePeer>>, conn_type: AVCTPConnectionType) {
    let random_delay: zx::MonotonicDuration =
        zx::MonotonicDuration::from_nanos(rand::random_range(
            MIN_CONNECTION_EST_TIME.into_nanos()..MAX_CONNECTION_EST_TIME.into_nanos(),
        ));
    debug!("AVRCP waiting {:?} millis before establishing connection", random_delay.into_millis());
    fuchsia_async::Timer::new(random_delay.after_now()).await;

    // While waiting to initiate an outgoing connection, if there was an incoming connection, don't
    // make the outgoing connection.
    {
        let mut lock = peer.write();
        match conn_type {
            AVCTPConnectionType::Control => {
                if lock.control_connected() {
                    return;
                }
                lock.control_channel.connecting();
            }
            AVCTPConnectionType::Browse => {
                if lock.browse_connected() {
                    return;
                }
                lock.browse_channel.connecting();
            }
        }
    }

    // Initiate outgoing connection.
    let peer_id = peer.read().peer_id;
    let conn_call = peer.write().connect(conn_type);

    match conn_call.await {
        Err(fidl_err) => {
            warn!("Profile service connect error for {peer_id}: {fidl_err:?}");
            peer.write().connect_failed(conn_type, true);
        }
        Ok(Ok(channel)) => match channel.try_into() {
            Ok(chan) => peer.write().connected(conn_type, chan),
            Err(e) => {
                error!("Can't make peer {peer_id} channel: {e:?}");
                peer.write().connect_failed(conn_type, true);
            }
        },
        Ok(Err(e)) => {
            error!(e:?, peer_id:%; "Couldn't connect to peer for {conn_type:?}");
            peer.write().connect_failed(conn_type, false);
        }
    }
}

/// Checks for supported notification on the peer and registers for notifications.
/// This is started on a remote peer when we have a connection and target profile descriptor.
async fn pump_notifications(peer: Arc<RwLock<RemotePeer>>) {
    // events we support when speaking to a peer that supports the target profile.
    const SUPPORTED_NOTIFICATIONS: [NotificationEventId; 6] = [
        NotificationEventId::EventPlaybackStatusChanged,
        NotificationEventId::EventTrackChanged,
        NotificationEventId::EventPlaybackPosChanged,
        NotificationEventId::EventVolumeChanged,
        NotificationEventId::EventAvailablePlayersChanged,
        NotificationEventId::EventAddressedPlayerChanged,
    ];

    let local_supported: HashSet<NotificationEventId> =
        SUPPORTED_NOTIFICATIONS.iter().cloned().collect();

    // look up what notifications we support on this peer first. Consider updating this from
    // time to time.
    let remote_supported = match get_supported_events_internal(peer.clone()).await {
        Ok(x) => x,
        Err(_) => return,
    };

    let supported_notifications = remote_supported.intersection(&local_supported);

    let mut notification_streams = SelectAll::new();

    for notif in supported_notifications {
        debug!("creating notification stream for {:?}", notif);
        let stream = NotificationStream::new(peer.clone(), notif.clone(), 1)
            .map_ok(move |data| (notif, data));
        notification_streams.push(stream);
    }

    while let Some(event_result) = notification_streams.next().await {
        match event_result.map(|(notif, data)| handle_notification(&notif, &peer, &data[..])) {
            Err(Error::CommandNotSupported) => continue,
            Err(_) => break,
            Ok(Err(e)) => {
                warn!("Error decoding packet from peer {:?}", e);
                break;
            }
            Ok(Ok(true)) => break,
            Ok(Ok(false)) => continue,
        }
    }
    debug!("stopping notifications for {:?}", peer.read().peer_id);
}

/// Starts a task to poll notifications on the remote peer. Aborted when the peer connection is
/// reset.
fn start_notifications_processing_task(peer: Arc<RwLock<RemotePeer>>) -> fasync::Task<()> {
    fasync::Task::spawn(pump_notifications(peer).map(|_| ()))
}

/// Starts a task to poll control messages from the peer. Aborted when the peer connection is
/// reset. Started when we have a connection to the remote peer and we have any type of valid SDP
/// profile from the peer.
fn start_control_stream_processing_task(peer: Arc<RwLock<RemotePeer>>) -> fasync::Task<()> {
    fasync::Task::spawn(process_control_stream(peer).map(|_| ()))
}

/// Starts a task to poll browse messages from the peer.
/// Started when we have a browse connection to the remote peer as well as an already
/// established control connection.
/// Aborted when the peer connection is reset.
fn start_browse_stream_processing_task(peer: Arc<RwLock<RemotePeer>>) -> fasync::Task<()> {
    fasync::Task::spawn(process_browse_stream(peer).map(|_| ()))
}

/// Spawns a task that:
/// - records metrics related to a peer
/// - sets browsed player for the peer
fn run_post_browse_setup_task(peer: Arc<RwLock<RemotePeer>>) -> fasync::Task<()> {
    fasync::Task::spawn(async move {
        if let Err(e) = set_browsed_player(peer).await {
            info!("Could not set browsed player: {:?}", e);
        }
    })
}

/// State observer task around a remote peer. Takes a change stream from the remote peer that wakes
/// the task whenever some state has changed on the peer. Swaps tasks such as making outgoing
/// connections, processing the incoming control messages, and registering for notifications on the
/// remote peer.
pub(super) async fn state_watcher(peer: Arc<RwLock<RemotePeer>>) {
    debug!("state_watcher starting");
    let mut change_stream = peer.read().state_change_listener.take_change_stream();
    let id = peer.read().id();
    let peer_weak = Arc::downgrade(&peer);
    drop(peer);

    let mut make_control_channel_task = fasync::Task::spawn(futures::future::ready(()));
    let mut make_browse_channel_task = fasync::Task::spawn(futures::future::ready(()));

    let mut control_channel_task: Option<fasync::Task<()>> = None;
    let mut browse_channel_task: Option<fasync::Task<()>> = None;
    let mut post_browse_setup_task: Option<fasync::Task<()>> = None;
    let mut notification_poll_task: Option<fasync::Task<()>> = None;

    while let Some(_) = change_stream.next().await {
        debug!("state_watcher command received");
        let peer = match peer_weak.upgrade() {
            Some(peer) => peer,
            None => break,
        };
        let mut peer_guard = peer.write();

        // The old tasks need to be cleaned up. Potentially terminate the channel
        // processing tasks and the notification processing task.
        // Reset the `cancel_{browse|control}_task` flags so that we don't fall
        // into a loop of constantly clearing the tasks.
        if peer_guard.cancel_browse_task {
            if post_browse_setup_task.take().is_some() {
                debug!("state_watcher: clearing previous post browse setup task.");
            }
            if browse_channel_task.take().is_some() {
                debug!("state_watcher: clearing previous browse channel task.");
            }
            peer_guard.cancel_browse_task = false;
        }
        if peer_guard.cancel_control_task {
            if control_channel_task.take().is_some() {
                debug!("state_watcher: clearing previous control channel task.");
            }
            notification_poll_task = None;
            peer_guard.cancel_control_task = false;
        }

        debug!("state_watcher control channel {:?}", peer_guard.control_channel);
        match peer_guard.control_channel.state() {
            &PeerChannelState::Connecting => {}
            &PeerChannelState::Disconnected => {
                // Have we discovered service profile data on the peer?
                if peer_guard.discovered() && peer_guard.attempt_control_connection {
                    debug!("Starting make_connection task for peer {}", id);
                    peer_guard.attempt_control_connection = false;
                    make_control_channel_task = fasync::Task::spawn(make_connection(
                        peer.clone(),
                        AVCTPConnectionType::Control,
                    ));
                }
            }
            &PeerChannelState::Connected(_) => {
                // If we have discovered profile data on this peer, start processing requests
                // over the control stream.
                if peer_guard.discovered() && control_channel_task.is_none() {
                    debug!("state_watcher: Starting control stream task for peer {}", id);
                    control_channel_task = Some(start_control_stream_processing_task(peer.clone()));
                }

                if peer_guard.target_descriptor.is_some() && notification_poll_task.is_none() {
                    notification_poll_task =
                        Some(start_notifications_processing_task(peer.clone()));
                }
            }
        }

        debug!("state_watcher browse channel {:?}", peer_guard.browse_channel);
        let target_supports_browsing = peer_guard.supports_target_browsing()
            && CONTROLLER_SUPPORTED_FEATURES.contains(AvrcpControllerFeatures::SUPPORTSBROWSING);
        let controller_supports_browsing = peer_guard.supports_controller_browsing()
            && TARGET_SUPPORTED_FEATURES.contains(AvrcpTargetFeatures::SUPPORTSBROWSING);

        match peer_guard.browse_channel.state() {
            &PeerChannelState::Connecting => {}
            &PeerChannelState::Disconnected => {
                browse_channel_task = None;
                if peer_guard.control_connected()
                    && (target_supports_browsing || controller_supports_browsing)
                    && peer_guard.attempt_browse_connection
                {
                    debug!("Starting make_connection task for browse channel on peer {}", id);
                    peer_guard.attempt_browse_connection = false;
                    make_browse_channel_task = fasync::Task::spawn(make_connection(
                        peer.clone(),
                        AVCTPConnectionType::Browse,
                    ));
                }
            }
            &PeerChannelState::Connected(_) => {
                // The Browse channel must be established after the control channel.
                // Ensure that the control channel exists before starting the browse task.
                if !peer_guard.control_connected() {
                    debug!("Received Browse connection before Control .. disconnecting");
                    peer_guard.browse_channel.disconnect();
                    continue;
                }
                if browse_channel_task.is_none() {
                    debug!("state_watcher: Starting browse stream task for peer {}", id);
                    browse_channel_task = Some(start_browse_stream_processing_task(peer.clone()));
                }

                post_browse_setup_task = Some(run_post_browse_setup_task(peer.clone()));
            }
        }
    }

    debug!("state_watcher shutting down. aborting processors");

    let _ = make_control_channel_task.abort();
    let _ = make_browse_channel_task.abort();

    // Stop processing state changes on the browse channel.
    // This needs to happen before stopping the control channel.
    drop(post_browse_setup_task.take());
    drop(browse_channel_task.take());

    // Stop processing state changes on the control channel.
    drop(control_channel_task.take());

    drop(notification_poll_task.take());
}
