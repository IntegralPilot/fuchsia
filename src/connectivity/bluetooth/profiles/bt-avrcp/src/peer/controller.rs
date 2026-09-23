// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::format_err;
use fidl_fuchsia_bluetooth_avrcp as fidl_avrcp;
use futures::channel::mpsc;
use futures::{Future, StreamExt};
use log::debug;
use packet_encoding::{Decodable, Encodable};
use std::collections::HashSet;

use crate::packets::{Error as PacketError, *};
use crate::peer::{AVCTPConnectionType, BrowsablePlayer, RemotePeerHandle};
use crate::types::PeerError as Error;

#[derive(Debug, Clone)]
pub enum ControllerEvent {
    PlaybackStatusChanged(PlaybackStatus),
    TrackIdChanged(u64),
    PlaybackPosChanged(u32),
    VolumeChanged(u8),
    AvailablePlayersChanged,
    AddressedPlayerChanged(u16),
}

pub type ControllerEventStream = mpsc::Receiver<ControllerEvent>;

impl From<Error> for fidl_avrcp::BrowseControllerError {
    fn from(e: Error) -> Self {
        match e {
            Error::PacketError(_) => fidl_avrcp::BrowseControllerError::PacketEncoding,
            Error::AvctpError(_) => fidl_avrcp::BrowseControllerError::ProtocolError,
            Error::RemoteNotFound => fidl_avrcp::BrowseControllerError::RemoteNotConnected,
            Error::CommandNotSupported => fidl_avrcp::BrowseControllerError::CommandNotImplemented,
            Error::ConnectionFailure(_) => fidl_avrcp::BrowseControllerError::ConnectionError,
            _ => fidl_avrcp::BrowseControllerError::UnknownFailure,
        }
    }
}

fn into_media_element_item(
    x: fidl_avrcp::FileSystemItem,
) -> Result<fidl_avrcp::MediaElementItem, fidl_avrcp::BrowseControllerError> {
    match x {
        fidl_avrcp::FileSystemItem::MediaElement(m) => Ok(m),
        _ => Err(fidl_avrcp::BrowseControllerError::PacketEncoding),
    }
}

/// Controller interface for a remote peer returned by the PeerManager using the
/// ControllerRequest stream for a given ControllerRequest.
#[derive(Debug)]
pub struct Controller {
    peer: RemotePeerHandle,
}

impl Controller {
    pub(crate) fn new(peer: RemotePeerHandle) -> Controller {
        Controller { peer }
    }

    // Waits for the underlying peer control connection to be established for this controller.
    pub async fn wait_for_control_connection(&self) -> Result<(), Error> {
        if self.peer.is_control_connected() {
            return Ok(());
        }
        let (sender, mut receiver) = mpsc::unbounded();
        self.peer.add_connection_listener(sender);
        self.peer.request_control_connection();

        while let Some(connection) = receiver.next().await {
            match connection {
                (AVCTPConnectionType::Control, true) => return Ok(()),
                _ => {}
            }
        }
        Err(Error::ConnectionFailure(format_err!(
            "connection listener closed before control connection was completed"
        )))
    }

    // Waits for the underlying peer browse connection to be established for this controller.
    pub async fn wait_for_browse_connection(&self) -> Result<(), Error> {
        if self.peer.is_browse_connected() {
            return Ok(());
        }

        let (sender, mut receiver) = mpsc::unbounded();
        self.peer.add_connection_listener(sender);
        self.peer.request_browse_connection();

        while let Some(connection) = receiver.next().await {
            match connection {
                (AVCTPConnectionType::Browse, true) => return Ok(()),
                _ => {}
            }
        }
        Err(Error::ConnectionFailure(format_err!(
            "connection listener closed before browse connection was completed"
        )))
    }

    /// Sends a AVC key press and key release passthrough command.
    pub async fn send_keypress(&self, avc_keycode: u8) -> Result<(), Error> {
        {
            // key_press
            let payload_1 = &[avc_keycode, 0x00];
            let _ = self.peer.send_avc_passthrough(payload_1).await?;
        }
        {
            // key_release
            let payload_2 = &[avc_keycode | 0x80, 0x00];
            self.peer.send_avc_passthrough(payload_2).await
        }
    }

