// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::format_err;
use assembly_constants::BoardFeature;

use crate::subsystems::prelude::*;
use assembly_config_capabilities::{Config, ConfigValueType};
use assembly_config_schema::platform_settings::bluetooth_config::{
    A2dpConfig, A2dpSinkAndSource, A2dpSinkAndSourceConfig, A2dpSinkAndSourceDefaultEnabled,
    A2dpSourceOnly, AudioGatewayConfig, BluetoothConfig, BluetoothProfilesConfig, HandsFreeConfig,
    HfpCodecId, Snoop,
};
use assembly_config_schema::platform_settings::media_config::{AudioConfig, PlatformMediaConfig};

fn get_source_type_str(sink_and_source: &A2dpSinkAndSourceConfig) -> String {
    match sink_and_source {
        A2dpSinkAndSourceConfig::Enabled(A2dpSinkAndSourceDefaultEnabled { enabled: true }) => {
            "audio_out".to_owned()
        }
        A2dpSinkAndSourceConfig::Source(A2dpSourceOnly { source })
        | A2dpSinkAndSourceConfig::SinkAndSource(A2dpSinkAndSource { source, .. }) => {
            serde_json::Value::from(*source).as_str().unwrap().to_string()
        }
        _ => "none".to_owned(),
    }
}

// Common values from BT and media configs used by HFP AG and HF
struct HfpAudioConfig {
    hfp_supported_codecs: Vec<HfpCodecId>,
    controller_encodes: Vec<HfpCodecId>,
    offload_type: String,
}

fn get_hfp_audio_config(
    profiles: &BluetoothProfilesConfig,
    media_config: &PlatformMediaConfig,
) -> anyhow::Result<HfpAudioConfig> {
    // TODO(https://fxbug.dev/362573469): Bail if the features don't make sense
    // (VoiceRecognitionText without EnhancedVoiceRecognitionStatus, for example)
    let hfp_supported_codecs = if profiles.hfp.codecs_supported.is_empty() {
        vec![HfpCodecId::Cvsd, HfpCodecId::Msbc, HfpCodecId::Lc3Swb]
    } else {
        profiles.hfp.codecs_supported.clone()
    };

    let controller_encodes = profiles.hfp.controller_encodes.codecs();

    let offload_type = match media_config.audio {
        Some(AudioConfig::FullStack(_)) => String::from("dai"),
        Some(AudioConfig::DeviceRegistry(_)) => String::from("codec"),
        None => return Err(format_err!("Bluetooth HFP requires an audio stack")),
    };

    Ok(HfpAudioConfig { hfp_supported_codecs, controller_encodes, offload_type })
}

