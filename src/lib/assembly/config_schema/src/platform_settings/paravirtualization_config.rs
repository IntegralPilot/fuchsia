// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::FeatureControl;

/// Platform configuration options for paravirtualization.
#[derive(Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct PlatformParavirtualizationConfig {
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub enabled: FeatureControl,
}

impl Default for PlatformParavirtualizationConfig {
    fn default() -> Self {
        Self { enabled: FeatureControl::Allowed }
    }
}