    /// Sends SetAbsoluteVolume command to the peer.
    /// Returns the volume as reported by the peer.
    pub async fn set_absolute_volume(&self, volume: u8) -> Result<u8, Error> {
        let cmd = SetAbsoluteVolumeCommand::new(volume)?;
        debug!("set_absolute_volume send command {:?}", cmd);
        let buf = self.peer.send_vendor_dependent_command(&cmd).await?;
        let response = SetAbsoluteVolumeResponse::decode(&buf[..])?;
        debug!("set_absolute_volume received response {:?}", response);
        Ok(response.volume())
    }

    /// Sends GetElementAttributes command to the peer.
    /// Returns all the media attributes received as a response or an error.
    pub async fn get_media_attributes(&self) -> Result<fidl_avrcp::MediaAttributes, Error> {
        let cmd = GetElementAttributesCommand::all_attributes();
        debug!("get_media_attributes send command {:?}", cmd);
        let buf = self.peer.send_vendor_dependent_command(&cmd).await?;
        let response = GetElementAttributesResponse::decode(&buf[..])?;
        debug!("get_media_attributes received response {:?}", response);
        Ok(fidl_avrcp::MediaAttributes {
            title: response.0.title.clone(),
            artist_name: response.0.artist_name.clone(),
            album_name: response.0.album_name.clone(),
            track_number: response.0.track_number.clone(),
            total_number_of_tracks: response.0.total_number_of_tracks.clone(),
            genre: response.0.genre.clone(),
            playing_time: response.0.playing_time,
            ..Default::default()
        })
    }

    /// Send a GetCapabilities command requesting all supported events by the peer.
    /// Returns the supported NotificationEventIds by the peer or an error.
    pub async fn get_supported_events(&self) -> Result<HashSet<NotificationEventId>, Error> {
        self.peer.get_supported_events().await
    }

    /// Send a GetPlayStatus command requesting current status of playing media.
    /// Returns the PlayStatus of current media on the peer, or an error.
    pub async fn get_play_status(&self) -> Result<fidl_avrcp::PlayStatus, Error> {
        let cmd = GetPlayStatusCommand::new();
        debug!("get_play_status send command {:?}", cmd);
        let buf = self.peer.send_vendor_dependent_command(&cmd).await?;
        let response = GetPlayStatusResponse::decode(&buf[..])?;
        debug!("get_play_status received response {:?}", response);
        let mut play_status = fidl_avrcp::PlayStatus::default();
        play_status.song_length = if response.song_length != SONG_LENGTH_NOT_SUPPORTED {
            Some(response.song_length)
        } else {
            None
        };
        play_status.song_position = if response.song_position != SONG_POSITION_NOT_SUPPORTED {
            Some(response.song_position)
        } else {
            None
        };
        play_status.playback_status = Some(response.playback_status.into());
        Ok(play_status)
    }

    pub async fn get_current_player_application_settings(
        &self,
        attribute_ids: Vec<PlayerApplicationSettingAttributeId>,
    ) -> Result<PlayerApplicationSettings, Error> {
        let cmd = GetCurrentPlayerApplicationSettingValueCommand::new(attribute_ids);
        debug!("get_current_player_application_settings command {:?}", cmd);
        let buf = self.peer.send_vendor_dependent_command(&cmd).await?;
        let response = GetCurrentPlayerApplicationSettingValueResponse::decode(&buf[..])?;
        Ok(response.try_into()?)
    }

    pub async fn get_all_player_application_settings(
        &self,
    ) -> Result<PlayerApplicationSettings, Error> {
        // Get all the supported attributes.
        let cmd = ListPlayerApplicationSettingAttributesCommand::new();
        debug!("list_player_application_setting_attributes command {:?}", cmd);
        let buf = self.peer.send_vendor_dependent_command(&cmd).await?;
        let response = ListPlayerApplicationSettingAttributesResponse::decode(&buf[..])?;

        // Get the text information of supported attributes.
        // TODO(https://fxbug.dev/42117316): Get attribute text information for only custom attributes.
        let cmd = GetPlayerApplicationSettingAttributeTextCommand::new(
            response.player_application_setting_attribute_ids(),
        );
        debug!("get_player_application_setting_attribute_text command {:?}", cmd);
        let buf = self.peer.send_vendor_dependent_command(&cmd).await?;
        let _text_response = GetPlayerApplicationSettingAttributeTextResponse::decode(&buf[..])?;

        // For each attribute returned, get the set of possible values and text.
        for attribute in response.player_application_setting_attribute_ids() {
            let cmd = ListPlayerApplicationSettingValuesCommand::new(attribute);
            debug!("list_player_application_setting_values command {:?}", cmd);
            let buf = self.peer.send_vendor_dependent_command(&cmd).await?;
            let list_response = ListPlayerApplicationSettingValuesResponse::decode(&buf[..])?;

            // TODO(https://fxbug.dev/42117316): Get value text information for only custom attributes.
            let cmd = GetPlayerApplicationSettingValueTextCommand::new(
                attribute,
                list_response.player_application_setting_value_ids(),
            );
            debug!("get_player_application_setting_value_text command {:?}", cmd);
            let buf = self.peer.send_vendor_dependent_command(&cmd).await?;
            let value_text_response =
                GetPlayerApplicationSettingValueTextResponse::decode(&buf[..])?;
            debug!(
                "Response from get_player_application_setting_value_text: {:?}",
                value_text_response
            );
        }

        // TODO(https://fxbug.dev/42117316): Use return value of ListPlayerApplicationSettingValuesResponse::decode()
        // to get current custom settings. For now, get current settings for default settings.
        self.get_current_player_application_settings(
            response.player_application_setting_attribute_ids().into(),
        )
        .await
    }

