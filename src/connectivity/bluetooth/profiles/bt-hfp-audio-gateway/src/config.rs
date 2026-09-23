// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bt_hfp::codec_id::CodecId;
use fuchsia_inspect as inspect;
use fuchsia_inspect_derive::{AttachError, Inspect};
use std::collections::HashSet;

/// Configuration of optional feature support.
/// This list of features is derived from the optional features in HFP v1.8, Table 3.1.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(test, derive(Default))]
pub struct AudioGatewayFeatureSupport {
    pub reject_incoming_voice_call: bool,
    pub three_way_calling: bool,
    pub in_band_ringtone: bool,
    pub echo_canceling_and_noise_reduction: bool,
    pub voice_recognition: bool,
    pub attach_phone_number_to_voice_tag: bool,
    pub enhanced_call_controls: bool,
    pub wide_band_speech: bool,
    pub enhanced_voice_recognition: bool,
    pub enhanced_voice_recognition_with_text: bool,
}

impl Inspect for AudioGatewayFeatureSupport {
    /// iattach for AudioGatewayFeatureSupport creates an immutable record of
    /// the inspect data without the need to store the node separately from the
    /// parent node. For this reason, the lifetime of the inspect data is tied
    /// to the lifetime of the `parent` node.
    fn iattach(self, parent: &inspect::Node, name: impl AsRef<str>) -> Result<(), AttachError> {
        let init = |node: &inspect::Node| {
            node.record_bool("reject_incoming_voice_call", self.reject_incoming_voice_call);
            node.record_bool("three_way_calling", self.three_way_calling);
            node.record_bool("in_band_ringtone", self.in_band_ringtone);
            node.record_bool(
                "echo_canceling_and_noise_reduction",
                self.echo_canceling_and_noise_reduction,
            );
            node.record_bool("voice_recognition", self.voice_recognition);
            node.record_bool(
                "attach_phone_number_to_voice_tag",
                self.attach_phone_number_to_voice_tag,
            );
            node.record_bool("enhanced_call_controls", self.enhanced_call_controls);
            node.record_bool("wide_band_speech", self.wide_band_speech);
            node.record_bool("enhanced_voice_recognition", self.enhanced_voice_recognition);
            node.record_bool(
                "enhanced_voice_recognition_with_text",
                self.enhanced_voice_recognition_with_text,
            );
        };
        let _child = parent.record_child(name.as_ref(), init);
        Ok(())
    }
}

impl From<hfp_profile_config::Config> for AudioGatewayFeatureSupport {
    fn from(src: hfp_profile_config::Config) -> Self {
        AudioGatewayFeatureSupport {
            reject_incoming_voice_call: src.reject_incoming_voice_call,
            three_way_calling: src.three_way_calling,
            // TODO(https://fxbug.dev/42155342): not supported yet
            in_band_ringtone: false,
            echo_canceling_and_noise_reduction: src.echo_canceling_and_noise_reduction,
            // TODO(https://fxbug.dev/42144778): not supported yet
            voice_recognition: false,
            attach_phone_number_to_voice_tag: src.attach_phone_number_to_voice_tag,
            enhanced_call_controls: src.enhanced_call_controls,
            wide_band_speech: src.wide_band_speech,
            // TODO(https://fxbug.dev/42144778): not supported yet
            enhanced_voice_recognition: false,
            // TODO(https://fxbug.dev/42144778): not supported yet
            enhanced_voice_recognition_with_text: false,
        }
    }
}

impl AudioGatewayFeatureSupport {
    /// Load AudioGatewayFeatureSupport from package config data directory.
    pub fn load_from_config(config: hfp_profile_config::Config) -> Self {
        config.into()
    }
}

pub fn controller_codecs(config: &hfp_profile_config::Config) -> HashSet<CodecId> {
    let mut controller_codecs = HashSet::new();
    if config.controller_encoding_cvsd {
        let _ = controller_codecs.insert(CodecId::CVSD);
    }
    if config.controller_encoding_msbc {
        let _ = controller_codecs.insert(CodecId::MSBC);
    }
    controller_codecs
}

#[cfg(test)]
mod tests {
    use super::*;
    use diagnostics_assertions::assert_data_tree;

    #[fuchsia::test]
    fn load_from_config() {
        let config = AudioGatewayFeatureSupport::load_from_config(hfp_profile_config::Config {
            reject_incoming_voice_call: false,
            three_way_calling: false,
            in_band_ringtone: false,
            echo_canceling_and_noise_reduction: true,
            voice_recognition: false,
            attach_phone_number_to_voice_tag: false,
            enhanced_call_controls: false,
            wide_band_speech: true,
            enhanced_voice_recognition: false,
            enhanced_voice_recognition_with_text: false,
            controller_encoding_cvsd: true,
            controller_encoding_msbc: true,
            offload_type: "dai".to_string(),
        });
        assert_eq!(
            config,
            AudioGatewayFeatureSupport {
                reject_incoming_voice_call: false,
                three_way_calling: false,
                in_band_ringtone: false,
                echo_canceling_and_noise_reduction: true,
                voice_recognition: false,
                attach_phone_number_to_voice_tag: false,
                enhanced_call_controls: false,
                wide_band_speech: true,
                enhanced_voice_recognition: false,
                enhanced_voice_recognition_with_text: false,
            }
        );
    }

    #[fuchsia::test]
    async fn expected_inspect_tree() {
        let inspector = inspect::Inspector::default();
        assert_data_tree!(inspector, root: {});

        let mut config = AudioGatewayFeatureSupport::default();
        config.three_way_calling = true;
        config.in_band_ringtone = true;

        let _ = config.iattach(&inspector.root(), "audio_gateway_feature_support").unwrap();
        assert_data_tree!(inspector, root: {
            audio_gateway_feature_support: {
                reject_incoming_voice_call: false,
                three_way_calling: true,
                in_band_ringtone: true,
                echo_canceling_and_noise_reduction: false,
                voice_recognition: false,
                attach_phone_number_to_voice_tag: false,
                enhanced_call_controls: false,
                wide_band_speech: false,
                enhanced_voice_recognition: false,
                enhanced_voice_recognition_with_text: false,
            }
        });
    }
}
