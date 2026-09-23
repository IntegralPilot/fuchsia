// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Assembly platform configuratio schema for the Fonts subsystem.
#[derive(Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct FontsConfig {
    /// If true, use assembly to configure fonts.
    ///
    /// Otherwise, assembly configuration is skipped, and we assume that
    /// the configuration is done some other way.
    /// Prod configurations will want to set this to false.
    pub enabled: bool,

    /// If defined, represents the name of the font collection domain
    /// config package to be included in the product.
    /// It is only valid if `enabled==true`.
    ///
    /// When unset, software assembly uses the (deprecated) fonts configuration
    /// that reads the fonts from `config-data`.  When set,
    /// software assembly uses the product configuration instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font_collection: Option<String>,
}

impl Default for FontsConfig {
    fn default() -> Self {
        FontsConfig {
            // Most configs want to enable fonts configuration, so the default here
            // is `true`.
            enabled: true,
            font_collection: None,
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_default_serialization() {
        crate::common::tests::default_serialization_helper::<FontsConfig>();
    }

    #[test]
    fn default_fonts_config() {
        let json = serde_json::json!({});
        let config: FontsConfig = serde_json::from_value(json).unwrap();
        // Checks that the default for the "fonts enabled" flag is true (i.e.
        // different from the "regular" zero type.)
        assert!(config.enabled);
    }
}
