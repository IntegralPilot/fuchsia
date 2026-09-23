// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::v1::FlashManifest as FlashManifestV1;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FlashManifest {
    pub hw_revision: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub product_matches: Vec<String>,
    #[serde(default)]
    pub credentials: Vec<String>,
    #[serde(rename = "products")]
    pub v1: FlashManifestV1,
}
