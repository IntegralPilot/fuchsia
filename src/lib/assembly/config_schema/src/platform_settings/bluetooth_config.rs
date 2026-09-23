// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Specifies the configuration for the Bluetooth Snoop component (`bt-snoop`).
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Snoop {
    /// Don't include `bt-snoop`.
    #[default]
    None,
    /// Include `bt-snoop` with lazy startup.
    Lazy,
    /// Include `bt-snoop` with an eager startup during boot.
    Eager,
}

/// Configuration options for Bluetooth audio streaming (bt-a2dp).
// TODO(https://fxbug.dev/324894109): Add profile-specific arguments
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Default, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum A2dpConfig {
    #[default]
    Disabled,
    #[serde(untagged)]
    Enabled(A2dpConfigEnabled),
}

impl A2dpConfig {
    pub fn enabled(&self) -> bool {
        !matches!(self, Self::Disabled)
    }

    pub fn sink_enabled(&self) -> bool {
        let Self::Enabled(config) = self else {
            return false;
        };
        config.sink_enabled()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
pub struct A2dpConfigEnabled {
    // TODO(https://fxbug.dev/434204218): remove this field in favor of following
    #[serde(flatten)]
    pub sink_and_source: A2dpSinkAndSourceConfig,
}

impl A2dpConfigEnabled {
    pub fn sink_enabled(&self) -> bool {
        self.sink_and_source.sink_enabled()
    }
}

impl Default for A2dpConfigEnabled {
    fn default() -> Self {
        Self {
            sink_and_source: A2dpSinkAndSourceConfig::Enabled(A2dpSinkAndSourceDefaultEnabled {
                enabled: false,
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case", untagged)]
pub enum A2dpSinkAndSourceConfig {
    // TODO(https://fxbug.dev/435705910): Remove after all products are migrated to new config
    Enabled(A2dpSinkAndSourceDefaultEnabled),
    SinkAndSource(A2dpSinkAndSource),
    Sink(A2dpSinkOnly),
    Source(A2dpSourceOnly),
}

impl A2dpSinkAndSourceConfig {
    pub fn sink_enabled(&self) -> bool {
        matches!(
            self,
            Self::Enabled(A2dpSinkAndSourceDefaultEnabled { enabled: true })
                | Self::Sink(_)
                | Self::SinkAndSource(_)
        )
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
pub struct A2dpSinkAndSourceDefaultEnabled {
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
pub struct A2dpSinkOnly {
    pub sink: A2dpSinkType,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
pub struct A2dpSourceOnly {
    pub source: A2dpSourceType,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
pub struct A2dpSinkAndSource {
    pub sink: A2dpSinkType,
    pub source: A2dpSourceType,
}

/// The method to play audio when sink is enabled.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum A2dpSinkType {
    /// Audio will be played using a media player provided via the core at #media_player
    MediaPlayer,
}

/// The source for audio when A2DP source is enabled.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum A2dpSourceType {
    /// Create an Audio Output device which receives the audio from a full audio stack.
    /// Audio will be encoded in-band using a CodecFactory to determine the available codecs
    /// in concert with the peer capabilities.
    AudioOut,
    /// Use a pre-canned set of triangle waves that loosely resemble the Winchester Chimes.
    /// Audio will be encoded in-band similar to the AudioOut setting, but no output device or
    /// audio stack is necessary. The CodecFactory is still required to encode audio.
    BigBen,
    /// Audio will take an offloaded path to the controller. This will register a Codec device
    /// with the audio_registry for the audio subsystem to start/stop and configure the audio
    /// stream, but audio will be delivered to the controller via a DAI interface that is defined
    /// per-platform in the audio subsystem.
    /// This uses the audio offload extension provided by the BT vendor driver.
    Offload,
}

impl From<A2dpSourceType> for serde_json::Value {
    fn from(value: A2dpSourceType) -> Self {
        serde_json::to_value(value).unwrap()
    }
}

/// Configuration options for Bluetooth media info and controls (bt-avrcp).
// TODO(https://fxbug.dev/324894109): Add profile-specific arguments
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(default)]
pub struct AvrcpConfig {
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub enabled: bool,
}

/// Configuration options for Bluetooth Device Identification profile (bt-device-id).
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(default)]
pub struct DeviceIdConfig {
    /// Enable the device identification profile (`bt-device-id`).
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub enabled: bool,
    /// Uniquely identifies the Vendor of the device.
    /// Mandatory if `enabled` is true.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub vendor_id: u16,
    /// Uniquely identifies the product - typically a value assigned by the Vendor.
    /// Mandatory if `enabled` is true.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub product_id: u16,
    /// Device release number.
    /// Mandatory if `enabled` is true.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub version: u16,
    /// If `true`, designates this identification as the primary service record for this device.
    /// Mandatory if `enabled` is true.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub primary: bool,
    /// A human-readable description of the service.
    /// Optional if `enabled` is true.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_description: Option<String>,
}

/// Codec IDs defined by the Bluetooth HFP Specification
/// See HFP v1.9 Appendix B
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum HfpCodecId {
    Cvsd,
    Msbc,
    Lc3Swb,
}

/// Specifies which codecs can be encoded by the Bluetooth controller.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum ControllerCodecs {
    /// The controller supports the default set of known codecs (tyically CVSD & MSBC).
    #[default]
    Default,
    /// The controller does not support the encoding/decoding of any codecs.
    None,
    /// The specified codecs are supported by the controller.
    Codecs(Vec<HfpCodecId>),
}

impl ControllerCodecs {
    /// Returns the set of codecs encoded by the controller.
    pub fn codecs(&self) -> Vec<HfpCodecId> {
        match self {
            Self::Default => vec![HfpCodecId::Cvsd, HfpCodecId::Msbc],
            Self::None => Vec::new(),
            Self::Codecs(codecs) => codecs.clone(),
        }
    }
}

impl<'de> serde::Deserialize<'de> for ControllerCodecs {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let opt = Option::<Vec<HfpCodecId>>::deserialize(deserializer)?;
        match opt {
            None => Ok(Self::Default),
            Some(codecs) if codecs.is_empty() => Ok(Self::None),
            Some(codecs) => Ok(Self::Codecs(codecs)),
        }
    }
}

impl serde::Serialize for ControllerCodecs {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Default => Option::<Vec<HfpCodecId>>::None.serialize(serializer),
            Self::None => Some(Vec::<HfpCodecId>::new()).serialize(serializer),
            Self::Codecs(codecs) => Some(codecs).serialize(serializer),
        }
    }
}

impl JsonSchema for ControllerCodecs {
    fn schema_name() -> String {
        "ControllerCodecs".to_owned()
    }

    fn json_schema(generator: &mut schemars::r#gen::SchemaGenerator) -> schemars::schema::Schema {
        <Option<Vec<HfpCodecId>>>::json_schema(generator)
    }
}

/// Tri-state representation of a Bluetooth profile with optional features.
/// Allows product integrators to enable a profile without specifying any optional feature values.
#[derive(Deserialize, Default, PartialEq, Debug)]
enum BluetoothProfileDeserializer<T> {
    /// Disable the profile.
    #[default]
    #[serde(rename = "disabled")]
    Disabled,
    /// Enable the profile and use default values for the features.
    #[serde(rename = "enabled")]
    EnabledDefault,
    /// Enable the profile and use the provided input `T` values for the features.
    #[serde(untagged)]
    Enabled(T),
}

/// HFP Audio Gateway Features
/// See HFP v1.9 Page 100 for details.
/// Features not included are disabled by default, with the exception of the following which are
/// always enabled:
///  - Enhanced Call Status
///  - Extended Error Result Codes
///  - Codec Negotiation
///  - HF Indicators
///  - eSCO S4
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(default)]
pub struct AudioGatewayEnabledConfig {
    /// Enable management of of several concurrent calls.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub three_way_calling: bool,
    /// Enable echo canceling and/or noise reduction functionality.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub echo_canceling_and_noise_reduction: bool,
    /// Enable hands-free control of a device's functions through voice commands.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub voice_recognition: bool,
    /// Enable sending the ringtone for a phone call.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub inband_ringtone: bool,
    /// Enable the voice tag association feature.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub attach_phone_number_voice_tag: bool,
    /// Enable the reject incoming call feature.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub reject_incoming_call: bool,
    /// Enabled enhanced call controls (private mode & release specified call index procedures).
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub enhanced_call_control: bool,
    /// Enable enhanced hands-free call controls including integration with voice assistants.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub enhanced_voice_recognition_status: bool,
    /// Enable the voice-to-text feature.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub voice_recognition_text: bool,
}

/// Configuration options for the Bluetooth HFP Audio Gateway component ('bt-hfp-audio-gateway').
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(from = "BluetoothProfileDeserializer<AudioGatewayEnabledConfig>")]
pub enum AudioGatewayConfig {
    /// Disable `bt-hfp-audio-gateway`.
    #[default]
    Disabled,
    /// Enable `bt-hfp-audio-gateway`.
    Enabled(AudioGatewayEnabledConfig),
}

impl From<BluetoothProfileDeserializer<AudioGatewayEnabledConfig>> for AudioGatewayConfig {
    fn from(s: BluetoothProfileDeserializer<AudioGatewayEnabledConfig>) -> Self {
        match s {
            BluetoothProfileDeserializer::Disabled => Self::Disabled,
            BluetoothProfileDeserializer::EnabledDefault => {
                Self::Enabled(AudioGatewayEnabledConfig::default())
            }
            BluetoothProfileDeserializer::Enabled(c) => Self::Enabled(c),
        }
    }
}

/// HFP Hands Free Features
/// See HFP v1.9 Table 6.4 for the list of features and Table 3.2 for a description of the features.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(default)]
pub struct HandsFreeEnabledConfig {
    /// Enable echo canceling and/or noise reduction functionality.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub echo_canceling_and_noise_reduction: bool,
    /// Enable management of of several concurrent calls.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub three_way_calling: bool,
    /// Enable call identification for incoming calls.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub calling_line_identification: bool,
    /// Enable hands-free control of a device's functions through voice commands.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub voice_recognition: bool,
    /// Enable the remote volume control feature.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub remote_volume_control: bool,
    /// Enable enhanced hands-free call controls including integration with voice assistants.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub enhanced_voice_recognition: bool,
    /// Enable the voice-to-text feature.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub voice_recognition_text: bool,
}

/// Configuration options for the Bluetooth HFP Hands Free component ('bt-hfp-hands-free').
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(from = "BluetoothProfileDeserializer<HandsFreeEnabledConfig>")]
pub enum HandsFreeConfig {
    /// Disabled `bt-hfp-hands-free`.
    #[default]
    Disabled,
    /// Enable `bt-hfp-hands-free`.
    Enabled(HandsFreeEnabledConfig),
}

impl From<BluetoothProfileDeserializer<HandsFreeEnabledConfig>> for HandsFreeConfig {
    fn from(s: BluetoothProfileDeserializer<HandsFreeEnabledConfig>) -> Self {
        match s {
            BluetoothProfileDeserializer::Disabled => Self::Disabled,
            BluetoothProfileDeserializer::EnabledDefault => {
                Self::Enabled(HandsFreeEnabledConfig::default())
            }
            BluetoothProfileDeserializer::Enabled(c) => Self::Enabled(c),
        }
    }
}

/// Configuration options for Bluetooth hands free calling.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(default)]
pub struct HfpConfig {
    /// Specifies the configuration for `bt-hfp-audio-gateway`.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub audio_gateway: AudioGatewayConfig,

