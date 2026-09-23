// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::anyhow;
use fidl::endpoints::create_request_stream;
use fidl_fuchsia_bluetooth_avrcp::{
    PeerManagerMarker, TargetAvcError, TargetHandlerMarker, TargetHandlerRequest,
    TargetHandlerRequestStream, TargetPassthroughError,
};
use fuchsia_component::client::connect_to_protocol;
use futures::TryStreamExt;
use log::{trace, warn};
use std::sync::Arc;

use crate::media::media_sessions::MediaSessions;

/// Fulfills an AVRCP Target request.
async fn handle_target_request(
    request: TargetHandlerRequest,
    media_sessions: Arc<MediaSessions>,
) -> Result<(), fidl::Error> {
    trace!(request = request.method_name(); "Received target request");
    match request {
        TargetHandlerRequest::GetEventsSupported { responder } => {
            // Send a static response of TG supported events.
            responder.send(Ok(media_sessions.get_supported_notification_events()))?;
        }
        TargetHandlerRequest::GetPlayStatus { responder } => {
            if let Ok(state) = media_sessions.get_active_session() {
                responder.send(Ok(&state.session_info().get_play_status().into()))?;
            } else {
                responder.send(Err(TargetAvcError::RejectedNoAvailablePlayers))?;
            }
        }
        TargetHandlerRequest::GetMediaAttributes { responder } => {
            if let Ok(state) = media_sessions.get_active_session() {
                responder.send(Ok(&state.session_info().get_media_info().clone().into()))?;
            } else {
                responder.send(Err(TargetAvcError::RejectedNoAvailablePlayers))?;
            }
        }
        TargetHandlerRequest::SendCommand { command, pressed, responder } => {
            if let Ok(state) = media_sessions.get_active_session() {
                responder.send(state.handle_avc_passthrough_command(command, pressed).await)?;
            } else {
                responder.send(Err(TargetPassthroughError::CommandRejected))?;
            }
        }
        TargetHandlerRequest::ListPlayerApplicationSettingAttributes { responder } => {
            // Send back the static list of Media supported PlayerApplicationSettingAttributes.
            if let Ok(state) = media_sessions.get_active_session() {
                responder.send(Ok(state.get_supported_player_application_setting_attributes()))?;
            } else {
                responder.send(Err(TargetAvcError::RejectedNoAvailablePlayers))?;
            }
        }
        TargetHandlerRequest::GetPlayerApplicationSettings { attribute_ids, responder } => {
            if let Ok(state) = media_sessions.get_active_session() {
                let result = state.session_info().get_player_application_settings(attribute_ids);
                responder.send(result.map(Into::into).as_ref().map_err(|e| *e))?;
            } else {
                responder.send(Err(TargetAvcError::RejectedNoAvailablePlayers))?;
            }
        }
        TargetHandlerRequest::SetPlayerApplicationSettings { requested_settings, responder } => {
            if let Ok(state) = media_sessions.get_active_session() {
                let result =
                    state.handle_set_player_application_settings(requested_settings.into()).await;
                responder.send(result.map(Into::into).as_ref().map_err(|e| *e))?;
            } else {
                responder.send(Err(TargetAvcError::RejectedNoAvailablePlayers))?;
            }
        }
        TargetHandlerRequest::GetNotification { event_id, responder } => {
            if let Ok(state) = media_sessions.get_active_session() {
                let result = state.session_info().get_notification_value(&event_id).map(Into::into);
                responder.send(result.as_ref().map_err(|e| *e))?;
            } else {
                responder.send(Err(TargetAvcError::RejectedNoAvailablePlayers))?;
            }
        }
        TargetHandlerRequest::WatchNotification {
            event_id,
            current,
            pos_change_interval,
            responder,
        } => {
            // Add the notification responder to our notifications map.
            // A FIDL response will be sent when the notification specified by `event_id` is triggered.
            let _ = media_sessions.register_notification(
                event_id,
                current.into(),
                pos_change_interval,
                responder,
            );
        }
        TargetHandlerRequest::SetAddressedPlayer { player_id, responder } => {
            let response = media_sessions.set_addressed_player(player_id);
            responder.send(response)?;
        }
        TargetHandlerRequest::GetMediaPlayerItems { responder } => {
            let response = media_sessions.get_media_player_items();
            responder.send(response.as_deref().map_err(|e| *e))?;
        }
    }

    Ok(())
}

/// Process and fulfill incoming TargetHandler requests.
pub(crate) async fn handle_target_requests(
    mut target_request_stream: TargetHandlerRequestStream,
    media_sessions: Arc<MediaSessions>,
) -> Result<(), anyhow::Error> {
    while let Some(req) = target_request_stream.try_next().await? {
        let fut = handle_target_request(req, media_sessions.clone());
        if let Err(e) = fut.await {
            warn!("Error handling request: {:?}", e);
        }
    }

    Err(anyhow!("AVRCP TargetHandler dropped."))
}

/// Set up the AVRCP Service and register the target handler.
/// Spin up task for handling incoming TargetHandler requests.
pub(crate) async fn process_avrcp_requests(
    media_sessions: Arc<MediaSessions>,
) -> Result<(), anyhow::Error> {
    // Register this target handler with the AVRCP component.
    let avrcp_svc = connect_to_protocol::<PeerManagerMarker>()
        .expect("Failed to connect to Bluetooth AVRCP interface");
    let (target_client, request_stream) = create_request_stream::<TargetHandlerMarker>();
    if let Err(e) = avrcp_svc.register_target_handler(target_client).await? {
        return Err(anyhow!("Error registering target handler: {:?}", e));
    }
    trace!("Registered the Target handler");
    handle_target_requests(request_stream, media_sessions).await
}
