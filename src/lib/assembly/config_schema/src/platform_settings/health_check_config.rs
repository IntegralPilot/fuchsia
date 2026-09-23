// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Platform configuration options for OTA health checks.
///
/// `verify_components` are capable of blocking an update if they do not report a healthy status.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct HealthCheckConfig {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub verify_components: Vec<VerifyComponent>,
}

/// Platform components that have implemented OTA health check verification service.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub enum VerifyComponent {
    Storage,
    Netstack,
    Setui,
    Omaha,
}

impl VerifyComponent {
    /// Map the VerifyComponent to its moniker.
    pub fn source_moniker(&self) -> &str {
        match *self {
            VerifyComponent::Storage => "bootstrap/fshost",
            VerifyComponent::Netstack => "core/network/netstack",
            VerifyComponent::Setui => "core/setui_service",
            VerifyComponent::Omaha => "core/system-update/omaha-client-service",
        }
    }
}
