// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::types::{
    AUDIO_STREAM_TYPE_COUNT, AudioInfo, AudioSettingSource, AudioStream, AudioStreamType,
};
use settings_common::config::default_settings::DefaultSetting;
use settings_common::inspect::config_logger::InspectConfigLogger;
use settings_storage::storage_factory::DefaultLoader;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Mutex;

const DEFAULT_VOLUME_LEVEL: f32 = 0.5;
const DEFAULT_VOLUME_MUTED: bool = false;

const DEFAULT_STREAMS: [AudioStream; AUDIO_STREAM_TYPE_COUNT] = [
    create_default_audio_stream(AudioStreamType::Background),
    create_default_audio_stream(AudioStreamType::Media),
    create_default_audio_stream(AudioStreamType::Interruption),
    create_default_audio_stream(AudioStreamType::SystemAgent),
    create_default_audio_stream(AudioStreamType::Communication),
    create_default_audio_stream(AudioStreamType::Accessibility),
];

const DEFAULT_AUDIO_INFO: AudioInfo =
    AudioInfo { streams: DEFAULT_STREAMS, modified_counters: None };

/// A mapping from stream type to an arbitrary numerical value. This number will
/// change from the number sent in the previous update if the stream type's
/// volume has changed.
pub type ModifiedCounters = HashMap<AudioStreamType, usize>;

pub fn create_default_modified_counters() -> ModifiedCounters {
    IntoIterator::into_iter([
        AudioStreamType::Background,
        AudioStreamType::Media,
        AudioStreamType::Interruption,
        AudioStreamType::SystemAgent,
        AudioStreamType::Communication,
        AudioStreamType::Accessibility,
    ])
    .map(|stream_type| (stream_type, 0))
    .collect()
}

pub const fn create_default_audio_stream(stream_type: AudioStreamType) -> AudioStream {
    AudioStream {
        stream_type,
        source: AudioSettingSource::User,
        user_volume_level: DEFAULT_VOLUME_LEVEL,
        user_volume_muted: DEFAULT_VOLUME_MUTED,
    }
}

pub fn build_audio_default_settings(
    config_logger: Rc<Mutex<InspectConfigLogger>>,
) -> DefaultSetting<AudioInfo, &'static str> {
    DefaultSetting::new(
        Some(DEFAULT_AUDIO_INFO),
        "/config/data/audio_config_data.json",
        config_logger,
    )
}

/// Returns a default audio [`AudioInfo`] that is derived from
/// [`DEFAULT_AUDIO_INFO`] with any fields specified in the
/// audio configuration set.
///
/// [`DEFAULT_AUDIO_INFO`]: static@DEFAULT_AUDIO_INFO
#[derive(Clone)]
pub struct AudioInfoLoader {
    audio_default_settings: Rc<Mutex<DefaultSetting<AudioInfo, &'static str>>>,
}

impl AudioInfoLoader {
    pub fn new(audio_default_settings: DefaultSetting<AudioInfo, &'static str>) -> Self {
        Self { audio_default_settings: Rc::new(Mutex::new(audio_default_settings)) }
    }
}

impl DefaultLoader for AudioInfoLoader {
    type Result = AudioInfo;

    fn default_value(&self) -> Self::Result {
        let mut default_audio_info: AudioInfo = DEFAULT_AUDIO_INFO.clone();

        if let Ok(Some(audio_configuration)) =
            self.audio_default_settings.lock().unwrap().get_cached_value()
        {
            default_audio_info.streams = audio_configuration.streams;
        }
        default_audio_info
    }
}

#[cfg(test)]
mod tests {
    // assert_eq!(..., false) is easier to read than assert!(!...).
    #![allow(clippy::bool_assert_comparison)]

    use super::*;
    use crate::types::{AudioInfoV1, AudioInfoV2, AudioInfoV3};
    use fuchsia_async::TestExecutor;
    use fuchsia_inspect::component;
    use settings_storage::device_storage::DeviceStorageCompatible;
    use settings_test_common::helpers::move_executor_forward_and_get;

