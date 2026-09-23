// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::agent::earcons::agent::CommonEarconsParams;
use crate::agent::earcons::sound_ids::{VOLUME_CHANGED_SOUND_ID, VOLUME_MAX_SOUND_ID};
use crate::agent::earcons::utils::{connect_to_sound_player, play_sound};
use anyhow::Error;
use futures::StreamExt;
use futures::channel::mpsc::{self, UnboundedSender};
use futures::channel::oneshot;
use futures::future::OptionFuture;
use settings_audio::types::{
    AUDIO_STREAM_TYPE_COUNT, AudioInfo, AudioSettingSource, AudioStream, AudioStreamType,
    SetAudioStream,
};
use settings_audio::{ModifiedCounters, Request as AudioRequest, create_default_modified_counters};
use settings_common::inspect::event::ExternalEventPublisher;
use settings_common::trace;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use {fuchsia_async as fasync, fuchsia_trace as ftrace};

/// The `VolumeChangeHandler` takes care of the earcons functionality on volume change.
pub(super) struct VolumeChangeHandler {
    common_earcons_params: CommonEarconsParams,
    last_user_volumes: HashMap<AudioStreamType, f32>,
    modified_counters: ModifiedCounters,
    external_publisher: ExternalEventPublisher,
    audio_request_tx: Option<UnboundedSender<AudioRequest>>,
}

/// The maximum volume level.
const MAX_VOLUME: f32 = 1.0;

/// The file path for the earcon to be played for max sound level.
const VOLUME_MAX_FILE_PATH: &str = "volume-max.wav";

/// The file path for the earcon to be played for volume changes below max volume level.
const VOLUME_CHANGED_FILE_PATH: &str = "volume-changed.wav";

impl VolumeChangeHandler {
    pub(super) async fn spawn(
        audio_request_tx: Option<UnboundedSender<AudioRequest>>,
        external_publisher: ExternalEventPublisher,
        params: CommonEarconsParams,
    ) -> Result<(), Error> {
        let info = if let Some(request_tx) = audio_request_tx.as_ref() {
            let (tx, rx) = oneshot::channel();
            if request_tx.unbounded_send(AudioRequest::Get(ftrace::Id::new(), tx)).is_ok() {
                rx.await.ok()
            } else {
                None
            }
        } else {
            None
        };
        let last_user_volumes = info
            .map(|info| {
                // Create map from stream type to user volume levels for each stream.
                info.streams
                    .iter()
                    .filter(|x| {
                        x.stream_type == AudioStreamType::Media
                            || x.stream_type == AudioStreamType::Interruption
                    })
                    .map(|stream| (stream.stream_type, stream.user_volume_level))
                    .collect()
            })
            .unwrap_or_else(HashMap::new);

        fasync::Task::local(async move {
            let mut handler = Self {
                common_earcons_params: params,
                last_user_volumes,
                modified_counters: create_default_modified_counters(),
                external_publisher,
                audio_request_tx: audio_request_tx.clone(),
            };

            let mut audio_rx = audio_request_tx.as_ref().and_then(|request_tx| {
                let (audio_tx, audio_rx) = mpsc::unbounded();
                request_tx.unbounded_send(AudioRequest::Listen(audio_tx)).ok().map(|_| audio_rx)
            });

            let mut audio_next = OptionFuture::from(audio_rx.as_mut().map(|rx| rx.next()));

            loop {
                futures::select! {
                    audio_info = audio_next => {
                        if let Some(Some(audio_info)) = audio_info {
                            handler.on_audio_info(audio_info).await;
                            audio_next = OptionFuture::from(audio_rx.as_mut().map(|rx| rx.next()));
                        }
                    }
                    complete => break,
                }
            }
        })
        .detach();

        Ok(())
    }

    /// Calculates and returns the streams that were changed based on
    /// their timestamps, updating them in the stored timestamps if
    /// they were changed.
    fn calculate_changed_streams(
        &mut self,
        all_streams: [AudioStream; AUDIO_STREAM_TYPE_COUNT],
        new_modified_counters: ModifiedCounters,
    ) -> Vec<AudioStream> {
        let mut changed_stream_types = HashSet::new();
        for (stream_type, timestamp) in new_modified_counters {
            if self.modified_counters.get(&stream_type) != Some(&timestamp) {
                let _ = changed_stream_types.insert(stream_type);
                let _ = self.modified_counters.insert(stream_type, timestamp);
            }
        }

        IntoIterator::into_iter(all_streams)
            .filter(|stream| changed_stream_types.contains(&stream.stream_type))
            .collect()
    }

