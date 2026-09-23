// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_hash::Hash;
use fuchsia_url::fuchsia_pkg::UnpinnedAbsolutePackageUrl;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Package Set Types as specified in RFC-212
/// https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs/0212_package_sets
#[derive(Clone, Copy, Hash, Serialize, Deserialize, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[serde(untagged)]
pub enum PackageSetType {
    /// Anchored packages
    Anchored(AnchoredPackageSetType),
    /// Upgradable packages
    Upgradable(UpgradablePackageSetType),
    /// Discoverable packages
    Discoverable(DiscoverablePackageSetType),
}

#[derive(Clone, Copy, Hash, Serialize, Deserialize, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AnchoredPackageSetType {
    /// Anchored, permanent packages, also known as static packages
    #[serde(rename = "anchored_permanent")]
    Permanent,
    /// Anchored automatic packages
    #[serde(rename = "anchored_automatic")]
    Automatic,
    /// Anchored on-demand packages
    #[serde(rename = "anchored_on_demand")]
    OnDemand,
}

#[derive(Clone, Copy, Hash, Serialize, Deserialize, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DiscoverablePackageSetType {
    #[serde(rename = "discoverable")]
    Discoverable,
}

#[derive(Clone, Copy, Hash, Serialize, Deserialize, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum UpgradablePackageSetType {
    /// Upgradable, permanent packages
    #[serde(rename = "upgradable_permanent")]
    Permanent,
    /// Upgradable, automatic packages
    #[serde(rename = "upgradable_automatic")]
    Automatic,
    /// Upgradable, on-demand packages
    #[serde(rename = "upgradable_on_demand")]
    OnDemand,
}

// The following types are used to enable (de)serialization of the different package set types
// into one JSON configuration file, as described by
// https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs/0271_anchored_packages?hl=en#the_dataanchored_packages_file
// Initially used for the implementation of anchored packages, the format is intended to be usable
// for all package set types.

/// PackageProperties contains additional properties besides
/// the package URL that apply to a package and can be provided. It is intended to be extensible.
#[derive(Clone, Hash, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct PackageProperties {
    pub hash: Hash,
}

/// PackageSet encapsulates one set of packages (e.g. the "automatic anchored set")
/// as a map of its package URL to the package's properties.
pub type PackageSet = BTreeMap<UnpinnedAbsolutePackageUrl, PackageProperties>;

/// PackageMap is the high level map of packages, containing maps to all the sets
/// referenced by its set type (e.g. the "on demand upgradable set").
pub type PackageMap = BTreeMap<PackageSetType, PackageSet>;

/// AnchoredPackageMap is a high level map of packages, similar to the PackageMap type, but
/// limited to the sets of anchored packages types.
pub type AnchoredPackageMap = BTreeMap<AnchoredPackageSetType, PackageSet>;