    pub async fn get_player_application_settings(
        &self,
        attribute_ids: Vec<PlayerApplicationSettingAttributeId>,
    ) -> Result<PlayerApplicationSettings, Error> {
        if attribute_ids.is_empty() {
            self.get_all_player_application_settings().await
        } else {
            self.get_current_player_application_settings(attribute_ids).await
        }
    }

    pub async fn set_player_application_settings(
        &self,
        requested_settings: PlayerApplicationSettings,
    ) -> Result<PlayerApplicationSettings, Error> {
        let settings_vec = settings_to_vec(&requested_settings);

        // Default the returned `set_settings` to be the input `requested_settings`.
        let mut set_settings = requested_settings.clone();

        // If the command fails, the target did not accept the setting. Reflect
        // this in the returned `set_settings`.
        for setting in settings_vec {
            let cmd = SetPlayerApplicationSettingValueCommand::new(vec![setting]);
            debug!("set_player_application_settings command {:?}", cmd);
            let response_buf = self.peer.send_vendor_dependent_command(&cmd).await;

            match response_buf {
                Ok(buf) => {
                    let _ = SetPlayerApplicationSettingValueResponse::decode(&buf[..])?;
                }
                Err(_) => {
                    set_settings.clear_attribute(setting.0);
                }
            }
        }
        Ok(set_settings)
    }

    pub async fn inform_battery_status(
        &self,
        battery_status: fidl_avrcp::BatteryStatus,
    ) -> Result<(), Error> {
        let cmd = InformBatteryStatusOfCtCommand::new(battery_status);
        debug!("inform_battery_status_of_ct command {:?}", cmd);
        let buf = self.peer.send_vendor_dependent_command(&cmd).await?;
        let response = InformBatteryStatusOfCtResponse::decode(&buf[..])?;
        debug!("inform_battery_status_of_ct received response {:?}", response);
        Ok(())
    }