    /// Specifies the configuration for `bt-hfp-hands-free`.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub hands_free: HandsFreeConfig,

    /// The set of codecs that are enabled to use.
    /// If MSBC is enabled, Wide Band Speech will be enabled
    /// If LC3 is enabled, Super Wide Band will be enabled
    /// By default, all codecs supported (either by the controller as specified below) will be
    /// enabled.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub codecs_supported: Vec<HfpCodecId>,

    /// Set of codec ids that the Bluetooth controller can encode.
    /// Codecs not supported will be ignored.
    /// Codecs not in this list but in codecs_supported will be encoded locally and sent inband.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub controller_encodes: ControllerCodecs,
}

/// Configuration options for Bluetooth message access profile (bt-map)
/// client equipment role.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(default)]
pub struct MapConfig {
    /// Enable message access client equipment (`bt-map-mce`).
    pub mce_enabled: bool,
}

/// Platform Configuration for the enabled RFCOMM (`bt-rfcomm`) service.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RfcommEnabledConfig {}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(from = "BluetoothProfileDeserializer<RfcommEnabledConfig>")]
pub enum RfcommConfig {
    Disabled,
    Enabled(RfcommEnabledConfig),
}

impl Default for RfcommConfig {
    /// `bt-rfcomm` is enabled by default on the platform.
    fn default() -> Self {
        Self::Enabled(RfcommEnabledConfig::default())
    }
}

impl From<BluetoothProfileDeserializer<RfcommEnabledConfig>> for RfcommConfig {
    fn from(s: BluetoothProfileDeserializer<RfcommEnabledConfig>) -> Self {
        match s {
            BluetoothProfileDeserializer::Disabled => Self::Disabled,
            BluetoothProfileDeserializer::EnabledDefault => {
                Self::Enabled(RfcommEnabledConfig::default())
            }
            BluetoothProfileDeserializer::Enabled(c) => Self::Enabled(c),
        }
    }
}

impl RfcommConfig {
    pub fn enabled(&self) -> bool {
        matches!(self, Self::Enabled(_))
    }
}

/// Platform configuration to enable Bluetooth profiles.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(default)]
pub struct BluetoothProfilesConfig {
    /// Specifies the configuration for `bt-a2dp`.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub a2dp: A2dpConfig,

