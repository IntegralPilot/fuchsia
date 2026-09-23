// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::hf_indicators::HfIndicators;

use crate::config::HandsFreeFeatureSupport;
use crate::features::{AgFeatures, CallHoldAction, HfFeatures};

use bt_hfp::codec_id::CodecId;

#[derive(Clone, Debug)]
pub struct ProcedureManipulatedState {
    /// Determines whether the SLCI procedure has completed and we
    /// can proceed to do other procedures.
    // TODO(https://fxbug.dev/332390332): Remove or explain #[allow(dead_code)].
    #[allow(dead_code)]
    pub initialized: bool,
    /// Features that the HF supports.
    pub hf_features: HfFeatures,
    /// Features that the AG supports.
    pub ag_features: AgFeatures,
    /// The current indicator status of the HF
    pub hf_indicators: HfIndicators,
    /// Determines whether the indicator status update function is enabled.
    // TODO(https://fxbug.dev/332390332): Remove or explain #[allow(dead_code)].
    #[allow(dead_code)]
    pub indicators_update_enabled: bool,
    /// Features supported from the three-way calling or call waiting
    pub three_way_features: Vec<CallHoldAction>,
    /// The negotiated codec for this connection between the AG and HF.
    pub selected_codec: Option<CodecId>,
    /// The codec(s) supported by the HF.
    pub supported_codecs: Vec<CodecId>,
}

impl ProcedureManipulatedState {
    pub fn new(hf_feature_support: HandsFreeFeatureSupport) -> Self {
        Self {
            initialized: false,
            hf_features: hf_feature_support.into(),
            ag_features: AgFeatures::default(),
            hf_indicators: HfIndicators::default(),
            indicators_update_enabled: true,
            three_way_features: Vec::new(),
            selected_codec: None,
            // TODO(https://fxbug.dev/130963) Make this configurable.
            // By default, we support the CVSD and MSBC codecs.
            supported_codecs: vec![CodecId::CVSD, CodecId::MSBC],
        }
    }

    pub fn supports_codec_negotiation(&self) -> bool {
        self.ag_features.contains(AgFeatures::CODEC_NEGOTIATION)
            && self.hf_features.contains(HfFeatures::CODEC_NEGOTIATION)
    }

    pub fn supports_three_way_calling(&self) -> bool {
        self.ag_features.contains(AgFeatures::THREE_WAY_CALLING)
            && self.hf_features.contains(HfFeatures::THREE_WAY_CALLING)
    }

    pub fn supports_hf_indicators(&self) -> bool {
        self.ag_features.contains(AgFeatures::HF_INDICATORS)
            && self.hf_features.contains(HfFeatures::HF_INDICATORS)
    }

    #[cfg(test)]
    pub fn load_with_set_features(hf_features: HfFeatures, ag_features: AgFeatures) -> Self {
        Self { hf_features, ag_features, ..ProcedureManipulatedState::default() }
    }
}

impl Default for ProcedureManipulatedState {
    fn default() -> Self {
        let hf_feature_support = HandsFreeFeatureSupport::default();
        ProcedureManipulatedState::new(hf_feature_support)
    }
}