    /// Retrieve a user volume of the specified `stream_type` from the given `changed_streams`.
    fn get_user_volume(
        &self,
        changed_streams: Vec<AudioStream>,
        stream_type: AudioStreamType,
    ) -> Option<f32> {
        changed_streams.iter().find(|&&x| x.stream_type == stream_type).map(|x| x.user_volume_level)
    }

    /// Retrieve the change source of the specified `stream_type` from the given `changed_streams`.
    fn get_change_source(
        &self,
        changed_streams: Vec<AudioStream>,
        stream_type: AudioStreamType,
    ) -> Option<AudioSettingSource> {
        changed_streams.iter().find(|&&x| x.stream_type == stream_type).map(|x| x.source)
    }

    /// Helper for on_audio_info. Handles the changes for a specific AudioStreamType.
    /// Enables separate handling of earcons on different streams.
    async fn on_audio_info_for_stream(
        &mut self,
        new_user_volume: f32,
        stream_type: AudioStreamType,
        change_source: Option<AudioSettingSource>,
    ) {
        let volume_is_max = new_user_volume == MAX_VOLUME;
        let last_user_volume = self.last_user_volumes.get(&stream_type);

        // Logging for debugging volume changes.
        log::debug!(
            "[earcons_agent] New {:?} user volume: {:?}, Last {:?} user volume: {:?}",
            stream_type,
            new_user_volume,
            stream_type,
            last_user_volume,
        );

        if last_user_volume != Some(&new_user_volume) || volume_is_max {
            // On restore, the last media user volume is set for the first time, and registers
            // as different from the last seen volume, because it is initially None. Don't play
            // the earcons sound on that set.
            //
            // If the change_source is System, do not play a sound since the user doesn't need
            // to hear feedback for such changes. For system sounds that need to play earcons,
            // the source should be AudioSettingSource::SystemWithFeedback. An example
            // of a system change is when the volume is reset after night mode deactivates.
            if last_user_volume.is_some() && change_source != Some(AudioSettingSource::System) {
                let id = ftrace::Id::new();
                trace!(id, "volume_change_handler set background");
                let streams = vec![SetAudioStream {
                    stream_type: AudioStreamType::Background,
                    source: AudioSettingSource::System,
                    user_volume_level: Some(new_user_volume),
                    user_volume_muted: None,
                }];
                let success = if let Some(audio_request_tx) = self.audio_request_tx.as_ref() {
                    let (tx, rx) = oneshot::channel();
                    if audio_request_tx.unbounded_send(AudioRequest::Set(streams, id, tx)).is_ok() {
                        rx.await
                            .map_err(|e| {
                                log::error!(
                                    "Failed to play sound after waiting for volume set: {e:?}"
                                )
                            })
                            .is_ok()
                    } else {
                        false
                    }
                } else {
                    false
                };
                if success {
                    self.play_volume_sound(new_user_volume);
                }
            }

            let _ = self.last_user_volumes.insert(stream_type, new_user_volume);
        }
    }

    /// Invoked when a new `AudioInfo` is retrieved. Determines whether an
    /// earcon should be played and plays sound if necessary.
    async fn on_audio_info(&mut self, audio_info: AudioInfo) {
        let changed_streams = match audio_info.modified_counters {
            None => Vec::new(),
            Some(counters) => self.calculate_changed_streams(audio_info.streams, counters),
        };

        let media_user_volume =
            self.get_user_volume(changed_streams.clone(), AudioStreamType::Media);
        let interruption_user_volume =
            self.get_user_volume(changed_streams.clone(), AudioStreamType::Interruption);
        let media_change_source =
            self.get_change_source(changed_streams.clone(), AudioStreamType::Media);

        if let Some(media_user_volume) = media_user_volume {
            self.on_audio_info_for_stream(
                media_user_volume,
                AudioStreamType::Media,
                media_change_source,
            )
            .await;
        }
        if let Some(interruption_user_volume) = interruption_user_volume {
            self.on_audio_info_for_stream(
                interruption_user_volume,
                AudioStreamType::Interruption,
                None,
            )
            .await;
        }
    }

