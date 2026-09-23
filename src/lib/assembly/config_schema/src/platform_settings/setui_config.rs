// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use assembly_container::WalkPaths;
use camino::Utf8PathBuf;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub enum ICUType {
    /// Use assembly to define the setui config.  Use the unflavored setui
    /// package, compiled without regard to a specific ICU version.
    #[serde(rename = "without_icu")]
    Unflavored,

    /// Use assembly to define the setui config. Use the ICU flavored setui
    /// package, compiled with the specific ICU commit ID.
    #[serde(rename = "with_icu")]
    #[default]
    Flavored,
}

/// Platform configuration options for the input area.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema, WalkPaths)]
#[serde(default, deny_unknown_fields)]
pub struct SetUiConfig {
    /// If set, the setui config is added to the product configuration.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub use_icu: ICUType,

    /// If set, uses the setui configured with camera settings.  Else uses
    /// setui without camera.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub with_camera: bool,

    #[schemars(schema_with = "crate::option_path_schema")]
    #[walk_paths]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display: Option<Utf8PathBuf>,

    #[schemars(schema_with = "crate::option_path_schema")]
    #[walk_paths]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interface: Option<Utf8PathBuf>,

    /// The setui agents to start
    #[schemars(schema_with = "crate::option_path_schema")]
    #[walk_paths]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<Utf8PathBuf>,

    /// If an external brightness controller is being used (as opposed to
    /// brightness being controlled by setui)
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub external_brightness_controller: bool,

    /// The input devices used with settings UI
    #[schemars(schema_with = "crate::option_path_schema")]
    #[walk_paths]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_device_config: Option<Utf8PathBuf>,

    /// The lights (LEDs) controlled by settings UI
    #[schemars(schema_with = "crate::option_path_schema")]
    #[walk_paths]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub light_hardware_config: Option<Utf8PathBuf>,
}
