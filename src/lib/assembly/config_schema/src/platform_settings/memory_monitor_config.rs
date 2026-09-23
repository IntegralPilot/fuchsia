// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Available versions of Memory Monitor
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryMonitorVersion {
    /// Use the default configuration for the given feature support level and build type.
    #[default]
    Default,
    /// Memory Monitor 1, as implemented in //src/developer/memory/monitor/,
    /// which uses process-based memory attribution.
    V1,
    /// Memory Monitor 1, with memory profiling enabled.
    V1WithProfiling,
    /// Memory Monitor 2, as implemented in //src/performance/memory/attribution/monitor,
    /// which uses component-based memory attribution.
    V2,
}

/// Platform configuration options for the memory monitor area.
#[derive(Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct PlatformMemoryMonitorConfig {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub memory_monitor_versions: Vec<MemoryMonitorVersion>,
}

impl Default for PlatformMemoryMonitorConfig {
    fn default() -> Self {
        Self { memory_monitor_versions: vec![MemoryMonitorVersion::Default] }
    }
}
