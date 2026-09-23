// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Platform configuration options for the session.
#[derive(Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct PlatformSessionConfig {
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub enabled: bool,

    /// If `autolaunch` is true (the default) and the `session.url` is set in
    /// the `ProductSettings`, the named session will be launched when the device
    /// boots up.
    pub autolaunch: bool,

    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub include_element_manager: bool,

    // Whether to enable 5-button press to debug.
    pub enable_debug_shortcut: bool,
}

impl Default for PlatformSessionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            autolaunch: true,
            include_element_manager: false,
            enable_debug_shortcut: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_serialization() {
        crate::common::tests::default_serialization_helper::<PlatformSessionConfig>();
    }
}