    /// Play the earcons sound given the changed volume streams.
    fn play_volume_sound(&self, volume: f32) {
        let external_publisher = self.external_publisher.clone();
        let common_earcons_params = self.common_earcons_params.clone();

        fasync::Task::local(async move {
            // Connect to the SoundPlayer if not already connected.
            connect_to_sound_player(
                external_publisher,
                common_earcons_params.service_context,
                Rc::clone(&common_earcons_params.sound_player_connection),
            )
            .await;

            let sound_player_connection = common_earcons_params.sound_player_connection;
            let sound_player_connection = sound_player_connection.lock().await;
            let sound_player_added_files = common_earcons_params.sound_player_added_files;

            if let (Some(sound_player_proxy), volume_level) =
                (sound_player_connection.as_ref(), volume)
            {
                let play_sound_result = if volume_level >= 1.0 {
                    play_sound(
                        sound_player_proxy,
                        VOLUME_MAX_FILE_PATH,
                        VOLUME_MAX_SOUND_ID,
                        sound_player_added_files.clone(),
                    )
                    .await
                } else if volume_level > 0.0 {
                    play_sound(
                        sound_player_proxy,
                        VOLUME_CHANGED_FILE_PATH,
                        VOLUME_CHANGED_SOUND_ID,
                        sound_player_added_files.clone(),
                    )
                    .await
                } else {
                    Ok(())
                };
                if let Err(e) = play_sound_result {
                    log::warn!("Failed to play sound: {:?}", e);
                }
            }
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuchsia_inspect::component;
    use futures::lock::Mutex;
    use settings_audio::build_audio_default_settings;
    use settings_common::inspect::config_logger::InspectConfigLogger;
    use settings_common::service_context::ServiceContext;
    use std::rc::Rc;

    fn fake_values() -> (
        [AudioStream; AUDIO_STREAM_TYPE_COUNT], // fake_streams
        ModifiedCounters,                       // old_counters
        ModifiedCounters,                       // new_counters
        Vec<AudioStream>,                       // expected_changed_streams
    ) {
        let config_logger =
            Rc::new(std::sync::Mutex::new(InspectConfigLogger::new(component::inspector().root())));
        let mut settings = build_audio_default_settings(config_logger);
        let settings = settings
            .load_default_value()
            .expect("config data should exist and be parseable for tests")
            .unwrap();
        let fake_streams = settings.streams;
        let old_timestamps = create_default_modified_counters();
        let new_timestamps = [
            (AudioStreamType::Background, 0),
            (AudioStreamType::Media, 1),
            (AudioStreamType::Interruption, 0),
            (AudioStreamType::SystemAgent, 2),
            (AudioStreamType::Communication, 3),
            (AudioStreamType::Accessibility, 4),
        ]
        .iter()
        .cloned()
        .collect();
        let expected_changed_streams =
            [fake_streams[1], fake_streams[3], fake_streams[4], fake_streams[5]].to_vec();
        (fake_streams, old_timestamps, new_timestamps, expected_changed_streams)
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_changed_streams() {
        let (fake_streams, old_timestamps, new_timestamps, expected_changed_streams) =
            fake_values();
        let (event_tx, _) = mpsc::unbounded();
        let external_publisher = ExternalEventPublisher::new(event_tx);
        let last_user_volumes: HashMap<_, _> =
            [(AudioStreamType::Media, 1.0), (AudioStreamType::Interruption, 0.5)].into();

        let mut handler = VolumeChangeHandler {
            common_earcons_params: CommonEarconsParams {
                service_context: Rc::new(ServiceContext::new(None)),
                sound_player_added_files: Rc::new(Mutex::new(HashSet::new())),
                sound_player_connection: Rc::new(Mutex::new(None)),
            },
            last_user_volumes,
            modified_counters: old_timestamps,
            external_publisher,
            audio_request_tx: None,
        };
        let changed_streams = handler.calculate_changed_streams(fake_streams, new_timestamps);
        assert_eq!(changed_streams, expected_changed_streams);
    }
}
