// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Release information for a single assembly input.
#[derive(Clone, Default, Debug, Deserialize, Eq, Hash, PartialEq, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseInfo {
    /// Name of this input artifact.
    pub name: String,

    /// Origin repository of this input artifact.
    pub repository: String,

    /// Version of this input artifact.
    pub version: String,
}

impl ReleaseInfo {
    /// Helper function for constructing a ReleaseInfo in tests.
    pub fn new_for_testing() -> Self {
        Self { name: "".into(), repository: "".into(), version: "".into() }
    }
}

/// Release information for boards and their associated BIB sets.
#[derive(Clone, Default, Debug, Deserialize, Eq, Hash, PartialEq, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BoardReleaseInfo {
    /// Board release information.
    pub info: ReleaseInfo,

    /// Release information for the associated BIB sets.
    pub bib_sets: Vec<ReleaseInfo>,
}

impl BoardReleaseInfo {
    /// Helper function for constructing a BoardReleaseInfo in tests.
    pub fn new_for_testing() -> Self {
        Self { info: ReleaseInfo::new_for_testing(), bib_sets: vec![] }
    }
}

/// Release information for products and their associated product input bundles.
#[derive(Clone, Default, Debug, Deserialize, Eq, Hash, PartialEq, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProductReleaseInfo {
    /// Product release information.
    pub info: ReleaseInfo,

    /// Release information for the associated product input bundles.
    pub pibs: Vec<ReleaseInfo>,
}

impl ProductReleaseInfo {
    /// Helper function for constructing a ProductReleaseInfo in tests.
    pub fn new_for_testing() -> Self {
        Self { info: ReleaseInfo::new_for_testing(), pibs: vec![] }
    }
}

/// Release information for an assembly image.
#[derive(Clone, Default, Debug, Deserialize, Eq, Hash, PartialEq, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SystemReleaseInfo {
    /// Platform release information.
    pub platform: ReleaseInfo,

    /// Product release information.
    pub product: ProductReleaseInfo,

    /// Board release information.
    pub board: BoardReleaseInfo,
}

impl SystemReleaseInfo {
    /// Helper function for constructing a SystemReleaseInfo in tests.
    pub fn new_for_testing() -> Self {
        Self {
            platform: ReleaseInfo::new_for_testing(),
            product: ProductReleaseInfo::new_for_testing(),
            board: BoardReleaseInfo::new_for_testing(),
        }
    }
}

/// Release information for all assembly artifacts that contributed to a product bundle.
#[derive(Clone, Default, Debug, Deserialize, Eq, Hash, PartialEq, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProductBundleReleaseInfo {
    /// Product Bundle name.
    pub name: String,

    /// Product Bundle version.
    pub version: String,

    /// SDK version.
    pub sdk_version: String,

    /// Release information for slot A.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_a: Option<SystemReleaseInfo>,

    /// Release information for slot B.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_b: Option<SystemReleaseInfo>,

    /// Release information for slot R.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_r: Option<SystemReleaseInfo>,
}
