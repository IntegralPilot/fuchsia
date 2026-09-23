// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::indexes::{
    STREAM_CONFIG_INDEX_HEADSET_IN, STREAM_CONFIG_INDEX_HEADSET_OUT, STREAM_CONFIG_INDEX_MICS,
    STREAM_CONFIG_INDEX_SPEAKERS, StreamConfigIndex,
};
use anyhow::Error;
use std::collections::HashMap;

#[derive(Eq, Hash, PartialEq, Debug)]
pub struct Device {
    /// Device manufacturer name.
    pub manufacturer: String,

    /// Device product name.
    pub product: String,

    /// Is codec or DAI.
    pub is_codec: bool,

    /// Hardwired.
    pub hardwired: bool,

    /// Is input.
    pub is_input: bool,
}

#[derive(Default)]
pub struct Config {
    /// Indexes to the available StreamConfigs.
    pub stream_config_indexes: HashMap<Device, Vec<StreamConfigIndex>>,
}

impl Config {
    pub fn new() -> Result<Self, Error> {
        Ok(Self { stream_config_indexes: HashMap::new() })
    }

    #[cfg(test)]
    pub fn load_device(&mut self, device: Device, index: StreamConfigIndex) {
        self.stream_config_indexes.entry(device).or_default().push(index);
    }
    pub fn load(&mut self) -> Result<(), Error> {
        // TODO(https://fxbug.dev/42177443): Add configurability per product/device instead of hardcoding.
        let indexes = HashMap::from([
            // Codecs:
            (
                Device {
                    manufacturer: "Maxim".to_string(),
                    product: "MAX98373".to_string(),
                    hardwired: true,
                    is_codec: true,
                    is_input: false,
                },
                vec![STREAM_CONFIG_INDEX_SPEAKERS, STREAM_CONFIG_INDEX_SPEAKERS],
            ),
            (
                Device {
                    manufacturer: "Dialog".to_string(),
                    product: "DA7219".to_string(),
                    hardwired: false,
                    is_codec: true,
                    is_input: true,
                },
                vec![STREAM_CONFIG_INDEX_HEADSET_IN],
            ),
            (
                Device {
                    manufacturer: "Dialog".to_string(),
                    product: "DA7219".to_string(),
                    hardwired: false,
                    is_codec: true,
                    is_input: false,
                },
                vec![STREAM_CONFIG_INDEX_HEADSET_OUT],
            ),
            // DAIs:
            (
                Device {
                    manufacturer: "Intel".to_string(),
                    product: "Builtin Speakers".to_string(),
                    hardwired: true,
                    is_codec: false,
                    is_input: false,
                },
                vec![STREAM_CONFIG_INDEX_SPEAKERS],
            ),
            (
                Device {
                    manufacturer: "Intel".to_string(),
                    product: "Builtin Microphones".to_string(),
                    hardwired: true,
                    is_codec: false,
                    is_input: true,
                },
                vec![STREAM_CONFIG_INDEX_MICS],
            ),
            (
                Device {
                    manufacturer: "Intel".to_string(),
                    product: "Builtin Headphone Jack Output".to_string(),
                    hardwired: true,
                    is_codec: false,
                    is_input: false,
                },
                vec![STREAM_CONFIG_INDEX_HEADSET_OUT],
            ),
            (
                Device {
                    manufacturer: "Intel".to_string(),
                    product: "Builtin Headphone Jack Input".to_string(),
                    hardwired: true,
                    is_codec: false,
                    is_input: true,
                },
                vec![STREAM_CONFIG_INDEX_HEADSET_IN],
            ),
        ]);
        self.stream_config_indexes = indexes;
        Ok(())
    }
}
