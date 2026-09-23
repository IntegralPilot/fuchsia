// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// LINT.IfChange
/// Generic types of supported input devices.
#[derive(
    Debug, Clone, Copy, Eq, Hash, PartialEq, PartialOrd, Ord, Deserialize, Serialize, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum InputDeviceType {
    Keyboard,
    LightSensor,
    #[serde(rename = "button")]
    ConsumerControls,
    Mouse,
    #[serde(rename = "touchscreen")]
    Touch,
}
// LINT.ThenChange(/build/bazel_sdk/bazel_rules_fuchsia/fuchsia/private/assembly/fuchsia_product_configuration.bzl)

impl InputDeviceType {
    /// Parses a list of supported `InputDeviceType`s from a structured configuration
    /// `supported_input_devices` list. Unknown device types are logged and skipped.
    pub fn list_from_structured_config_list<'a, V, T>(list: V) -> Vec<Self>
    where
        V: IntoIterator<Item = &'a T>,
        T: AsRef<str> + 'a,
    {
        list.into_iter()
            .filter_map(|device| match device.as_ref() {
                "button" => Some(InputDeviceType::ConsumerControls),
                "keyboard" => Some(InputDeviceType::Keyboard),
                "lightsensor" => Some(InputDeviceType::LightSensor),
                "mouse" => Some(InputDeviceType::Mouse),
                "touchscreen" => Some(InputDeviceType::Touch),
                _ => {
                    log::warn!("Ignoring unsupported device configuration: {}", device.as_ref());
                    None
                }
            })
            .collect()
    }
}

impl std::fmt::Display for InputDeviceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_device_list_from_structured_config_list() {
        let config = vec![
            "touchscreen".to_string(),
            "button".to_string(),
            "keyboard".to_string(),
            "mouse".to_string(),
            "hamster".to_string(),
            "lightsensor".to_string(),
        ];
        let expected = vec![
            InputDeviceType::Touch,
            InputDeviceType::ConsumerControls,
            InputDeviceType::Keyboard,
            InputDeviceType::Mouse,
            InputDeviceType::LightSensor,
        ];
        let actual = InputDeviceType::list_from_structured_config_list(&config);
        assert_eq!(actual, expected);
    }

    #[test]
    fn input_device_list_from_structured_config_list_strs() {
        let config = ["hamster", "button", "keyboard", "mouse"];
        let expected = vec![
            InputDeviceType::ConsumerControls,
            InputDeviceType::Keyboard,
            InputDeviceType::Mouse,
        ];
        let actual = InputDeviceType::list_from_structured_config_list(&config);
        assert_eq!(actual, expected);
    }
}