    const CONFIG_AUDIO_INFO: AudioInfo = AudioInfo {
        streams: [
            AudioStream {
                stream_type: AudioStreamType::Background,
                source: AudioSettingSource::System,
                user_volume_level: 0.6,
                user_volume_muted: true,
            },
            AudioStream {
                stream_type: AudioStreamType::Media,
                source: AudioSettingSource::System,
                user_volume_level: 0.7,
                user_volume_muted: true,
            },
            AudioStream {
                stream_type: AudioStreamType::Interruption,
                source: AudioSettingSource::System,
                user_volume_level: 0.2,
                user_volume_muted: true,
            },
            AudioStream {
                stream_type: AudioStreamType::SystemAgent,
                source: AudioSettingSource::User,
                user_volume_level: 0.3,
                user_volume_muted: true,
            },
            AudioStream {
                stream_type: AudioStreamType::Communication,
                source: AudioSettingSource::User,
                user_volume_level: 0.4,
                user_volume_muted: false,
            },
            AudioStream {
                stream_type: AudioStreamType::Accessibility,
                source: AudioSettingSource::User,
                user_volume_level: 0.35,
                user_volume_muted: false,
            },
        ],
        modified_counters: None,
    };

    /// Construct default audio settings and its dependencies.
    fn make_default_settings() -> DefaultSetting<AudioInfo, &'static str> {
        let config_logger =
            Rc::new(Mutex::new(InspectConfigLogger::new(component::inspector().root())));
        build_audio_default_settings(config_logger)
    }

    /// Load default settings from disk.
    fn load_default_settings(
        default_settings: &mut DefaultSetting<AudioInfo, &'static str>,
    ) -> AudioInfo {
        default_settings
            .load_default_value()
            .expect("if config exists, it should be parseable")
            .expect("default value should always exist")
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_audio_config() {
        let mut default_settings = make_default_settings();
        let current_from_storage = load_default_settings(&mut default_settings);
        // Ensure that settings are read from storage.
        assert_eq!(CONFIG_AUDIO_INFO, current_from_storage);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_audio_info_migration_v1_to_v2() {
        let mut default_settings = make_default_settings();
        let mut v1_settings =
            AudioInfoV1::default_value(load_default_settings(&mut default_settings));
        let updated_mic_mute_val = !v1_settings.input.mic_mute;
        v1_settings.input.mic_mute = updated_mic_mute_val;
        v1_settings.streams[0].user_volume_level = 0.9;
        v1_settings.streams[0].user_volume_muted = false;

        let serialized_v1 = serde_json::to_string(&v1_settings).expect("default should serialize");
        let v2_from_v1 = AudioInfoV2::try_deserialize_from(&serialized_v1)
            .expect("deserialization should succeed");

        // Ensure that changes made in v1 are migrated to v2.
        assert_eq!(v2_from_v1.input.mic_mute, updated_mic_mute_val);
        assert_eq!(v2_from_v1.streams[0].user_volume_level, 0.9);
        assert_eq!(v2_from_v1.streams[0].user_volume_muted, false);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_audio_info_migration_v2_to_v3() {
        let mut default_settings = make_default_settings();
        let mut v2_settings =
            AudioInfoV1::default_value(load_default_settings(&mut default_settings));
        v2_settings.streams[0].user_volume_level = 0.9;
        v2_settings.streams[0].user_volume_muted = false;

        let serialized_v2 = serde_json::to_string(&v2_settings).expect("default should serialize");
        let v3_from_v2 = AudioInfoV3::try_deserialize_from(&serialized_v2)
            .expect("deserialization should succeed");

        // Ensure that changes made in v2 are migrated to v3.
        assert_eq!(v3_from_v2.streams[0].user_volume_level, 0.9);
        assert_eq!(v3_from_v2.streams[0].user_volume_muted, false);
    }

    #[fuchsia::test]
    fn test_audio_info_migration_v3_to_current() {
        let mut executor = TestExecutor::new_with_fake_time();
        let mut default_settings = make_default_settings();
        let current_defaults = load_default_settings(&mut default_settings);

        let mut v3_settings = move_executor_forward_and_get(
            &mut executor,
            async { AudioInfoV3::default_value(current_defaults) },
            "Unable to get V3 default value",
        );
        v3_settings.streams[0].user_volume_level = 0.9;
        v3_settings.streams[0].user_volume_muted = false;

        let serialized_v3 = serde_json::to_string(&v3_settings).expect("default should serialize");
        let current_from_v3 = AudioInfo::try_deserialize_from(&serialized_v3)
            .expect("deserialization should succeed");

        // Ensure that changes made in v3 are migrated to current.
        assert_eq!(current_from_v3.streams[0].user_volume_level, 0.9);
        assert_eq!(current_from_v3.streams[0].user_volume_muted, false);
        // Ensure that migrating from v3 picks up a default for the new stream type.
        assert_eq!(current_from_v3.streams[5], DEFAULT_AUDIO_INFO.streams[5]);
    }

    #[fuchsia::test]
    fn test_audio_info_migration_v2_to_current() {
        let mut executor = TestExecutor::new_with_fake_time();
        let mut default_settings = make_default_settings();
        let current_defaults = load_default_settings(&mut default_settings);

        let mut v2_settings = move_executor_forward_and_get(
            &mut executor,
            async { AudioInfoV2::default_value(current_defaults) },
            "Unable to get V2 default value",
        );
        let updated_mic_mute_val = !v2_settings.input.mic_mute;
        v2_settings.input.mic_mute = updated_mic_mute_val;
        v2_settings.streams[0].user_volume_level = 0.9;
        v2_settings.streams[0].user_volume_muted = false;

        let serialized_v2 = serde_json::to_string(&v2_settings).expect("default should serialize");
        let current_from_v2 = AudioInfo::try_deserialize_from(&serialized_v2)
            .expect("deserialization should succeed");

        // Ensure that changes made in v2 are migrated to current.
        assert_eq!(current_from_v2.streams[0].user_volume_level, 0.9);
        assert_eq!(current_from_v2.streams[0].user_volume_muted, false);
        // Ensure that migrating from v2 picks up a default for the new stream type.
        assert_eq!(current_from_v2.streams[5], DEFAULT_AUDIO_INFO.streams[5]);
    }

    #[fuchsia::test]
    fn test_audio_info_migration_v1_to_current() {
        let mut executor = TestExecutor::new_with_fake_time();
        let mut default_settings = make_default_settings();
        let current_defaults = load_default_settings(&mut default_settings);

        let mut v1_settings = move_executor_forward_and_get(
            &mut executor,
            async { AudioInfoV1::default_value(current_defaults) },
            "Unable to get V1 default value",
        );
        let updated_mic_mute_val = !v1_settings.input.mic_mute;
        v1_settings.input.mic_mute = updated_mic_mute_val;
        v1_settings.streams[0].user_volume_level = 0.9;
        v1_settings.streams[0].user_volume_muted = false;

        let serialized_v1 = serde_json::to_string(&v1_settings).expect("default should serialize");
        let current_from_v1 = AudioInfo::try_deserialize_from(&serialized_v1)
            .expect("deserialization should succeed");

        // Ensure that changes made in v1 are migrated to current.
        assert_eq!(current_from_v1.streams[0].user_volume_level, 0.9);
        assert_eq!(current_from_v1.streams[0].user_volume_muted, false);
        // Ensure that migrating from v1 picks up a default for the new stream type.
        assert_eq!(current_from_v1.streams[5], DEFAULT_AUDIO_INFO.streams[5]);
    }
}
