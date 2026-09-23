// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use assembly_container::WalkPaths;
use camino::Utf8PathBuf;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Platform configuration options for system sounds
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema, WalkPaths)]
#[serde(default, deny_unknown_fields)]
pub struct SystemSoundsConfig {
    /// earcon configuration
    #[walk_paths]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub earcons: Option<Earcons>,
}

/// Earcons are "audible icons"
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema, WalkPaths)]
#[serde(default, deny_unknown_fields)]
pub struct Earcons {
    /// Sound to play on bluetooth connection
    #[schemars(schema_with = "crate::option_path_schema")]
    #[walk_paths]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bluetooth_connected: Option<Utf8PathBuf>,

    /// Sound to play on bluetooth disconnect
    #[schemars(schema_with = "crate::option_path_schema")]
    #[walk_paths]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bluetooth_disconnected: Option<Utf8PathBuf>,

    /// Sound to play when changing volume
    #[schemars(schema_with = "crate::option_path_schema")]
    #[walk_paths]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume_changed: Option<Utf8PathBuf>,

    /// Sound to play when reaching max volume
    #[schemars(schema_with = "crate::option_path_schema")]
    #[walk_paths]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume_max_reached: Option<Utf8PathBuf>,

    /// Sound to play on system start
    #[schemars(schema_with = "crate::option_path_schema")]
    #[walk_paths]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_start: Option<Utf8PathBuf>,
}