    /// Sends a raw vendor dependent AVC command on the control channel. Returns the response
    /// from from the peer or an error. Used by the test controller and intended only for debugging.
    pub async fn send_raw_vendor_command<'a>(
        &'a self,
        pdu_id: u8,
        payload: &'a [u8],
    ) -> Result<Vec<u8>, Error> {
        let command = RawVendorDependentPacket::new(PduId::try_from(pdu_id)?, payload);
        self.peer.send_vendor_dependent_command(&command).await
    }

    /// Sends a command over the browse channel to set the browsed player. If the peer
    /// responded with a success code, sets the browsed player to the player ID otherwise
    /// clears the previously set browsed player. Returns the response from from the peer
    /// or an error. Used by the test controller and intended only for debugging.
    pub async fn set_browsed_player(
        &self,
        player_id: u16,
    ) -> Result<(), fidl_avrcp::BrowseControllerError> {
        let cmd = SetBrowsedPlayerCommand::new(player_id);
        let err: fidl_avrcp::BrowseControllerError =
            match self.send_browse_command(PduId::SetBrowsedPlayer, cmd).await {
                Ok(buf) => match SetBrowsedPlayerResponse::decode(&buf[..]) {
                    Ok(SetBrowsedPlayerResponse::Success(r)) => {
                        self.peer.set_browsable_player(Some(BrowsablePlayer::new(player_id, r)));
                        return Ok(());
                    }
                    Ok(SetBrowsedPlayerResponse::Failure(status)) => status.into(),
                    Err(e) => e.into(),
                },
                Err(e) => e.into(),
            };
        // Clear the previously-set browsed player if we failed to
        // set the new browsed player.
        self.peer.set_browsable_player(None);
        Err(err)
    }

    pub fn get_browsed_player(&self) -> Result<BrowsablePlayer, fidl_avrcp::BrowseControllerError> {
        // AVRCP 1.6.2 Section 6.9.3 states that `SetBrowsedPlayer` command
        // shall be sent successfully before any other commands are sent on the
        // browsing channel except GetFolderItems in the Media Player List
        // scope.
        self.peer
            .get_browsable_player()
            .ok_or(fidl_avrcp::BrowseControllerError::NoAvailablePlayers)
    }

    pub async fn get_folder_items(
        &self,
        cmd: GetFolderItemsCommand,
    ) -> Result<GetFolderItemsResponseParams, fidl_avrcp::BrowseControllerError> {
        let buf = self.send_browse_command(PduId::GetFolderItems, cmd).await?;
        let response = GetFolderItemsResponse::decode(&buf[..])?;
        debug!("get_folder_items received response {:?}", response);
        match response {
            GetFolderItemsResponse::Failure(status) => Err(status.into()),
            GetFolderItemsResponse::Success(r) => Ok(r),
        }
    }

    pub async fn get_file_system_items(
        &self,
        start_index: u32,
        end_index: u32,
        attribute_option: fidl_avrcp::AttributeRequestOption,
    ) -> Result<Vec<fidl_avrcp::FileSystemItem>, fidl_avrcp::BrowseControllerError> {
        let _ = self.get_browsed_player()?;

        let cmd = GetFolderItemsCommand::new_virtual_file_system(
            start_index,
            end_index,
            attribute_option,
        );
        let response = self.get_folder_items(cmd).await?;
        response.item_list().into_iter().map(TryInto::try_into).collect()
    }

    /// Returns the list of media players from `start_index` up to `end_index`.
    /// If `update_players_info` is set to true, updates the peer with players information.
    pub async fn get_media_player_items(
        &mut self,
        start_index: u32,
        end_index: u32,
        update_players_info: bool,
    ) -> Result<Vec<fidl_avrcp::MediaPlayerItem>, fidl_avrcp::BrowseControllerError> {
        // AVRCP 1.6.2 Section 6.9.3 states that GetFolderItems in the Media Player List scope command can be sent before
        // SetBrowsedPlayer command is sent over the browse channel.
        let cmd = GetFolderItemsCommand::new_media_player_list(start_index, end_index);
        let response = self.get_folder_items(cmd).await?;
        let players = response
            .item_list()
            .into_iter()
            .map(|i| i.try_into_media_player())
            .collect::<Result<Vec<MediaPlayerItem>, PacketError>>()
            .map_err(|e| fidl_avrcp::BrowseControllerError::from(e))?;
        if update_players_info {
            self.peer.peer.write().update_available_players(&players);
        }
        Ok(players.into_iter().map(Into::into).collect())
    }

    pub async fn get_now_playing_items(
        &self,
        start_index: u32,
        end_index: u32,
        attribute_option: fidl_avrcp::AttributeRequestOption,
    ) -> Result<Vec<fidl_avrcp::MediaElementItem>, fidl_avrcp::BrowseControllerError> {
        let _ = self.get_browsed_player()?;

        let cmd =
            GetFolderItemsCommand::new_now_playing_list(start_index, end_index, attribute_option);
        let response = self.get_folder_items(cmd).await?;
        response
            .item_list()
            .into_iter()
            .map(|item| item.try_into().and_then(into_media_element_item))
            .collect()
    }

    /// Changes directory from the current directory.
    /// If folder_uid is set to some value, it will invoke a ChangePath
    /// command with direction set to Folder Down.
    /// Otherwise, it will invoke a ChangePath command with direction set to
    /// Folder Up.
    /// If the command was successful, returns the number of items in the
    /// changed directory. Otherwise, error is returned.
    pub async fn change_directory(
        &self,
        folder_uid: Option<u64>,
    ) -> Result<u32, fidl_avrcp::BrowseControllerError> {
        let player = self.get_browsed_player()?;

        let cmd = ChangePathCommand::new(player.uid_counter(), folder_uid)
            .map_err(|_| fidl_avrcp::BrowseControllerError::PacketEncoding)?;
        let buf = self.send_browse_command(PduId::ChangePath, cmd).await?;
        let response = ChangePathResponse::decode(&buf[..])?;
        debug!("change_directory received response {:?}", response);
        match response {
            ChangePathResponse::Failure(status) => Err(status.into()),
            ChangePathResponse::Success { num_of_items } => Ok(num_of_items),
        }
    }

    pub async fn play_item(
        &self,
        uid: u64,
        scope: Scope,
    ) -> Result<(), fidl_avrcp::BrowseControllerError> {
        let player = self.get_browsed_player()?;

        let cmd = PlayItemCommand::new(scope, uid, player.uid_counter())?;
        let buf = self
            .peer
            .send_vendor_dependent_command(&cmd)
            .await
            .map_err(Into::<fidl_avrcp::BrowseControllerError>::into)?;
        let response = PlayItemResponse::decode(&buf[..]).map_err(|e| Error::PacketError(e))?;
        debug!("play_item received response {:?}", response);

        match response.status() {
            StatusCode::Success => Ok(()),
            e => Err(e.into()),
        }
    }

    /// Get item attributes from the now playing list using the browse channel.
    pub async fn get_item_attributes(
        &self,
        uid: u64,
    ) -> Result<fidl_avrcp::MediaAttributes, fidl_avrcp::BrowseControllerError> {
        let player = self.get_browsed_player()?;

        let cmd =
            GetItemAttributesCommand::from_now_playing_list(uid, player.uid_counter(), vec![]);
        let buf = self.send_browse_command(PduId::GetItemAttributes, cmd).await?;
        let response = GetItemAttributesResponse::decode(&buf[..])?;
        debug!("get_item_attributes received response {:?}", response);
        match response {
            GetItemAttributesResponse::Failure(status) => Err(status.into()),
            GetItemAttributesResponse::Success(attributes) => Ok(attributes.into()),
        }
    }

    async fn send_browse_command(
        &self,
        pdu_id: PduId,
        command: impl Encodable<Error = PacketError>,
    ) -> Result<Vec<u8>, Error> {
        let mut payload = vec![0; command.encoded_len()];
        let _ = command.encode(&mut payload[..])?;
        self.send_raw_browse_command(u8::from(&pdu_id), &payload).await
    }

    pub fn send_raw_browse_command<'a>(
        &'a self,
        pdu_id: u8,
        payload: &'a [u8],
    ) -> impl Future<Output = Result<Vec<u8>, Error>> + 'a {
        self.peer.send_browse_command(pdu_id, payload)
    }

    /// For the FIDL test controller. Informational only and intended for logging only. The state is
    /// inherently racey.
    pub fn is_control_connected(&self) -> bool {
        self.peer.is_control_connected()
    }

    /// For the FIDL test browse controller. Informational only and intended for logging only. The state is
    /// inherently racey.
    pub fn is_browse_connected(&self) -> bool {
        self.peer.is_browse_connected()
    }

    /// Creates new stream for events and returns the receiveing end for
    /// getting notification events from the peer.
    pub fn add_event_listener(&self) -> ControllerEventStream {
        // TODO(https://fxbug.dev/42120733) handle back pressure correctly and reduce mpsc::channel buffer sizes.
        let (sender, receiver) = mpsc::channel(512);
        self.peer.add_control_listener(sender);
        receiver
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    use crate::peer::tests::*;
    use crate::peer_manager::TargetDelegate;
    use fuchsia_bluetooth::profile::avrcp::{
        AvrcpProtocolVersion, AvrcpService, AvrcpTargetFeatures,
    };

    use assert_matches::assert_matches;
    use async_utils::PollExt;
    use bt_avctp::{AvcPeer, AvctpCommandStream, AvctpPeer};
    use bt_channel_test_support::{Transport, create_test_channels};
    use fidl::endpoints::create_proxy_and_stream;
    use fidl_fuchsia_bluetooth_avrcp::AttributeRequestOption;
    use fidl_fuchsia_bluetooth_bredr::ProfileMarker;
    use fuchsia_async as fasync;
    use fuchsia_bluetooth::profile::Psm;
    use fuchsia_bluetooth::types::PeerId;
    use futures::TryStreamExt;
    use std::pin::pin;
    use std::sync::Arc;
    use test_case::test_case;

    const PLAYER_ID: u16 = 1004;
    const UID_COUNTER: u16 = 1;

    fn set_browsed_player(
        exec: &mut fasync::TestExecutor,
        controller: &Controller,
        avctp_cmd_stream: &mut AvctpCommandStream,
    ) {
        let set_browsed_player_fut = controller.set_browsed_player(PLAYER_ID);
        let mut set_browsed_player_fut = pin!(set_browsed_player_fut);

        exec.run_until_stalled(&mut set_browsed_player_fut).expect_pending("should not be ready");
        let command = get_next_avctp_command(exec, avctp_cmd_stream);
        // Ensure command params are correct.
        let params = decode_avctp_command(&command, PduId::SetBrowsedPlayer);
        let cmd =
            SetBrowsedPlayerCommand::decode(&params).expect("should have received valid command");
        assert_eq!(cmd.player_id(), PLAYER_ID);
        // Create mock response.
        let resp =
            SetBrowsedPlayerResponse::new_success(UID_COUNTER, 1, vec!["folder1".to_string()])
                .expect("should be initialized");
        send_avctp_response(PduId::SetBrowsedPlayer, &resp, &command);

        assert_matches!(
            exec.run_until_stalled(&mut set_browsed_player_fut).expect("should be ready"),
            Ok(())
        );
    }

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

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn get_media_player_items(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();

        let (mut controller, remote_avc_peer, remote_avctp_peer) = set_up(transport);
        let mut avctp_cmd_stream = remote_avctp_peer.take_command_stream();
        let mut avc_cmd_stream = remote_avc_peer.take_command_stream();
        expect_outgoing_commands(&mut exec, &mut avc_cmd_stream, &mut avctp_cmd_stream);

        // Mock response returning 1 player item.
        let resp = GetFolderItemsResponse::new_success(
            1,
            vec![BrowseableItem::MediaPlayer(MediaPlayerItem::new(
                1,
                fidl_avrcp::MajorPlayerType::AUDIO.bits(),
                0,
                PlaybackStatus::Stopped,
                [0, 0],
                "player 1".to_string(),
            ))],
        );

        // Get media player items without updating the peer.
        {
            let get_media_players_fut = controller.get_media_player_items(0, 5, false);
            let mut get_media_players_fut = pin!(get_media_players_fut);

            exec.run_until_stalled(&mut get_media_players_fut).expect_pending("result not ready");
            let command = get_next_avctp_command(&mut exec, &mut avctp_cmd_stream);
            // Ensure command params are correct.
            let params = decode_avctp_command(&command, PduId::GetFolderItems);
            let cmd =
                GetFolderItemsCommand::decode(&params).expect("should have received valid command");
            assert_matches!(cmd.scope(),
            Scope::MediaPlayerList => {
                assert_eq!(cmd.start_item(), 0);
                assert_eq!(cmd.end_item(), 5);
                assert!(cmd.attribute_list().is_none());
            });
            // Create mock response.

            send_avctp_response(PduId::GetFolderItems, &resp, &command);

            assert_matches!(
                exec.run_until_stalled(&mut get_media_players_fut).expect("should be ready"),
                Ok(list) if list.len() == 1);
        }
        // Peer's available player info is not updated.
        assert_eq!(controller.peer.peer.read().available_players.len(), 0);

        // Get media player items and update the peer afterwards.
        {
            let get_media_players_fut = controller.get_media_player_items(0, 5, true);
            let mut get_media_players_fut = pin!(get_media_players_fut);

            exec.run_until_stalled(&mut get_media_players_fut).expect_pending("result not ready");
            let command = get_next_avctp_command(&mut exec, &mut avctp_cmd_stream);
            send_avctp_response(PduId::GetFolderItems, &resp, &command);

            assert_matches!(
                exec.run_until_stalled(&mut get_media_players_fut).expect("should be ready"),
                Ok(list) if list.len() == 1);
        }
        // Peer's available player info is updated.
        assert!(controller.peer.peer.read().available_players.contains_key(&1));
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn get_file_system_items(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();

        let (controller, remote_avc_peer, remote_avctp_peer) = set_up(transport);
        let mut avctp_cmd_stream = remote_avctp_peer.take_command_stream();
        let mut avc_cmd_stream = remote_avc_peer.take_command_stream();
        expect_outgoing_commands(&mut exec, &mut avc_cmd_stream, &mut avctp_cmd_stream);

        // Set browsed player.
        set_browsed_player(&mut exec, &controller, &mut avctp_cmd_stream);

        let get_file_system_fut =
            controller.get_file_system_items(0, 5, AttributeRequestOption::GetAll(false));
        let mut get_file_system_fut = pin!(get_file_system_fut);

        exec.run_until_stalled(&mut get_file_system_fut).expect_pending("result not ready");
        let command = get_next_avctp_command(&mut exec, &mut avctp_cmd_stream);
        // Ensure command params are correct.
        let params = decode_avctp_command(&command, PduId::GetFolderItems);
        let cmd =
            GetFolderItemsCommand::decode(&params).expect("should have received valid command");
        assert_matches!(cmd.scope(),
        Scope::MediaPlayerVirtualFilesystem => {
            assert_eq!(cmd.start_item(), 0);
                        assert_eq!(cmd.end_item(), 5);
                        assert_eq!(cmd.attribute_list().unwrap().len(), 0);
        });
        // Create mock response.
        let resp = GetFolderItemsResponse::new_success(1, vec![]);
        send_avctp_response(PduId::GetFolderItems, &resp, &command);

        let _ = exec.run_until_stalled(&mut get_file_system_fut).expect("should be ready");
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn get_now_playing_items_failure(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();

        let (controller, remote_avc_peer, remote_avctp_peer) = set_up(transport);
        let mut avctp_cmd_stream = remote_avctp_peer.take_command_stream();
        let mut avc_cmd_stream = remote_avc_peer.take_command_stream();
        expect_outgoing_commands(&mut exec, &mut avc_cmd_stream, &mut avctp_cmd_stream);

        let get_now_playing_fut =
            controller.get_now_playing_items(0, 5, AttributeRequestOption::GetAll(true));
        let mut get_now_playing_fut = pin!(get_now_playing_fut);

        // Should fail since browsed player isn't set.
        assert_matches!(
            exec.run_until_stalled(&mut get_now_playing_fut).expect("should be done"),
            Err(_)
        );
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn test_change_directory(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();

        let (controller, remote_avc_peer, remote_avctp_peer) = set_up(transport);
        let mut avctp_cmd_stream = remote_avctp_peer.take_command_stream();
        let mut avc_cmd_stream = remote_avc_peer.take_command_stream();
        expect_outgoing_commands(&mut exec, &mut avc_cmd_stream, &mut avctp_cmd_stream);

        set_browsed_player(&mut exec, &controller, &mut avctp_cmd_stream);

        let move_into_dir_fut = controller.change_directory(Some(2));
        let mut move_into_dir_fut = pin!(move_into_dir_fut);

        exec.run_until_stalled(&mut move_into_dir_fut).expect_pending("result not ready");
        let command = get_next_avctp_command(&mut exec, &mut avctp_cmd_stream);
        // Ensure command params are correct.
        let params = decode_avctp_command(&command, PduId::ChangePath);
        let cmd = ChangePathCommand::decode(&params).expect("should have received valid command");
        assert_eq!(cmd.uid_counter(), UID_COUNTER);
        assert_eq!(cmd.folder_uid(), Some(&std::num::NonZeroU64::new(2).unwrap()));
        // Create mock response.
        let resp = ChangePathResponse::new_success(10);
        send_avctp_response(PduId::ChangePath, &resp, &command);

        assert_matches!(
        exec.run_until_stalled(&mut move_into_dir_fut).expect("result ready"),
        Ok(num_of_items) => {
            assert_eq!(num_of_items, 10);
        });
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn test_get_item_attributes(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();

        let (controller, remote_avc_peer, remote_avctp_peer) = set_up(transport);
        let mut avctp_cmd_stream = remote_avctp_peer.take_command_stream();
        let mut avc_cmd_stream = remote_avc_peer.take_command_stream();
        expect_outgoing_commands(&mut exec, &mut avc_cmd_stream, &mut avctp_cmd_stream);

        set_browsed_player(&mut exec, &controller, &mut avctp_cmd_stream);

        let get_item_attributes_fut = controller.get_item_attributes(0xc0decafe);
        let mut get_item_attributes_fut = pin!(get_item_attributes_fut);

        let res = exec.run_until_stalled(&mut get_item_attributes_fut);
        let futures::task::Poll::Pending = res else {
            panic!("Expected pending and got {res:?}");
        };
        let command = get_next_avctp_command(&mut exec, &mut avctp_cmd_stream);
        // Ensure command params are correct.
        let params = decode_avctp_command(&command, PduId::GetItemAttributes);
        let cmd =
            GetItemAttributesCommand::decode(&params).expect("should have received valid command");
        assert_eq!(cmd.uid_counter(), UID_COUNTER);
        assert_eq!(cmd.uid(), 0xc0decafe);
        // Create mock response.
        let resp = vec![
            0x04, 0x05, 0x00, 0x00, 0x00, 0x01, 0x00, 0x6a, 0x00, 0x06, 0x47, 0x6c, 0x6f, 0x72,
            0x69, 0x61, 0x00, 0x00, 0x00, 0x02, 0x00, 0x6a, 0x00, 0x0b, 0x45, 0x61, 0x72, 0x6c,
            0x79, 0x20, 0x44, 0x6f, 0x6f, 0x72, 0x73, 0x00, 0x00, 0x00, 0x03, 0x00, 0x6a, 0x00,
            0x0f, 0x47, 0x6c, 0x6f, 0x72, 0x69, 0x61, 0x20, 0x2d, 0x20, 0x53, 0x69, 0x6e, 0x67,
            0x6c, 0x65, 0x00, 0x00, 0x00, 0x06, 0x00, 0x6a, 0x00, 0x04, 0x52, 0x6f, 0x63, 0x6b,
            0x00, 0x00, 0x00, 0x07, 0x00, 0x6a, 0x00, 0x06, 0x32, 0x37, 0x37, 0x39, 0x36, 0x36,
        ];

        send_avctp_response_raw(PduId::GetItemAttributes, resp, &command);

        assert_matches!(
            exec.run_until_stalled(&mut get_item_attributes_fut).expect("result ready"),
            Ok(_media_attributes)
        );
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn test_play_item(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();

        let (controller, remote_avc_peer, remote_avctp_peer) = set_up(transport);
        let mut avctp_cmd_stream = remote_avctp_peer.take_command_stream();
        let mut avc_cmd_stream = remote_avc_peer.take_command_stream();
        expect_outgoing_commands(&mut exec, &mut avc_cmd_stream, &mut avctp_cmd_stream);
        set_browsed_player(&mut exec, &controller, &mut avctp_cmd_stream);

        // Test PlayFileSystemItem.
        let play_fut = controller.play_item(1, Scope::MediaPlayerVirtualFilesystem);
        let mut play_fut = pin!(play_fut);
        exec.run_until_stalled(&mut play_fut).expect_pending("result not ready");
        let command = exec
            .run_until_stalled(&mut avc_cmd_stream.try_next())
            .expect("should be ready")
            .unwrap()
            .expect("has valid command");
        // Ensure command params are correct.
        assert!(command.is_vendor_dependent());
        let (pdu_id, body) = decode_avc_vendor_command(&command).expect("should succeed");
        assert_eq!(pdu_id, PduId::PlayItem);
        let cmd = PlayItemCommand::decode(body).expect("can decode");
        assert_eq!(cmd.uid(), 1);
        assert_eq!(cmd.scope(), Scope::MediaPlayerVirtualFilesystem);
        assert_eq!(cmd.uid_counter(), UID_COUNTER);
        // Create mock response.
        let resp = PlayItemResponse::new(StatusCode::Success);
        let packet = resp.encode_packet().expect("unable to encode packets for event");
        let _ = command
            .send_response(bt_avctp::AvcResponseType::Accepted, &packet[..])
            .expect("should have succeeded");

        // Test PlayNowPlayingItem failure.
        let play_fut = controller.play_item(1, Scope::NowPlaying);
        let mut play_fut = pin!(play_fut);
        exec.run_until_stalled(&mut play_fut).expect_pending("result not ready");
        let command = exec
            .run_until_stalled(&mut avc_cmd_stream.try_next())
            .expect("should be ready")
            .unwrap()
            .expect("has valid command");
        // Ensure scope is correct (skip other param checks since they were
        // done in the above call).
        assert!(command.is_vendor_dependent());
        let (_, body) = decode_avc_vendor_command(&command).expect("should succeed");
        let cmd = PlayItemCommand::decode(body).expect("can decode");
        assert_eq!(cmd.scope(), Scope::NowPlaying);
        // Create mock response that results in failure.
        let resp = PlayItemResponse::new(StatusCode::InternalError);
        let packet = resp.encode_packet().expect("unable to encode packets for event");
        let _ = command
            .send_response(bt_avctp::AvcResponseType::Rejected, &packet[..])
            .expect("should have succeeded");

        assert_matches!(exec.run_until_stalled(&mut play_fut).expect("should be ready"), Err(_));
    }
}
