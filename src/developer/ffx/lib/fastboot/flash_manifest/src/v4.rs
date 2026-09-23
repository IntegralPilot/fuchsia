// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::v2::FlashManifest as FlashManifestV2;
use crate::v3::FlashManifest as FlashManifestV3;
use assembly_partitions_config::UploadMethod;
use serde::{Deserialize, Serialize};

/// V4 is similar to V3, but adds support for specifying the SSH key upload method.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct FlashManifest {
    #[serde(flatten)]
    pub v3: FlashManifestV3,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_key_upload_method: Option<UploadMethod>,
}

impl From<&FlashManifest> for FlashManifestV2 {
    fn from(p: &FlashManifest) -> FlashManifestV2 {
        (&p.v3).into()
    }
}