    /// Specifies the configuration for `bt-avrcp`.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub avrcp: AvrcpConfig,

    /// Specifies the configuration for `bt-device-id`.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub did: DeviceIdConfig,

    /// Specifies the configuration for `bt-hfp`.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub hfp: HfpConfig,

    /// Specifies the configuration for `bt-map`.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub map: MapConfig,

    /// Specifies the configuration for `bt-rfcomm`.
    #[serde(default)]
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub rfcomm: RfcommConfig,
}

impl BluetoothProfilesConfig {
    /// Returns true if any enabled profiles require RFCOMM.
    pub fn requires_rfcomm(&self) -> bool {
        let hfp_ag_enabled = matches!(self.hfp.audio_gateway, AudioGatewayConfig::Enabled(_));
        let hfp_hf_enabled = matches!(self.hfp.hands_free, HandsFreeConfig::Enabled(_));
        hfp_ag_enabled || hfp_hf_enabled || self.map.mce_enabled
    }
}

/// Platform configuration for Bluetooth Low Energy advertising.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(default)]
pub struct LeAdvertisingConfig {
    /// advertising interval minimum
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub interval_min: u16,
    /// advertising interval maximum
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub interval_max: u16,
    /// advertising interval max tx power
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub max_tx_power: i8,
}

impl Default for LeAdvertisingConfig {
    fn default() -> Self {
        Self {
            interval_min: 0,
            interval_max: 0,
            max_tx_power: 127, // 127 (0x7F) indicates no preference
        }
    }
}

/// Platform configuration for Bluetooth Low Energy scanning.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(default)]
pub struct LeScanConfig {
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub active_interval: u16,

    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub active_window: u16,

