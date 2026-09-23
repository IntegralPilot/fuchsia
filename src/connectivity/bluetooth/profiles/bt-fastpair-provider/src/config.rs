// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::format_err;
use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig};
use base64::engine::{DecodePaddingMode, Engine as _};
use fastpair_provider_config::Config as StructuredConfig;
use p256::SecretKey;

use crate::types::keys::private_key_from_bytes;
use crate::types::{Error, ModelId};

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub model_id: ModelId,
    pub firmware_revision: String,
    pub local_private_key: SecretKey,
}

impl Config {
    pub fn load() -> Result<Self, Error> {
        let config = StructuredConfig::take_from_startup_handle();

        // Allow for payloads that do not have canonical padding.
        let base64_indifferent_padding = GeneralPurpose::new(
            &base64::alphabet::STANDARD,
            GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
        );

        let private_key_bytes = base64_indifferent_padding
            .decode(config.private_key)
            .map_err(|e| format_err!("Couldn't decode base64 key: {:?}", e))?;

        Ok(Self {
            model_id: ModelId::try_from(config.model_id)?,
            firmware_revision: config.firmware_revision,
            local_private_key: private_key_from_bytes(private_key_bytes)?,
        })
    }

    #[cfg(test)]
    pub fn example_config() -> Self {
        Self {
            model_id: ModelId::try_from(1).expect("valid ID"),
            firmware_revision: "1.0.0".to_string(),
            local_private_key: private_key_from_bytes(
                crate::types::keys::LOCAL_PRIVATE_KEY_BYTES.to_vec(),
            )
            .expect("valid private key"),
        }
    }
}
