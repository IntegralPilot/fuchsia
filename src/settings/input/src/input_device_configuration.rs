// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::types::{DeviceStateSource, InputDeviceType};
use serde::Deserialize;
use settings_common::config::default_settings::DefaultSetting;
use settings_common::inspect::config_logger::InspectConfigLogger;
use std::rc::Rc;
use std::sync::Mutex;

#[derive(PartialEq, Debug, Default, Clone, Deserialize)]
pub struct InputConfiguration {
    /// List of input devices that are present on this product.
    pub devices: Vec<InputDeviceConfiguration>,
}

#[derive(PartialEq, Debug, Clone, Deserialize)]
pub struct InputDeviceConfiguration {
    /// Name of the device.
    ///
    /// Must be unique per device type. Can be empty if there is only one
    /// input device of this type.
    pub device_name: String,

    /// The type of input device, e.g. MICROPHONE.
    pub device_type: InputDeviceType,

    /// The sources (e.g. HARDWARE) with their corresponding states.
    pub source_states: Vec<SourceState>,

    /// The number representing the states that are toggleable by a client.
    /// This is the sum of the bitflags that are set.
    pub mutable_toggle_state: u64,
}

#[derive(PartialEq, Debug, Clone, Deserialize)]
pub struct SourceState {
    /// The source, e.g. HARDWARE.
    pub source: DeviceStateSource,

    /// The number representing the state for the source. This is the sum of
    /// the bitflags that are set.
    pub state: u64,
}
pub fn build_input_default_settings(
    config_logger: Rc<Mutex<InspectConfigLogger>>,
) -> DefaultSetting<InputConfiguration, &'static str> {
    DefaultSetting::new(
        Some(InputConfiguration::default()),
        "/config/data/input_device_config.json",
        config_logger,
    )
}
