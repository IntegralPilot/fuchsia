// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Platform configuration options for the graphics are.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct GraphicsConfig {
    /// Configuration for the virtual console
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub virtual_console: VirtconConfig,

    /// Which Vulkan ICDs to allow.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub vulkan_icd: VulkanIcd,

    /// Configuration for fake display.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub fake_display: FakeDisplayConfig,
}

#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema, Clone, Copy)]
#[serde(default, deny_unknown_fields)]
pub struct VulkanIcd {
    pub allow_magma: Option<bool>,
    pub allow_goldfish: Option<bool>,
    pub allow_lavapipe: Option<bool>,
}

/// Allows using a default "off" setting, enabling the fake display AIB
/// with default values, or enabling the AIB with customized dimensions.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema, Clone, Copy)]
#[serde(rename_all = "snake_case", untagged)]
pub enum FakeDisplayConfig {
    #[default]
    Off,
    OnWithDefaultValues,
    Values(FakeDisplayValues),
}

/// Requires supplying both width and height when customizing.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema, Clone, Copy)]
#[serde(deny_unknown_fields)]
pub struct FakeDisplayValues {
    /// Width of the fake display in pixels.
    pub width: u32,

    /// Height of the fake display in pixels.
    pub height: u32,
}

/// Platform configuration options for the virtual console
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct VirtconConfig {
    /// Whether the virtual console should be included.  This has a different
    /// default value depending on the BuildType.  It's 'true' for Eng and
    /// UserDebug, false for User.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable: Option<bool>,

    /// The color scheme for the virtual console to use
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_scheme: Option<VirtconColorScheme>,

    /// The cdpi of the virtual console
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dpi: Vec<u32>,

    /// Specify the keymap for the virtual console. "qwerty" and "dvorak" are
    /// supported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keymap: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VirtconColorScheme {
    #[default]
    Dark,
    Light,
    Special,
}

impl std::fmt::Display for VirtconColorScheme {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            VirtconColorScheme::Dark => "dark",
            VirtconColorScheme::Light => "light",
            VirtconColorScheme::Special => "special",
        };
        f.write_str(name)
    }
}
