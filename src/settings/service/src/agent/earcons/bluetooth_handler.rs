// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::agent::earcons::agent::CommonEarconsParams;
use crate::agent::earcons::sound_ids::{
    BLUETOOTH_CONNECTED_SOUND_ID, BLUETOOTH_DISCONNECTED_SOUND_ID,
};
use crate::agent::earcons::utils::{connect_to_sound_player, play_sound};
use anyhow::{Context, Error, format_err};
use fidl::endpoints::create_request_stream;
use fidl_fuchsia_media_sessions2::{
    DiscoveryMarker, SessionsWatcherRequest, SessionsWatcherRequestStream, WatchOptions,
};
use futures::channel::mpsc::UnboundedSender;
use futures::channel::oneshot;
use futures::stream::TryStreamExt;
use settings_audio::Request as AudioRequest;
use settings_audio::types::{AudioSettingSource, AudioStreamType, SetAudioStream};
use settings_common::inspect::event::ExternalEventPublisher;
use settings_common::{call, trace};
use std::collections::HashSet;
use {fuchsia_async as fasync, fuchsia_trace as ftrace};

/// Type for uniquely identifying bluetooth media sessions.
type SessionId = u64;

/// The file path for the earcon to be played for bluetooth connecting.
const BLUETOOTH_CONNECTED_FILE_PATH: &str = "bluetooth-connected.wav";

/// The file path for the earcon to be played for bluetooth disconnecting.
const BLUETOOTH_DISCONNECTED_FILE_PATH: &str = "bluetooth-disconnected.wav";

pub(crate) const BLUETOOTH_DOMAIN: &str = "Bluetooth";

/// The `BluetoothHandler` takes care of the earcons functionality on bluetooth connection
/// and disconnection.
pub(super) struct BluetoothHandler {
    // Parameters common to all earcons handlers.
    common_earcons_params: CommonEarconsParams,
    audio_request_tx: Option<UnboundedSender<AudioRequest>>,
    // The publisher to use for connecting to services.
    external_publisher: ExternalEventPublisher,
    // The ids of the media sessions that are currently active.
    active_sessions: HashSet<SessionId>,
}

impl std::fmt::Debug for BluetoothHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BluetoothHandler")
            .field("common_earcons_params", &self.common_earcons_params)
            .field("audio_request_tx", &self.audio_request_tx)
            .field("active_sessions", &self.active_sessions)
            .finish_non_exhaustive()
    }
}

/// The type of bluetooth earcons sound.
enum BluetoothSoundType {
    Connected,
    Disconnected,
}

impl BluetoothHandler {
    pub(super) async fn spawn(
        audio_request_tx: Option<UnboundedSender<AudioRequest>>,
        external_publisher: ExternalEventPublisher,
        params: CommonEarconsParams,
    ) -> Result<(), Error> {
        let mut handler = Self {
            audio_request_tx,
            common_earcons_params: params,
            external_publisher,
            active_sessions: HashSet::<SessionId>::new(),
        };
        handler.watch_bluetooth_connections().await
    }

    /// Watch for media session changes. The media sessions that have the
    /// Bluetooth mode in their metadata signify a bluetooth connection.
    /// The id of a disconnected device will be received on removal.
    pub(super) async fn watch_bluetooth_connections(&mut self) -> Result<(), Error> {
        // Connect to media session Discovery service.
        let discovery_connection_result = self
            .common_earcons_params
            .service_context
            .connect_with_publisher::<DiscoveryMarker, _>(self.external_publisher.clone())
            .await
            .context("Connecting to fuchsia.media.sessions2.Discovery");

        let discovery_proxy = discovery_connection_result.map_err(|e| {
            format_err!("Failed to connect to fuchsia.media.sessions2.Discovery: {:?}", e)
        })?;

        // Create and handle the request stream of media sessions.
        let (watcher_client, watcher_requests) = create_request_stream();

        call!(discovery_proxy =>
            watch_sessions(&WatchOptions::default(), watcher_client))
        .map_err(|e| format_err!("Unable to start discovery of MediaSessions: {:?}", e))?;

        self.handle_bluetooth_connections(watcher_requests);
        Ok(())
    }

