// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::resolved_driver::ResolvedDriver;
use bind::compiler::Symbol;
use bind::interpreter::decode_bind_rules::DecodedCompositeBindRules;
use bind::interpreter::match_bind::{DeviceProperties, PropertyKey};
use fidl_fuchsia_driver_framework as fdf;
use std::sync::LazyLock;
use zx::Status;
use zx::sys::zx_status_t;

static BIND_PROTOCOL_KEY: LazyLock<PropertyKey> =
    LazyLock::new(|| PropertyKey::StringKey(String::from("fuchsia.BIND_PROTOCOL")));
static BIND_AUTOBIND_KEY: LazyLock<PropertyKey> =
    LazyLock::new(|| PropertyKey::StringKey(String::from("fuchsia.BIND_AUTOBIND")));

pub fn node_to_device_property(
    node_properties: &Vec<fdf::NodeProperty2>,
) -> Result<DeviceProperties, zx_status_t> {
    let mut device_properties = DeviceProperties::new();

    for property in node_properties {
        let key = PropertyKey::StringKey(property.key.clone());

        let value = match &property.value {
            fdf::NodePropertyValue::IntValue(i) => Symbol::NumberValue(i.clone().into()),
            fdf::NodePropertyValue::StringValue(s) => Symbol::StringValue(s.clone()),
            fdf::NodePropertyValue::EnumValue(s) => Symbol::EnumValue(s.clone()),
            fdf::NodePropertyValue::BoolValue(b) => Symbol::BoolValue(b.clone()),
            _ => {
                return Err(Status::INVALID_ARGS.into_raw());
            }
        };

        // TODO(https://fxbug.dev/42175777): Platform bus devices may contain two different BIND_PROTOCOL values.
        // The duplicate key needs to be fixed since this is incorrect and is working by luck.
        if key != *BIND_PROTOCOL_KEY {
            if device_properties.contains_key(&key) && device_properties.get(&key) != Some(&value) {
                log::error!(
                    "Node property key {:?} contains multiple values: {:?} and {:?}",
                    key,
                    device_properties.get(&key),
                    value
                );
                return Err(Status::INVALID_ARGS.into_raw());
            }
        }

        device_properties.insert(key, value);
    }

    Ok(device_properties)
}

pub fn node_to_device_property_no_autobind(
    node_properties: &Vec<fdf::NodeProperty2>,
) -> Result<DeviceProperties, zx_status_t> {
    let mut properties = node_to_device_property(node_properties)?;
    if properties.contains_key(&BIND_AUTOBIND_KEY) {
        properties.remove(&BIND_AUTOBIND_KEY);
    }
    properties.insert(BIND_AUTOBIND_KEY.clone(), Symbol::NumberValue(0));
    Ok(properties)
}

pub fn get_composite_rules_from_composite_driver<'a>(
    composite_driver: &'a ResolvedDriver,
) -> Result<&'a DecodedCompositeBindRules, i32> {
    match &composite_driver.bind_rules {
        bind::interpreter::decode_bind_rules::DecodedRules::Normal(_) => {
            log::error!("Cannot extract composite bind rules from a non-composite driver.");
            Err(Status::INTERNAL.into_raw())
        }
        bind::interpreter::decode_bind_rules::DecodedRules::Composite(rules) => Ok(rules),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    async fn test_duplicate_properties() {
        let node_properties = vec![
            fdf::NodeProperty2 {
                key: "foo".to_string(),
                value: fdf::NodePropertyValue::IntValue(200),
            },
            fdf::NodeProperty2 {
                key: "foo".to_string(),
                value: fdf::NodePropertyValue::IntValue(200),
            },
        ];

        let mut expected_properties = DeviceProperties::new();
        expected_properties
            .insert(PropertyKey::StringKey("foo".to_string()), Symbol::NumberValue(200));

        let result = node_to_device_property(&node_properties).unwrap();
        assert_eq!(expected_properties, result);
    }

    #[fuchsia::test]
    async fn test_property_collision() {
        let node_properties = vec![
            fdf::NodeProperty2 {
                key: "foo".to_string(),
                value: fdf::NodePropertyValue::IntValue(200),
            },
            fdf::NodeProperty2 {
                key: "foo".to_string(),
                value: fdf::NodePropertyValue::IntValue(10),
            },
        ];

        assert_eq!(Err(Status::INVALID_ARGS.into_raw()), node_to_device_property(&node_properties));
    }

    // TODO(https://fxbug.dev/42175777): Remove this case once the issue with multiple BIND_PROTOCOL properties
    // is resolved.
    #[fuchsia::test]
    async fn test_multiple_bind_protocol() {
        let node_properties = vec![
            fdf::NodeProperty2 {
                key: "fuchsia.BIND_PROTOCOL".to_string(),
                value: fdf::NodePropertyValue::IntValue(200),
            },
            fdf::NodeProperty2 {
                key: "fuchsia.BIND_PROTOCOL".to_string(),
                value: fdf::NodePropertyValue::IntValue(10),
            },
        ];

        let mut expected_properties = DeviceProperties::new();
        expected_properties.insert(BIND_PROTOCOL_KEY.clone(), Symbol::NumberValue(10));
        assert_eq!(Ok(expected_properties), node_to_device_property(&node_properties));
    }

    // TODO(https://fxbug.dev/42175777): Remove this case once the issue with multiple BIND_PROTOCOL properties
    // is resolved.
    #[fuchsia::test]
    async fn test_multiple_bind_protocol_w_deprecated_str_key() {
        let node_properties = vec![
            fdf::NodeProperty2 {
                key: "fuchsia.BIND_PROTOCOL".to_string(),
                value: fdf::NodePropertyValue::IntValue(28),
            },
            fdf::NodeProperty2 {
                key: "fuchsia.BIND_PROTOCOL".to_string(),
                value: fdf::NodePropertyValue::IntValue(10),
            },
        ];

        let mut expected_properties = DeviceProperties::new();
        expected_properties.insert(
            PropertyKey::StringKey("fuchsia.BIND_PROTOCOL".to_string()),
            Symbol::NumberValue(10),
        );
        assert_eq!(Ok(expected_properties), node_to_device_property(&node_properties));
    }

    #[fuchsia::test]
    async fn test_no_autobind() {
        let node_properties = vec![fdf::NodeProperty2 {
            key: "fuchsia.BIND_PROTOCOL".to_string(),
            value: fdf::NodePropertyValue::IntValue(200),
        }];

        let mut expected_properties = DeviceProperties::new();
        expected_properties.insert(BIND_PROTOCOL_KEY.clone(), Symbol::NumberValue(200));
        expected_properties.insert(BIND_AUTOBIND_KEY.clone(), Symbol::NumberValue(0));
        assert_eq!(Ok(expected_properties), node_to_device_property_no_autobind(&node_properties));
    }

    #[fuchsia::test]
    async fn test_no_autobind_override() {
        let node_properties = vec![fdf::NodeProperty2 {
            key: "fuchsia.BIND_AUTOBIND".to_string(),
            value: fdf::NodePropertyValue::IntValue(1),
        }];

        let mut expected_properties = DeviceProperties::new();
        expected_properties.insert(BIND_AUTOBIND_KEY.clone(), Symbol::NumberValue(0));
        assert_eq!(Ok(expected_properties), node_to_device_property_no_autobind(&node_properties));
    }
}