pub(crate) struct BluetoothSubsystemConfig;
impl DefineSubsystemConfiguration<(&BluetoothConfig, &PlatformMediaConfig)>
    for BluetoothSubsystemConfig
{
    fn define_configuration(
        context: &ConfigurationContext<'_>,
        config: &(&BluetoothConfig, &PlatformMediaConfig),
        builder: &mut dyn ConfigurationBuilder,
    ) -> anyhow::Result<()> {
        let (config, media_config) = config;
        // Snoop is only useful when Inspect filtering is turned on. In practice, this is in Eng &
        // UserDebug builds.
        match (context.build_type, config.snoop()) {
            (_, Snoop::None) => {}
            (BuildType::User, _) => return Err(format_err!("Snoop forbidden on user builds")),
            (_, Snoop::Eager) => {
                builder.platform_bundle("bluetooth_snoop_eager")?;
            }
            (_, Snoop::Lazy) => {
                builder.platform_bundle("bluetooth_snoop_lazy")?;
            }
        }

        // Include bt-transport-uart driver through a platform AIB.
        if context.board_config.provides_feature(BoardFeature::BtTransportUart)
            && (*context.feature_set_level == FeatureSetLevel::Standard
                || *context.feature_set_level == FeatureSetLevel::Utility)
        {
            builder.platform_bundle("bt_transport_uart_driver")?;
        }

        let BluetoothConfig::Standard { profiles, core, snoop: _ } = config else {
            return Ok(());
        };

        // Bluetooth Core & Profile packages can only be added to the Standard platform
        // service level.
        if *context.feature_set_level != FeatureSetLevel::Standard {
            return Err(format_err!(
                "Bluetooth core & profiles are forbidden on non-Standard service levels"
            ));
        }
        builder.platform_bundle("bluetooth_core")?;
        builder.set_config_capability(
            "fuchsia.bluetooth.LegacyPairing",
            Config::new(ConfigValueType::Bool, core.legacy_pairing_enabled.into()),
        )?;
        builder.set_config_capability(
            "fuchsia.bluetooth.ScoOffloadPathIndex",
            Config::new(ConfigValueType::Uint8, core.sco_offload_path_index.into()),
        )?;
        builder.set_config_capability(
            "fuchsia.bluetooth.OverrideVendorCapabilitiesVersion",
            Config::new(ConfigValueType::Uint16, core.override_vendor_capabilities_version.into()),
        )?;
        builder.set_config_capability(
            "fuchsia.bluetooth.LeSlowAdvIntervalMin",
            Config::new(ConfigValueType::Uint16, core.slow_advertising.interval_min.into()),
        )?;
        builder.set_config_capability(
            "fuchsia.bluetooth.LeSlowAdvIntervalMax",
            Config::new(ConfigValueType::Uint16, core.slow_advertising.interval_max.into()),
        )?;
        builder.set_config_capability(
            "fuchsia.bluetooth.LeSlowAdvMaxTxPower",
            Config::new(ConfigValueType::Int8, core.slow_advertising.max_tx_power.into()),
        )?;
        builder.set_config_capability(
            "fuchsia.bluetooth.LeFastAdvIntervalMin",
            Config::new(ConfigValueType::Uint16, core.fast_advertising.interval_min.into()),
        )?;
        builder.set_config_capability(
            "fuchsia.bluetooth.LeFastAdvIntervalMax",
            Config::new(ConfigValueType::Uint16, core.fast_advertising.interval_max.into()),
        )?;
        builder.set_config_capability(
            "fuchsia.bluetooth.LeFastAdvMaxTxPower",
            Config::new(ConfigValueType::Int8, core.fast_advertising.max_tx_power.into()),
        )?;
        builder.set_config_capability(
            "fuchsia.bluetooth.LeVeryFastAdvIntervalMin",
            Config::new(ConfigValueType::Uint16, core.very_fast_advertising.interval_min.into()),
        )?;
        builder.set_config_capability(
            "fuchsia.bluetooth.LeVeryFastAdvIntervalMax",
            Config::new(ConfigValueType::Uint16, core.very_fast_advertising.interval_max.into()),
        )?;
        builder.set_config_capability(
            "fuchsia.bluetooth.LeVeryFastAdvMaxTxPower",
            Config::new(ConfigValueType::Int8, core.very_fast_advertising.max_tx_power.into()),
        )?;
        builder.set_config_capability(
            "fuchsia.bluetooth.LeActiveScanInterval",
            Config::new(ConfigValueType::Uint16, core.scan.active_interval.into()),
        )?;
        builder.set_config_capability(
            "fuchsia.bluetooth.LeActiveScanWindow",
            Config::new(ConfigValueType::Uint16, core.scan.active_window.into()),
        )?;
        builder.set_config_capability(
            "fuchsia.bluetooth.LeBatchedScanningEnabled",
            Config::new(ConfigValueType::Bool, core.scan.batched.into()),
        )?;
        builder.set_config_capability(
            "fuchsia.bluetooth.LeScanBatchMaxReadDelaySeconds",
            Config::new(ConfigValueType::Uint8, core.scan.batch_max_read_delay_seconds.into()),
        )?;
        builder.set_config_capability(
            "fuchsia.bluetooth.LeScanOffloadFiltersEnabled",
            Config::new(ConfigValueType::Bool, core.scan.offload_filters_enabled.into()),
        )?;
        builder.set_config_capability(
            "fuchsia.bluetooth.HciCommandTimeout",
            Config::new(ConfigValueType::Uint16, core.hci_command_timeout.into()),
        )?;
        // Fast Pair Provider is currently disabled by default.
        // TODO(https://fxbug.dev/253626392): Add a Fast Pair config to the schema and use it here.
        builder.set_config_capability(
            "fuchsia.bluetooth.FastPairProvider",
            Config::new(ConfigValueType::Bool, serde_json::Value::Bool(false)),
        )?;
        builder.set_config_capability(
            "fuchsia.bluetooth.Rfcomm",
            Config::new(ConfigValueType::Bool, profiles.rfcomm.enabled().into()),
        )?;

        // `bt-gap` is included as part of the `bluetooth_core` platform bundle (packaged
        // with `bt-init`).
        // While `bredr_connectable` is the only configurable field, we must override the entire
        // config. Default values are taken from bt-gap's default.
        builder
            .package("bt-init")
            .component("meta/bt-gap.cm")?
            .field("le_privacy", true)?
            .field("le_background_scanning", false)?
            .field("le_security_mode", "Mode1")?
            .field("bredr_connectable", core.start_connectable)?
            .field("bredr_security_mode", "Mode4")?;

        if profiles.rfcomm.enabled() {
            builder.platform_bundle("bluetooth_rfcomm")?;
        }
        // Bail if RFCOMM is required by any enabled profiles but is not enabled in the schema.
        if profiles.requires_rfcomm() && !profiles.rfcomm.enabled() {
            return Err(format_err!("RFCOMM must be enabled when HFP or MAP are enabled"));
        }

        if let A2dpConfig::Enabled(a2dp) = profiles.a2dp {
            builder.platform_bundle("bluetooth_a2dp")?;

            let mut a2dp_config = builder.package("bt-a2dp").component("meta/bt-a2dp.cm")?;
            a2dp_config
                .field("domain", "Bluetooth")?
                .field("enable_avrcp_target", true)?
                .field("enable_aac", true)?
                .field("initiator_delay", 500)?
                .field("channel_mode", "basic")?
                .field("enable_sink", a2dp.sink_enabled())?
                .field("source_type", get_source_type_str(&a2dp.sink_and_source))?;
        }
        if profiles.avrcp.enabled {
            builder.platform_bundle("bluetooth_avrcp")?;
        }
        if profiles.did.enabled {
            builder.platform_bundle("bluetooth_device_id")?;
            let mut did_config =
                builder.package("bt-device-id").component("meta/bt-device-id.cm")?;
            did_config
                .field("vendor_id", profiles.did.vendor_id)?
                .field("product_id", profiles.did.product_id)?
                .field("version", profiles.did.version)?
                .field("primary", profiles.did.primary)?
                .field(
                    "service_description",
                    profiles.did.service_description.clone().unwrap_or(String::new()),
                )?;
        }

        if let AudioGatewayConfig::Enabled(hfp_ag_features) = &profiles.hfp.audio_gateway {
            let audio_config = get_hfp_audio_config(profiles, media_config)?;
            builder.platform_bundle("bluetooth_hfp_ag")?;
            let mut hfp_ag_config = builder
                .package("bt-hfp-audio-gateway")
                .component("meta/bt-hfp-audio-gateway.cm")?;
            hfp_ag_config
                .field("three_way_calling", hfp_ag_features.three_way_calling)?
                .field("reject_incoming_voice_call", hfp_ag_features.reject_incoming_call)?
                .field("in_band_ringtone", hfp_ag_features.inband_ringtone)?
                .field("voice_recognition", hfp_ag_features.voice_recognition)?
                .field(
                    "echo_canceling_and_noise_reduction",
                    hfp_ag_features.echo_canceling_and_noise_reduction,
                )?
                .field(
                    "attach_phone_number_to_voice_tag",
                    hfp_ag_features.attach_phone_number_voice_tag,
                )?
                .field("enhanced_call_controls", hfp_ag_features.enhanced_call_control)?
                .field(
                    "enhanced_voice_recognition",
                    hfp_ag_features.enhanced_voice_recognition_status,
                )?
                .field(
                    "enhanced_voice_recognition_with_text",
                    hfp_ag_features.voice_recognition_text,
                )?
                .field(
                    "controller_encoding_cvsd",
                    audio_config.controller_encodes.contains(&HfpCodecId::Cvsd),
                )?
                .field(
                    "controller_encoding_msbc",
                    audio_config.controller_encodes.contains(&HfpCodecId::Msbc),
                )?
                .field(
                    "wide_band_speech",
                    audio_config.hfp_supported_codecs.contains(&HfpCodecId::Msbc),
                )?
                .field("offload_type", audio_config.offload_type)?;
        }
        if let HandsFreeConfig::Enabled(hfp_hf_features) = &profiles.hfp.hands_free {
            let audio_config = get_hfp_audio_config(profiles, media_config)?;
            builder.platform_bundle("bluetooth_hfp_hf")?;

            let mut hfp_hf_config =
                builder.package("bt-hfp-hands-free").component("meta/bt-hfp-hands-free.cm")?;
            hfp_hf_config
                .field("ec_or_nr", hfp_hf_features.echo_canceling_and_noise_reduction)?
                .field("call_waiting_or_three_way_calling", hfp_hf_features.three_way_calling)?
                .field("cli_presentation_capability", hfp_hf_features.calling_line_identification)?
                .field("voice_recognition_activation", hfp_hf_features.voice_recognition)?
                .field("remote_volume_control", hfp_hf_features.remote_volume_control)?
                .field(
                    "wide_band_speech",
                    audio_config.hfp_supported_codecs.contains(&HfpCodecId::Msbc),
                )?
                .field("enhanced_voice_recognition", hfp_hf_features.enhanced_voice_recognition)?
                .field(
                    "enhanced_voice_recognition_with_text",
                    hfp_hf_features.voice_recognition_text,
                )?
                .field(
                    "controller_encoding_cvsd",
                    audio_config.controller_encodes.contains(&HfpCodecId::Cvsd),
                )?
                .field(
                    "controller_encoding_msbc",
                    audio_config.controller_encodes.contains(&HfpCodecId::Msbc),
                )?
                .field("offload_type", audio_config.offload_type)?;
        }
        if profiles.map.mce_enabled {
            builder.platform_bundle("bluetooth_map_mce")?;
        }

        if *context.feature_set_level == FeatureSetLevel::Standard
            && *context.build_type == BuildType::Eng
        {
            builder.platform_bundle("bluetooth_affordances")?;
            builder.platform_bundle("bluetooth_pandora")?;

            if !profiles.a2dp.enabled()
                && matches!(media_config.audio, Some(AudioConfig::FullStack(_)))
            {
                builder.platform_bundle("bluetooth_a2dp_with_consumer")?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use assembly_config_schema::platform_settings::bluetooth_config::{
        A2dpSinkAndSource, A2dpSinkAndSourceConfig, A2dpSinkAndSourceDefaultEnabled, A2dpSinkOnly,
        A2dpSinkType, A2dpSourceOnly, A2dpSourceType,
    };

    #[test]
    fn test_a2dp_source_type_str() {
        let config =
            A2dpSinkAndSourceConfig::Enabled(A2dpSinkAndSourceDefaultEnabled { enabled: true });
        assert_eq!(get_source_type_str(&config), "audio_out");
        let config =
            A2dpSinkAndSourceConfig::Source(A2dpSourceOnly { source: A2dpSourceType::AudioOut });
        assert_eq!(get_source_type_str(&config), "audio_out");
        let config =
            A2dpSinkAndSourceConfig::Source(A2dpSourceOnly { source: A2dpSourceType::BigBen });
        assert_eq!(get_source_type_str(&config), "big_ben");
        let config = A2dpSinkAndSourceConfig::SinkAndSource(A2dpSinkAndSource {
            source: A2dpSourceType::Offload,
            sink: A2dpSinkType::MediaPlayer,
        });
        assert_eq!(get_source_type_str(&config), "offload");
        let config =
            A2dpSinkAndSourceConfig::Sink(A2dpSinkOnly { sink: A2dpSinkType::MediaPlayer });
        assert_eq!(get_source_type_str(&config), "none");
    }
}
