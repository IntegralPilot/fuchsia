// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Standard library memory allocator configuration options.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct MemoryAllocatorConfig {
    /// Runtime configuration for Scudo heap allocation. [See Scudo flags
    /// documentation](https://cs.opensource.google/fuchsia/fuchsia/+/main:third_party/scudo/src/flags.inc)
    /// for details. It is shadowed by `SCUDO_OPTIONS` environ variable from the component
    /// manifest.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub scudo_options: BTreeMap<String, String>,
}
