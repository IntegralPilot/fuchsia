// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use assembly_container::WalkPaths;
use camino::Utf8PathBuf;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Platform configuration options for the starnix area.
#[derive(Debug, Deserialize, Serialize, PartialEq, JsonSchema, WalkPaths)]
#[serde(default, deny_unknown_fields)]
pub struct PowerConfig {
    /// Whether power suspend/resume is supported.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub suspend_enabled: bool,

    /// Whether storage power management is supported.
    /// This will only work on |suspend_enabled| is also true.
    /// TODO(https://fxbug.dev/383772372): Remove when no longer needed.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub storage_power_management_enabled: bool,

    /// Whether to include the power framework components that are needed
    /// for power system non-hermetic testing in the platform.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub enable_non_hermetic_testing: bool,

    /// Configuration of devices and drivers for power-metrics collection
    #[schemars(schema_with = "crate::option_path_schema")]
    #[walk_paths]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metrics_logging_config: Option<Utf8PathBuf>,
}

impl Default for PowerConfig {
    fn default() -> Self {
        Self {
            suspend_enabled: false,
            storage_power_management_enabled: true,
            enable_non_hermetic_testing: false,
            metrics_logging_config: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_serialization() {
        crate::common::tests::default_serialization_helper::<PowerConfig>();
    }
}
