// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::common_utils::common::macros::{fx_err_and_bail, with_line};
use crate::media_session::types::PlayerStatusWrapper;
use anyhow::Error;
use fidl::endpoints::create_request_stream;
use fidl_fuchsia_media_sessions2::{
    self as sessions2, ActiveSessionMarker, ActiveSessionProxy, PlayerRegistration, PublisherMarker,
};
use fuchsia_async as fasync;
use fuchsia_component::client::connect_to_protocol;
use fuchsia_sync::RwLock;
use futures::StreamExt;
use log::info;
use std::sync::Arc;

#[derive(Debug)]
pub struct MediaSessionFacade {
    active_session_proxy: Option<ActiveSessionProxy>,
    player_task: RwLock<Option<fasync::Task<()>>>,
    requests_received: Arc<RwLock<Vec<String>>>,
}

impl MediaSessionFacade {
    pub fn new() -> Self {
        Self {
            active_session_proxy: None,
            player_task: RwLock::new(None),
            requests_received: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Returns the active session proxy.
    fn get_active_session_proxy(&self) -> Result<ActiveSessionProxy, Error> {
        if let Some(proxy) = &self.active_session_proxy {
            Ok(proxy.clone())
        } else {
            match connect_to_protocol::<ActiveSessionMarker>() {
                Ok(proxy) => Ok(proxy),
                Err(e) => fx_err_and_bail!(
                    &with_line!("MediaSessionFacade::get_active_session_proxy"),
                    format_err!("Failed to create proxy: {:?}", e)
                ),
            }
        }
    }

    /// Returns the active media session's player status.
    /// If there's no active session, it will return a None type value.
    pub async fn watch_active_session_status(&self) -> Result<Option<PlayerStatusWrapper>, Error> {
        let active_session_proxy = &self
            .get_active_session_proxy()
            .map_err(|e| format_err!("Failed to get active session proxy: {}", e))?;
        let session = active_session_proxy
            .watch_active_session()
            .await
            .map_err(|e| format_err!("Failed to watch active session: {}", e))?;
        if let Some(session_control_marker) = session {
            let info_delta = session_control_marker
                .into_proxy()
                .watch_status()
                .await
                .map_err(|e| format_err!("Failed to watch session status: {}", e))?;
            Ok(Some(info_delta.player_status.into()))
        } else {
            Ok(None)
        }
    }

    /// Publishes a mock Media Player via the fuchsia.media.sessions2 Publisher protocol.
    /// Clients can watch Media Sessions via the fuchsia.media.sessions2 Discovery protocol.
    /// This mock player does not route requests; it just records the requests it receives.
    /// The list of requests can be accessed through this facade's ListReceivedRequests command.
    /// The Bluetooth AVRCP component forwards commands to the most recently activated player.
    pub async fn publish_mock_player(&self) -> Result<(), Error> {
        let requests_received = self.requests_received.clone();
        let fut = async move {
            let publisher = fuchsia_component::client::connect_to_protocol::<PublisherMarker>()
                .expect("Failed to connect to Publisher");

            let (player_client, mut player_request_stream) = create_request_stream();

            publisher
                .publish(
                    player_client,
                    &PlayerRegistration {
                        domain: Some("domain://sl4f".to_string()),
                        ..Default::default()
                    },
                )
                .await
                .expect("Failed to publish Player");

            #[allow(clippy::collection_is_never_read)]
            let mut _player_info_change_responder = None;
            let mut sent_response = false;

            loop {
                match player_request_stream.next().await {
                    Some(Ok(request)) => match request {
                        sessions2::PlayerRequest::WatchInfoChange { responder } => {
                            if !sent_response {
                                let _ = responder.send(&sessions2::PlayerInfoDelta::default());
                                sent_response = true;
                            } else {
                                // Save responder so request stream is not dropped
                                _player_info_change_responder = Some(responder);
                            }
                        }
                        request => {
                            requests_received.write().push(request.method_name().to_string());
                        }
                    },
                    Some(Err(e)) => {
                        info!(e:?; "Mock player request stream error");
                        break;
                    }
                    None => {
                        info!("Mock player request stream terminated");
                        break;
                    }
                }
            }
        };

        if let Some(_task) = self.player_task.write().take() {
            info!("Dropping old mock player");
        }
        *self.player_task.write() = Some(fasync::Task::spawn(fut));
        info!("Publishing mock player");

        Ok(())
    }

    /// Unpublishes a mock Media Player and resets the list of received requests.
    /// Returns true if a mock Media Player was running and is now stopped.
    pub async fn stop_mock_player(&self) -> Result<bool, Error> {
        if let Some(_task) = self.player_task.write().take() {
            info!("Stopping mock player");
            self.requests_received.write().clear();
            return Ok(true);
        }
        info!("Mock player stoppage requested, but no session exists");
        Ok(false)
    }

    /// Returns a list of the player requests received by mock Media Players spawned by this
    /// facade's PublishMockPlayer command.
    pub async fn list_received_requests(&self) -> Result<Vec<String>, Error> {
        Ok(self.requests_received.read().clone())
    }
}