    /// Handles the stream of media session updates, and possibly plays earcons
    /// sounds based on what type of update is received.
    fn handle_bluetooth_connections(&mut self, mut watcher_requests: SessionsWatcherRequestStream) {
        let audio_request_tx = self.audio_request_tx.clone();
        let mut active_sessions_clone = self.active_sessions.clone();
        let external_publisher = self.external_publisher.clone();
        let common_earcons_params = self.common_earcons_params.clone();

        fasync::Task::local(async move {
            loop {
                let maybe_req = watcher_requests.try_next().await;
                match maybe_req {
                    Ok(Some(req)) => {
                        match req {
                            SessionsWatcherRequest::SessionUpdated {
                                session_id: id,
                                session_info_delta: delta,
                                responder,
                            } => {
                                if let Err(e) = responder.send() {
                                    log::error!("Failed to acknowledge delta from SessionWatcher: {:?}", e);
                                    return;
                                }

                                if active_sessions_clone.contains(&id)
                                    || !matches!(delta.domain, Some(name) if name == BLUETOOTH_DOMAIN)
                                {
                                    continue;
                                }
                                let _ = active_sessions_clone.insert(id);

                                let audio_request_tx = audio_request_tx.clone();
                                let external_publisher = external_publisher.clone();
                                let common_earcons_params = common_earcons_params.clone();
                                fasync::Task::local(async move {
                                    play_bluetooth_sound(
                                        common_earcons_params,
                                        audio_request_tx,
                                        external_publisher,
                                        BluetoothSoundType::Connected,
                                    )
                                    .await;
                                })
                                .detach();
                            }
                            SessionsWatcherRequest::SessionRemoved { session_id, responder } => {
                                if let Err(e) = responder.send() {
                                    log::error!(
                                        "Failed to acknowledge session removal from SessionWatcher: {:?}",
                                        e
                                    );
                                    return;
                                }

                                if !active_sessions_clone.contains(&session_id) {
                                    log::warn!(
                                        "Tried to remove nonexistent media session id {:?}",
                                        session_id
                                    );
                                    continue;
                                }
                                let _ = active_sessions_clone.remove(&session_id);
                                let audio_request_tx = audio_request_tx.clone();
                                let external_publisher = external_publisher.clone();
                                let common_earcons_params = common_earcons_params.clone();
                                fasync::Task::local(async move {
                                    play_bluetooth_sound(
                                        common_earcons_params,
                                        audio_request_tx,
                                        external_publisher,
                                        BluetoothSoundType::Disconnected,
                                    )
                                    .await;
                                })
                                .detach();
                            }
                        }
                    },
                    Ok(None) => {
                        log::warn!("stream ended on fuchsia.media.sessions2.SessionsWatcher");
                        break;
                    },
                    Err(e) => {
                        log::error!("failed to watch fuchsia.media.sessions2.SessionsWatcher: {:?}", e);
                        break;
                    },
                }
            }
        })
        .detach();
    }
}

/// Play a bluetooth earcons sound.
async fn play_bluetooth_sound(
    common_earcons_params: CommonEarconsParams,
    audio_request_tx: Option<UnboundedSender<AudioRequest>>,
    external_publisher: ExternalEventPublisher,
    sound_type: BluetoothSoundType,
) {
    // Connect to the SoundPlayer if not already connected.
    connect_to_sound_player(
        external_publisher,
        common_earcons_params.service_context.clone(),
        common_earcons_params.sound_player_connection.clone(),
    )
    .await;

    let sound_player_connection = common_earcons_params.sound_player_connection.clone();
    let sound_player_connection_lock = sound_player_connection.lock().await;
    let sound_player_added_files = common_earcons_params.sound_player_added_files.clone();

    if let Some(sound_player_proxy) = sound_player_connection_lock.as_ref() {
        match_background_to_media(audio_request_tx).await;
        match sound_type {
            BluetoothSoundType::Connected => {
                if play_sound(
                    sound_player_proxy,
                    BLUETOOTH_CONNECTED_FILE_PATH,
                    BLUETOOTH_CONNECTED_SOUND_ID,
                    sound_player_added_files.clone(),
                )
                .await
                .is_err()
                {
                    log::error!(
                        "[bluetooth_earcons_handler] failed to play bluetooth earcon connection sound"
                    );
                }
            }
            BluetoothSoundType::Disconnected => {
                if play_sound(
                    sound_player_proxy,
                    BLUETOOTH_DISCONNECTED_FILE_PATH,
                    BLUETOOTH_DISCONNECTED_SOUND_ID,
                    sound_player_added_files.clone(),
                )
                .await
                .is_err()
                {
                    log::error!(
                        "[bluetooth_earcons_handler] failed to play bluetooth earcon disconnection sound"
                    );
                }
            }
        };
    } else {
        log::error!(
            "[bluetooth_earcons_handler] failed to play bluetooth earcon sound: no sound player connection"
        );
    }
}

/// Match the background volume to the current media volume before playing the bluetooth earcon.
async fn match_background_to_media(audio_request_tx: Option<UnboundedSender<AudioRequest>>) {
    let info = if let Some(audio_request_tx) = audio_request_tx.as_ref() {
        let (tx, rx) = oneshot::channel();
        if audio_request_tx.unbounded_send(AudioRequest::Get(ftrace::Id::new(), tx)).is_ok() {
            rx.await.ok()
        } else {
            None
        }
    } else {
        None
    };
    // Extract media and background volumes.
    let mut media_volume = 0.0;
    let mut background_volume = 0.0;
    if let Some(info) = info {
        for stream in &info.streams {
            if stream.stream_type == AudioStreamType::Media {
                media_volume = stream.user_volume_level;
            } else if stream.stream_type == AudioStreamType::Background {
                background_volume = stream.user_volume_level;
            }
        }
    } else {
        log::error!("Could not extract background and media volumes")
    }

    // If they are different, set the background volume to match the media volume.
    if media_volume != background_volume {
        let id = ftrace::Id::new();
        trace!(id, "bluetooth_handler set background volume");
        let streams = vec![SetAudioStream {
            stream_type: AudioStreamType::Background,
            source: AudioSettingSource::System,
            user_volume_level: Some(media_volume),
            user_volume_muted: None,
        }];
        if let Some(audio_request_tx) = audio_request_tx {
            let (tx, rx) = oneshot::channel();
            if audio_request_tx.unbounded_send(AudioRequest::Set(streams, id, tx)).is_ok()
                && let Err(e) = rx.await
            {
                log::error!(
                    "Failed to play bluetooth connection sound after waiting for request response: {e:?}"
                );
            }
        }
    }
}
