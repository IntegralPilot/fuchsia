// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::BTreeMap;

use assembly_container::WalkPaths;
use camino::Utf8PathBuf;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::FeatureSetLevel;
use crate::common::option_path_schema;

/// Platform configuration options for recovery.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema, WalkPaths)]
#[serde(default, deny_unknown_fields)]
pub struct RecoveryConfig {
    /// Include the factory-reset-trigger package, and configure it using the given file.
    ///
    /// This is a a map of channel names to indices, when the current OTA
    /// channel matches one of the names in the file, if a stored index is less
    /// than the index value in the file, a factory reset is triggered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub factory_reset_trigger_config: Option<BTreeMap<String, i32>>,

    /// Which system_recovery implementation to include
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_recovery: Option<SystemRecovery>,

    /// The path to the logo for the recovery process to use.
    ///
    /// This must be a rive file (.riv).
    #[schemars(schema_with = "option_path_schema")]
    #[walk_paths]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logo: Option<Utf8PathBuf>,

    /// The path to the instructions to display.
    ///
    /// This file must be raw text for displaying.
    #[schemars(schema_with = "option_path_schema")]
    #[walk_paths]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<Utf8PathBuf>,

    /// Perform a managed-mode check before doing an FDR.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub check_for_managed_mode: bool,
}

impl RecoveryConfig {
    /// Returns true if the configuration includes the `factory_reset` component.
    pub fn has_factory_reset(&self, feature_set_level: FeatureSetLevel) -> bool {
        // factory_reset is required by the standard feature set level, and when system_recovery
        // is enabled.
        feature_set_level == FeatureSetLevel::Standard || self.system_recovery.is_some()
    }
}

/// Which system recovery implementation to include in the image
#[derive(Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SystemRecovery {
    Fdr,
    Android,
    Bootfs(BootfsRecoveryConfig),
}

/// Details of bootfs recovery if it is selected
// Avoid using Default trait since product_component_url is required.
#[derive(Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct BootfsRecoveryConfig {
    /// Url of the product-provided bootfs recovery component
    pub product_component_url: String,

    /// Don't start on startup
    #[serde(default)] // Default to false.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub disable_eager_startup: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_recovery_config_defaults() {
        let original = RecoveryConfig::default();
        let serialized = serde_json::to_value(&original).expect("serialization failed");
        let deserialized: RecoveryConfig =
            serde_json::from_value(serialized).expect("deserialization failed");
        assert_eq!(original, deserialized);
    }

    #[test]
    fn test_recovery_config_with_bootfs() {
        let config = RecoveryConfig {
            system_recovery: Some(SystemRecovery::Bootfs(BootfsRecoveryConfig {
                product_component_url: "url".into(),
                disable_eager_startup: true,
            })),
            ..Default::default()
        };
        let serialized = serde_json::to_value(&config).expect("serialization failed");
        let deserialized: RecoveryConfig =
            serde_json::from_value(serialized).expect("deserialization failed");
        assert_eq!(config, deserialized);
    }

    #[test]
    fn test_bootfs_recovery_config_defaults() {
        // BootfsRecoveryConfig doesn't implement Default, but we can test that
        // the fields with defaults are handled correctly when missing from JSON.
        let product_component_url = "fuchsia-pkg://fuchsia.com/recovery#meta/recovery.cm";
        let json = json!({
            "product_component_url": product_component_url
        });
        let config: BootfsRecoveryConfig =
            serde_json::from_value(json).expect("deserialization failed");

        assert_eq!(config.product_component_url, product_component_url);
        assert!(!config.disable_eager_startup); // Default is false.
    }

    #[test]
    fn test_recovery_config_check_for_managed_mode() {
        let config = RecoveryConfig { check_for_managed_mode: true, ..Default::default() };
        let serialized = serde_json::to_value(&config).expect("serialization failed");
        let deserialized: RecoveryConfig =
            serde_json::from_value(serialized).expect("deserialization failed");
        assert_eq!(config, deserialized);
    }
}
