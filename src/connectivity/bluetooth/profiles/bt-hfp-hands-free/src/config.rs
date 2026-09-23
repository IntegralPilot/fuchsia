// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, format_err};
use bt_hfp::audio;
use hfp_hands_free_profile_config::Config;

#[derive(Clone, Copy, Default)]
pub struct HandsFreeFeatureSupport {
    pub ec_or_nr: bool,
    pub call_waiting_or_three_way_calling: bool,
    pub cli_presentation_capability: bool,
    pub voice_recognition_activation: bool,
    pub remote_volume_control: bool,
    pub wide_band_speech: bool,
    pub enhanced_voice_recognition: bool,
    pub enhanced_voice_recognition_with_text: bool,
}

#[derive(Clone, Copy)]
pub struct AudioConfig {
    pub controller_encoding_cvsd: bool,
    pub controller_encoding_msbc: bool,
    pub offload_type: audio::OffloadType,
}

#[derive(Clone, Copy)]
pub struct Configs {
    pub hands_free_features: HandsFreeFeatureSupport,
    pub audio: AudioConfig,
}

impl Configs {
    pub fn load() -> Result<Self, Error> {
        let config = Config::take_from_startup_handle();
        Self::from_raw_config(&config)
    }

    fn from_raw_config(str_config: &Config) -> Result<Self, Error> {
        let hands_free_features = HandsFreeFeatureSupport::from_raw_config(str_config)?;
        let audio = AudioConfig::from_raw_config(str_config)?;
        Ok(Self { hands_free_features, audio })
    }
}

impl HandsFreeFeatureSupport {
    fn from_raw_config(str_config: &Config) -> Result<Self, Error> {
        let mut config = Self::default();
        config.ec_or_nr = str_config.ec_or_nr;
        config.call_waiting_or_three_way_calling = str_config.call_waiting_or_three_way_calling;
        config.cli_presentation_capability = str_config.cli_presentation_capability;
        config.voice_recognition_activation = str_config.voice_recognition_activation;
        config.remote_volume_control = str_config.remote_volume_control;
        config.wide_band_speech = str_config.wide_band_speech;
        config.enhanced_voice_recognition = str_config.enhanced_voice_recognition;
        config.enhanced_voice_recognition_with_text =
            str_config.enhanced_voice_recognition_with_text;
        Ok(config)
    }
}

impl AudioConfig {
    fn from_raw_config(str_config: &Config) -> Result<Self, Error> {
        let controller_encoding_cvsd = str_config.controller_encoding_cvsd;
        let controller_encoding_msbc = str_config.controller_encoding_msbc;
        let offload_type;
        match str_config.offload_type.as_str() {
            "dai" => offload_type = audio::OffloadType::Dai,
            "codec" => offload_type = audio::OffloadType::Codec,
            _ => return Err(format_err!("Unknown offload type: {}", str_config.offload_type)),
        }
        let config =
            AudioConfig { controller_encoding_cvsd, controller_encoding_msbc, offload_type };
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    fn default_features() {
        let config = HandsFreeFeatureSupport::default();
        assert!(!config.ec_or_nr);
        assert!(!config.call_waiting_or_three_way_calling);
        assert!(!config.cli_presentation_capability);
        assert!(!config.voice_recognition_activation);
        assert!(!config.remote_volume_control);
        assert!(!config.wide_band_speech);
        assert!(!config.enhanced_voice_recognition);
        assert!(!config.enhanced_voice_recognition_with_text);
    }

    #[fuchsia::test]
    fn load_from_config() {
        let config = Config {
            ec_or_nr: true,
            call_waiting_or_three_way_calling: true,
            cli_presentation_capability: true,
            voice_recognition_activation: true,
            remote_volume_control: true,
            wide_band_speech: true,
            enhanced_voice_recognition: true,
            enhanced_voice_recognition_with_text: true,
            controller_encoding_cvsd: true,
            controller_encoding_msbc: true,
            offload_type: "dai".to_string(),
        };
        let configs = Configs::from_raw_config(&config).unwrap();
        let hands_free_config = configs.hands_free_features;
        let audio_config = configs.audio;

        assert_eq!(hands_free_config.ec_or_nr, true);
        assert_eq!(hands_free_config.call_waiting_or_three_way_calling, true);
        assert_eq!(hands_free_config.cli_presentation_capability, true);
        assert_eq!(hands_free_config.voice_recognition_activation, true);
        assert_eq!(hands_free_config.remote_volume_control, true);
        assert_eq!(hands_free_config.wide_band_speech, true);
        assert_eq!(hands_free_config.enhanced_voice_recognition, true);
        assert_eq!(hands_free_config.enhanced_voice_recognition_with_text, true);

        assert_eq!(audio_config.controller_encoding_cvsd, true);
        assert_eq!(audio_config.controller_encoding_msbc, true);
        assert_eq!(audio_config.offload_type, audio::OffloadType::Dai);
    }
}
