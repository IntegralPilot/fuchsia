// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Platform configuration options for driver load testing.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct TestFuzzingConfig {
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub enable_load_fuzzer: bool,

    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub max_load_delay_ms: u64,

    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub enable_test_shutdown_delays: bool,
}

/// Platform configuration options for driver framework support.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct DriverFrameworkConfig {
    /// The list of driver components to load eagerly. Eager drivers are those that are forced to
    /// be non-fallback drivers, even if their manifest indicates they should be fallback.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub eager_drivers: Vec<String>,

    /// The list of drivers to disable. These drivers are skipped when encountered during the
    /// driver loading process.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub disabled_drivers: Vec<String>,

    /// The list of devicetree nodes to enable even if their status is disabled.
    ///
    /// List of absolute devicetree node paths (always starting with `"/"` and
    /// separated by `"/"`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub enabled_devicetree_nodes: Vec<String>,

    /// Fuzzing configuration used for testing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test_fuzzing_config: Option<TestFuzzingConfig>,

    /// Enables routing storage capabilities to the base-drivers collection.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub enable_base_drivers_storage: bool,
}