    /// Whether batched scanning is enabled; bt-host checks for firmware support before actually
    /// using the feature. This flag only enables the feature if the firmware also supports it.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub batched: bool,

    /// Maximum read delay in seconds for batched scanning.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub batch_max_read_delay_seconds: u8,

    /// Whether offloaded packet filtering is enabled for scanning; bt-host checks for firmware
    /// support before actually using the feature. This flag only enables the feature if the
    /// firmware also supports it.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub offload_filters_enabled: bool,
}

impl Default for LeScanConfig {
    fn default() -> Self {
        Self {
            active_interval: 0,
            active_window: 0,
            batched: false,
            batch_max_read_delay_seconds: 3,
            offload_filters_enabled: false,
        }
    }
}

/// Platform configuration for Bluetooth core features.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(default)]
pub struct BluetoothCoreConfig {
    /// Enable BR/EDR legacy pairing.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub legacy_pairing_enabled: bool,
    /// Which index should be used when we ask SCO traffic to be offloaded.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub sco_offload_path_index: u8,
    // TODO(https://fxbug.dev/450278813): Remove this assembly config
    /// What we should override the Vendor Capabilities version to, if necessary
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub override_vendor_capabilities_version: u16,
    /// Whether the device is BR/EDR connectable by default on boot.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub start_connectable: bool,
    /// slow advertising parameters
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub slow_advertising: LeAdvertisingConfig,
    /// fast advertising parameters
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub fast_advertising: LeAdvertisingConfig,
    /// very fast advertising parameters
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub very_fast_advertising: LeAdvertisingConfig,
    /// scan parameters
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub scan: LeScanConfig,
    /// HCI command timeout in seconds.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub hci_command_timeout: u16,
}

impl Default for BluetoothCoreConfig {
    fn default() -> Self {
        Self {
            legacy_pairing_enabled: Default::default(),
            sco_offload_path_index: 6,
            override_vendor_capabilities_version: 0,
            start_connectable: true,
            slow_advertising: LeAdvertisingConfig::default(),
            fast_advertising: LeAdvertisingConfig::default(),
            very_fast_advertising: LeAdvertisingConfig::default(),
            scan: LeScanConfig::default(),
            hci_command_timeout: 10,
        }
    }
}

/// Platform configuration options for Bluetooth.
/// The default platform configuration does not include any Bluetooth packages.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum BluetoothConfig {
    /// The standard Bluetooth configuration includes the "core" set of components that provide
    /// basic Bluetooth functionality (GATT, Advertising, etc.) and optional profiles and tools.
    /// This is expected to be the most common configuration used in the platform.
    Standard {
        /// Configuration for Bluetooth profiles. The default includes no profiles.
        #[serde(default)]
        #[serde(skip_serializing_if = "crate::common::is_default")]
        profiles: BluetoothProfilesConfig,

        /// Configuration for Bluetooth core.
        #[serde(default)]
        #[serde(skip_serializing_if = "crate::common::is_default")]
        core: BluetoothCoreConfig,

        /// Configuration for `bt-snoop`.
        #[serde(default)]
        #[serde(skip_serializing_if = "crate::common::is_default")]
        snoop: Snoop,
    },
    /// The coreless Bluetooth configuration omits the "core" set of Bluetooth components and only
    /// includes any specified standalone packages.
    /// This is typically reserved for testing or special scenarios in which minimal BT things are
    /// needed.
    Coreless {
        /// Configuration for `bt-snoop`.
        #[serde(default)]
        #[serde(skip_serializing_if = "crate::common::is_default")]
        snoop: Snoop,
    },
}

