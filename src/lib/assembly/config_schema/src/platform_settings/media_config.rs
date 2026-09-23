// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Platform configuration options for the starnix area.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct PlatformMediaConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio: Option<AudioConfig>,

    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub camera: CameraConfig,

    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub multizone_leader: MultizoneConfig,

    /// Enable platform-provided video and audio decoders and encoders.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub enable_codecs: bool,

    /// Enable a platform-provided service that allows active media players (sessions) to be
    /// published and discovered, primarily for user control of those sessions.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub enable_sessions: bool,
}

/// The audio stack to use in the platform.
#[derive(Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AudioConfig {
    /// Use the full AudioCore stack.
    FullStack(AudioCoreConfig),

    /// Use AudioDeviceRegistry (the "partial" stack).
    DeviceRegistry(AudioDeviceRegistryConfig),
}

/// Configuration options for the AudioCore stack.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct AudioCoreConfig {
    /// Route the ADC device to audio_core.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub use_adc_device: bool,
}

/// Configuration options for AudioDeviceRegistry.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct AudioDeviceRegistryConfig {
    /// Start the ADR during target boot-up, rather than upon first client connection.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub eager_start: bool,

    /// Do not detect/acquire audio devices; allow others to directly connect to audio drivers.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub ignore_devices: bool,
}

/// The camera settings for the platform.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(default)]
pub struct CameraConfig {
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub enabled: bool,
}

/// The multizone_leader settings for the platform.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(default)]
pub struct MultizoneConfig {
    /// The component url for the multizone leader component.
    /// The component should expose these capabilities:
    ///   fuchsia.media.SessionAudioConsumerFactory
    ///   google.cast.multizone.Leader
    #[serde(skip_serializing_if = "Option::is_none")]
    pub component_url: Option<String>,
}