impl Default for BluetoothConfig {
    fn default() -> BluetoothConfig {
        // The default platform configuration does not include any Bluetooth packages.
        BluetoothConfig::Coreless { snoop: Snoop::None }
    }
}

impl BluetoothConfig {
    pub fn snoop(&self) -> Snoop {
        match &self {
            Self::Standard { snoop, .. } => *snoop,
            Self::Coreless { snoop, .. } => *snoop,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_serialization() {
        crate::common::tests::default_serialization_helper::<BluetoothConfig>();
    }

    #[test]
    fn test_serialize_profiles() {
        let config = BluetoothConfig::Standard {
            profiles: BluetoothProfilesConfig {
                a2dp: A2dpConfig::Enabled(A2dpConfigEnabled {
                    sink_and_source: A2dpSinkAndSourceConfig::SinkAndSource(A2dpSinkAndSource {
                        sink: A2dpSinkType::MediaPlayer,
                        source: A2dpSourceType::AudioOut,
                    }),
                }),
                rfcomm: RfcommConfig::Enabled(RfcommEnabledConfig::default()),
                ..Default::default()
            },
            core: BluetoothCoreConfig::default(),
            snoop: Snoop::Lazy,
        };
        crate::common::tests::value_serialization_helper(config);
    }

    #[test]
    fn deserialize_standard_config_no_profiles() {
        let json = serde_json::json!({
            "type": "standard",
            "snoop": "lazy",
        });

        let parsed: BluetoothConfig = serde_json::from_value(json).unwrap();
        let expected = BluetoothConfig::Standard {
            profiles: BluetoothProfilesConfig::default(),
            core: BluetoothCoreConfig::default(),
            snoop: Snoop::Lazy,
        };

        assert_eq!(parsed, expected);
        let BluetoothConfig::Standard { profiles, .. } = parsed else {
            panic!("Should be standard bluetooth");
        };
        assert!(profiles.rfcomm.enabled());
    }

    #[test]
    fn deserialize_standard_config_with_profiles() {
        // TODO(https://fxbug.dev/435705910): update "a2dp: { enabled }" when enabled is removed
        let json = serde_json::json!({
            "type": "standard",
            "snoop": "eager",
            "profiles": {
                "a2dp": {
                    "enabled": true,
                },
                "avrcp": {
                    "enabled": true,
                },
                "did": {
                    "enabled": true,
                    "vendor_id": 0,
                    "product_id": 1,
                    "version": 0x0100,
                    "primary": true,
                    "service_description": "foobar",
                },
                "hfp": {
                    "audio_gateway": {
                        "voice_recognition": true,
                        "three_way_calling": true,
                        "inband_ringtone": true,
                        "echo_canceling_and_noise_reduction": true,
                        "attach_phone_number_voice_tag": true,
                        "reject_incoming_call": true,
                        "enhanced_call_control": true,
                        "enhanced_voice_recognition_status": true,
                        "voice_recognition_text": true,
                    },
                    "hands_free": {
                        "echo_canceling_and_noise_reduction": true,
                        "three_way_calling": true,
                        "calling_line_identification": true,
                        "voice_recognition": true,
                        "remote_volume_control": true,
                        "enhanced_voice_recognition": true,
                        "voice_recognition_text": true,
                    },
                    "codecs_supported": ["cvsd", "msbc", "lc3swb"],
                    "controller_encodes": ["cvsd", "msbc", "lc3swb"],
                },
                "map": {
                    "mce_enabled": false,
                },
                "rfcomm": "enabled",
            },
            "core": {
                "legacy_pairing_enabled": true,
                "sco_offload_path_index": 1,
                "override_vendor_capabilities_version": 0x9900,
                "start_connectable": false,
                "slow_advertising": {
                    "interval_min": 100,
                    "interval_max": 200,
                    "max_tx_power": 10
                },
                "fast_advertising": {
                    "interval_min": 50,
                    "interval_max": 100,
                    "max_tx_power": 20
                },
                "very_fast_advertising": {
                    "interval_min": 20,
                    "interval_max": 40,
                    "max_tx_power": 30
                },
                "scan": {
                    "active_interval": 30,
                    "active_window": 60
                }
            },
        });

        let parsed: BluetoothConfig = serde_json::from_value(json).unwrap();
        let expected_profiles = BluetoothProfilesConfig {
            a2dp: A2dpConfig::Enabled(A2dpConfigEnabled {
                sink_and_source: A2dpSinkAndSourceConfig::Enabled(
                    A2dpSinkAndSourceDefaultEnabled { enabled: true },
                ),
            }),
            avrcp: AvrcpConfig { enabled: true },
            did: DeviceIdConfig {
                enabled: true,
                vendor_id: 0,
                product_id: 1,
                version: 0x0100,
                primary: true,
                service_description: Some("foobar".to_string()),
            },
            hfp: HfpConfig {
                audio_gateway: AudioGatewayConfig::Enabled(AudioGatewayEnabledConfig {
                    three_way_calling: true,
                    echo_canceling_and_noise_reduction: true,
                    voice_recognition: true,
                    inband_ringtone: true,
                    attach_phone_number_voice_tag: true,
                    reject_incoming_call: true,
                    enhanced_call_control: true,
                    enhanced_voice_recognition_status: true,
                    voice_recognition_text: true,
                }),
                hands_free: HandsFreeConfig::Enabled(HandsFreeEnabledConfig {
                    echo_canceling_and_noise_reduction: true,
                    three_way_calling: true,
                    calling_line_identification: true,
                    voice_recognition: true,
                    remote_volume_control: true,
                    enhanced_voice_recognition: true,
                    voice_recognition_text: true,
                }),
                codecs_supported: vec![HfpCodecId::Cvsd, HfpCodecId::Msbc, HfpCodecId::Lc3Swb],
                controller_encodes: ControllerCodecs::Codecs(vec![
                    HfpCodecId::Cvsd,
                    HfpCodecId::Msbc,
                    HfpCodecId::Lc3Swb,
                ]),
            },
            map: MapConfig { mce_enabled: false },
            rfcomm: RfcommConfig::Enabled(RfcommEnabledConfig::default()),
        };
        let expected_core = BluetoothCoreConfig {
            legacy_pairing_enabled: true,
            sco_offload_path_index: 1,
            override_vendor_capabilities_version: 0x9900,
            start_connectable: false,
            slow_advertising: LeAdvertisingConfig {
                interval_min: 100,
                interval_max: 200,
                max_tx_power: 10,
            },
            fast_advertising: LeAdvertisingConfig {
                interval_min: 50,
                interval_max: 100,
                max_tx_power: 20,
            },
            very_fast_advertising: LeAdvertisingConfig {
                interval_min: 20,
                interval_max: 40,
                max_tx_power: 30,
            },
            scan: LeScanConfig {
                active_interval: 30,
                active_window: 60,
                batched: false,
                batch_max_read_delay_seconds: 3,
                offload_filters_enabled: false,
            },
            hci_command_timeout: 10,
        };
        let expected = BluetoothConfig::Standard {
            profiles: expected_profiles,
            core: expected_core,
            snoop: Snoop::Eager,
        };

        assert_eq!(parsed, expected);
        let BluetoothConfig::Standard { profiles, .. } = parsed else {
            panic!("Should be standard bluetooth");
        };
        assert!(profiles.a2dp.sink_enabled());
        assert!(profiles.rfcomm.enabled());
    }

    #[test]
    fn deserialize_a2dp_profile_without_defaults() {
        let json = serde_json::json!({
            "type": "standard",
            "profiles": {
                "a2dp": {
                    "source": "offload",
                },
            },
        });

        let parsed: BluetoothConfig = serde_json::from_value(json).unwrap();
        let expected_profiles = BluetoothProfilesConfig {
            hfp: HfpConfig {
                audio_gateway: AudioGatewayConfig::Disabled,
                hands_free: HandsFreeConfig::Disabled,
                ..Default::default()
            },
            a2dp: A2dpConfig::Enabled(A2dpConfigEnabled {
                sink_and_source: A2dpSinkAndSourceConfig::Source(A2dpSourceOnly {
                    source: A2dpSourceType::Offload,
                }),
            }),
            rfcomm: RfcommConfig::Enabled(RfcommEnabledConfig::default()),
            ..Default::default()
        };
        let expected = BluetoothConfig::Standard {
            profiles: expected_profiles,
            core: BluetoothCoreConfig {
                legacy_pairing_enabled: false,
                sco_offload_path_index: 6,
                override_vendor_capabilities_version: 0,
                start_connectable: true,
                ..Default::default()
            },
            snoop: Snoop::None,
        };

        assert_eq!(parsed, expected);
        let BluetoothConfig::Standard { profiles, .. } = parsed else {
            panic!("Should be standard bluetooth");
        };
        assert!(!profiles.a2dp.sink_enabled());
    }

    #[test]
    fn deserialize_hfp_profiles_without_defaults() {
        let json = serde_json::json!({
            "type": "standard",
            "profiles": {
                "hfp": {
                    "audio_gateway": "enabled",
                    "hands_free": "enabled",
                },
            },
        });

        let parsed: BluetoothConfig = serde_json::from_value(json).unwrap();
        let expected_profiles = BluetoothProfilesConfig {
            hfp: HfpConfig {
                audio_gateway: AudioGatewayConfig::Enabled(AudioGatewayEnabledConfig {
                    three_way_calling: false,
                    echo_canceling_and_noise_reduction: false,
                    voice_recognition: false,
                    inband_ringtone: false,
                    attach_phone_number_voice_tag: false,
                    reject_incoming_call: false,
                    enhanced_call_control: false,
                    enhanced_voice_recognition_status: false,
                    voice_recognition_text: false,
                }),
                hands_free: HandsFreeConfig::Enabled(HandsFreeEnabledConfig {
                    echo_canceling_and_noise_reduction: false,
                    three_way_calling: false,
                    calling_line_identification: false,
                    voice_recognition: false,
                    remote_volume_control: false,
                    enhanced_voice_recognition: false,
                    voice_recognition_text: false,
                }),
                ..Default::default()
            },
            rfcomm: RfcommConfig::Enabled(RfcommEnabledConfig::default()),
            ..Default::default()
        };
        let expected = BluetoothConfig::Standard {
            profiles: expected_profiles,
            core: BluetoothCoreConfig {
                legacy_pairing_enabled: false,
                sco_offload_path_index: 6,
                override_vendor_capabilities_version: 0,
                start_connectable: true,
                ..Default::default()
            },
            snoop: Snoop::None,
        };

        assert_eq!(parsed, expected);
    }

    #[test]
    fn deserialize_coreless_config() {
        let json = serde_json::json!({
            "type": "coreless",
            "snoop": "eager",
        });

        let parsed: BluetoothConfig = serde_json::from_value(json).unwrap();
        let expected = BluetoothConfig::Coreless { snoop: Snoop::Eager };

        assert_eq!(parsed, expected);
    }

    #[test]
    fn deserialize_coreless_with_profiles_is_error() {
        let json = serde_json::json!({
            "type": "coreless",
            "profiles": "",
        });

        let parsed_result: Result<BluetoothConfig, _> = serde_json::from_value(json);
        assert!(parsed_result.is_err());
    }

    #[test]
    fn deserialize_standard_config_omitted_rfcomm() {
        let json = serde_json::json!({
            "type": "standard",
            "snoop": "lazy",
            "profiles": {
                 "avrcp": {
                    "enabled": true,
                },
            },
        });

        let parsed: BluetoothConfig = serde_json::from_value(json).expect("parse config");
        let expected_config = BluetoothConfig::Standard {
            profiles: BluetoothProfilesConfig {
                avrcp: AvrcpConfig { enabled: true },
                rfcomm: RfcommConfig::Enabled(RfcommEnabledConfig::default()),
                ..Default::default()
            },
            core: BluetoothCoreConfig::default(),
            snoop: Snoop::Lazy,
        };
        assert_eq!(parsed, expected_config);
    }

    #[test]
    fn deserialize_standard_config_disabled_rfcomm() {
        let json = serde_json::json!({
            "type": "standard",
            "snoop": "lazy",
            "profiles": {
                 "rfcomm": "disabled",
            },
        });

        let parsed: BluetoothConfig = serde_json::from_value(json).expect("parse config");
        let expected_config = BluetoothConfig::Standard {
            profiles: BluetoothProfilesConfig {
                rfcomm: RfcommConfig::Disabled,
                ..Default::default()
            },
            core: BluetoothCoreConfig::default(),
            snoop: Snoop::Lazy,
        };
        assert_eq!(parsed, expected_config);
    }

    #[test]
    fn test_deserialize_hfp_controller_encodes() {
        // Omitted case
        let json_none = serde_json::json!({
            "type": "standard",
            "profiles": {
                "hfp": {
                    "hands_free": "enabled",
                }
            }
        });
        let parsed_none: BluetoothConfig = serde_json::from_value(json_none).unwrap();
        let BluetoothConfig::Standard { profiles, .. } = parsed_none else {
            panic!("expected standard config");
        };
        assert_eq!(profiles.hfp.controller_encodes, ControllerCodecs::Default);

        // Empty list case
        let json_empty = serde_json::json!({
            "type": "standard",
            "profiles": {
                "hfp": {
                    "hands_free": "enabled",
                    "controller_encodes": [],
                }
            }
        });
        let parsed_empty: BluetoothConfig = serde_json::from_value(json_empty).unwrap();
        let BluetoothConfig::Standard { profiles, .. } = parsed_empty else {
            panic!("expected standard config");
        };
        assert_eq!(profiles.hfp.controller_encodes, ControllerCodecs::None);

        // Populated list case
        let json_populated = serde_json::json!({
            "type": "standard",
            "profiles": {
                "hfp": {
                    "hands_free": "enabled",
                    "controller_encodes": ["cvsd"],
                }
            }
        });
        let parsed_populated: BluetoothConfig = serde_json::from_value(json_populated).unwrap();
        let BluetoothConfig::Standard { profiles, .. } = parsed_populated else {
            panic!("expected standard config");
        };
        assert_eq!(
            profiles.hfp.controller_encodes,
            ControllerCodecs::Codecs(vec![HfpCodecId::Cvsd])
        );
    }

    #[test]
    fn test_deserialize_le_scan_offload_filters_enabled() {
        let json_enabled = serde_json::json!({
            "type": "standard",
            "core": {
                "scan": {
                    "offload_filters_enabled": true,
                }
            }
        });
        let parsed: BluetoothConfig = serde_json::from_value(json_enabled).unwrap();
        let BluetoothConfig::Standard { core, .. } = parsed else {
            panic!("expected standard config");
        };
        assert!(core.scan.offload_filters_enabled);

        let json_default = serde_json::json!({
            "type": "standard",
        });
        let parsed_default: BluetoothConfig = serde_json::from_value(json_default).unwrap();
        let BluetoothConfig::Standard { core, .. } = parsed_default else {
            panic!("expected standard config");
        };
        assert!(!core.scan.offload_filters_enabled);
    }

    #[test]
    fn test_deserialize_le_scan_batched() {
        let json_enabled = serde_json::json!({
            "type": "standard",
            "core": {
                "scan": {
                    "batched": true,
                }
            }
        });
        let parsed: BluetoothConfig = serde_json::from_value(json_enabled).unwrap();
        let BluetoothConfig::Standard { core, .. } = parsed else {
            panic!("expected standard config");
        };
        assert!(core.scan.batched);

        let json_default = serde_json::json!({
            "type": "standard",
        });
        let parsed_default: BluetoothConfig = serde_json::from_value(json_default).unwrap();
        let BluetoothConfig::Standard { core, .. } = parsed_default else {
            panic!("expected standard config");
        };
        assert!(!core.scan.batched);
    }

    #[test]
    fn test_deserialize_le_scan_batch_max_read_delay_seconds() {
        let json_enabled = serde_json::json!({
            "type": "standard",
            "core": {
                "scan": {
                    "batch_max_read_delay_seconds": 5,
                }
            }
        });
        let parsed: BluetoothConfig = serde_json::from_value(json_enabled).unwrap();
        let BluetoothConfig::Standard { core, .. } = parsed else {
            panic!("expected standard config");
        };
        assert_eq!(core.scan.batch_max_read_delay_seconds, 5);

        let json_default = serde_json::json!({
            "type": "standard",
        });
        let parsed_default: BluetoothConfig = serde_json::from_value(json_default).unwrap();
        let BluetoothConfig::Standard { core, .. } = parsed_default else {
            panic!("expected standard config");
        };
        assert_eq!(core.scan.batch_max_read_delay_seconds, 3);
    }
}
