// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! UniqueReleaseInfo defines a struct for holding release information for assembly
//! input artifacts, and is used to construct the output of `ffx product get-version`.
//!
//! This file represents a contract between ffx, MOS, and fuchsia infrastructure.
//! Changes to this file must be implemented using soft transitions.

use assembly_partitions_config::Slot;
use assembly_release_info::{BoardReleaseInfo, ProductReleaseInfo, ReleaseInfo};
use assembly_util::sanitize_for_mos_apis;
use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use std::hash::{Hash, Hasher};

/// Struct holding release information (name, repository, version, etc..)
/// for a given assembly input artifact.
#[derive(Default, Clone, Debug, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UniqueReleaseInfo {
    /// The name of this assembly artifact.
    pub name: String,

    /// The name of this assembly artifact, sanitized. Meaning:
    ///   > All uppercase letters are converted to lowercase.
    ///   > All instances of "/" are converted to "_".
    ///
    /// All other constraints on what a valid name string can be are enforced
    /// during construction. See //build/assembly/tools/assembly_config
    /// for more information.
    name_sanitized: String,

    /// Release version for this assembly artifact.
    pub version: String,

    /// Release version for this assembly artifact, sanitized. Meaning:
    ///   > All uppercase letters are converted to lowercase.
    ///   > All instances of "/" are converted to "_".
    ///
    /// All other constraints on what a valid version string can be are enforced
    /// during construction. See //build/assembly/tools/assembly_config
    /// for more information.
    version_sanitized: String,

    /// Origin where this release artifact was created.
    pub repository: String,

    /// Origin where this release artifact was created, sanitized. Meaning:
    ///   > All uppercase letters are converted to lowercase.
    ///   > All instances of "/" are converted to "_".
    ///
    /// All other constraints on what a valid repository string can be are
    /// enforced during construction. See //build/assembly/tools/assembly_config
    /// for more information.
    repository_sanitized: String,

    /// System image location.
    pub slot: Vec<Slot>,
}

impl UniqueReleaseInfo {
    pub fn new(
        name: String,
        version: String,
        repository: String,
        slot: Vec<Slot>,
        artifact_type: String,
    ) -> Self {
        let name_to_sanitize = if artifact_type == "product_bundle" {
            name.clone()
        } else {
            format!("{}_{}", artifact_type, name)
        };
        let name_sanitized = sanitize_for_mos_apis(&name_to_sanitize);
        let version_sanitized = sanitize_for_mos_apis(&version);
        let repository_sanitized = sanitize_for_mos_apis(&repository);

        UniqueReleaseInfo {
            name,
            version,
            repository,
            slot,
            name_sanitized,
            version_sanitized,
            repository_sanitized,
        }
    }
}

impl PartialOrd for UniqueReleaseInfo {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for UniqueReleaseInfo {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.name
            .cmp(&other.name)
            .then_with(|| self.version.cmp(&other.version))
            .then_with(|| self.repository.cmp(&other.repository))
    }
}

impl PartialEq for UniqueReleaseInfo {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && self.version == other.version
            && self.repository == other.repository
    }
}

impl Hash for UniqueReleaseInfo {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name.hash(state);
        self.version.hash(state);
        self.repository.hash(state);
    }
}

fn from_release_info(
    name: String,
    version: String,
    repository: String,
    slot: &Option<Slot>,
    artifact_type: &str,
) -> UniqueReleaseInfo {
    UniqueReleaseInfo::new(
        name,
        version,
        repository,
        if let Some(s) = slot { vec![s.clone()] } else { vec![] },
        artifact_type.to_string(),
    )
}

/// Convert a Platform ReleaseInfo instance into a UniqueReleaseInfo instance,
/// to better suit the format needed by customers of `ffx product get-version`.
pub fn from_platform_release_info(info: &ReleaseInfo, slot: &Option<Slot>) -> UniqueReleaseInfo {
    from_release_info(
        info.name.clone(),
        info.version.clone(),
        info.repository.clone(),
        slot,
        "platform",
    )
}

/// Convert a ProductReleaseInfo instance into a UniqueReleaseInfo instance,
/// to better suit the format needed by customers of `ffx product get-version`.
pub fn from_product_release_info(
    info: &ProductReleaseInfo,
    slot: &Option<Slot>,
) -> UniqueReleaseInfo {
    from_release_info(
        info.info.name.clone(),
        info.info.version.clone(),
        info.info.repository.clone(),
        slot,
        "products",
    )
}

/// Convert a PIB ReleaseInfo instance into a UniqueReleaseInfo instance,
/// to better suit the format needed by customers of `ffx product get-version`.
pub fn from_pib_release_info(info: &ReleaseInfo, slot: &Option<Slot>) -> UniqueReleaseInfo {
    from_release_info(
        info.name.clone(),
        info.version.clone(),
        info.repository.clone(),
        slot,
        "product_input_bundles",
    )
}

/// Convert a BoardReleaseInfo instance into a UniqueReleaseInfo instance,
/// to better suit the format needed by customers of `ffx product get-version`.
pub fn from_board_release_info(info: &BoardReleaseInfo, slot: &Option<Slot>) -> UniqueReleaseInfo {
    from_release_info(
        info.info.name.clone(),
        info.info.version.clone(),
        info.info.repository.clone(),
        slot,
        "boards",
    )
}

/// Convert a BIB Set ReleaseInfo instance into a UniqueReleaseInfo instance,
/// to better suit the format needed by customers of `ffx product get-version`.
pub fn from_bib_set_release_info(info: &ReleaseInfo, slot: &Option<Slot>) -> UniqueReleaseInfo {
    from_release_info(
        info.name.clone(),
        info.version.clone(),
        info.repository.clone(),
        slot,
        "board_input_bundles",
    )
}

/// Struct holding a vector of UniqueReleaseInfo elements.
///
/// This is used to modify the serialization / deserialization of a vector
/// of elements.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UniqueReleaseInfoVector(pub Vec<UniqueReleaseInfo>);

// Custom wrapper for conditional serialization/deserialization

impl Serialize for UniqueReleaseInfoVector {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if self.0.len() == 1 {
            // If there's exactly one item, serialize the item directly
            self.0[0].serialize(serializer)
        } else {
            // Otherwise, serialize the entire vector
            self.0.serialize(serializer)
        }
    }
}

impl<'de> Deserialize<'de> for UniqueReleaseInfoVector {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // First, deserialize into a serde_json::Value. This consumes the
        // deserializer once, but we can then try to deserialize from the Value
        // multiple times without consuming it.
        let value = Value::deserialize(deserializer)?;

        // Try to deserialize as a single UniqueReleaseInfo object
        if let Ok(item) = UniqueReleaseInfo::deserialize(&value) {
            return Ok(UniqueReleaseInfoVector(vec![item]));
        }

        // If that fails, try to deserialize as a Vec<UniqueReleaseInfo>
        if let Ok(vec) = Vec::<UniqueReleaseInfo>::deserialize(&value) {
            return Ok(UniqueReleaseInfoVector(vec));
        }

        // If neither conversion works, return an error
        Err(D::Error::custom(
            "Expected either a single ReleaseInfo object or an array of ReleaseInfo objects",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitization() {
        // Test with valid characters that don't need sanitization.
        let info = UniqueReleaseInfo::new(
            "product-name".to_string(),
            "1.2.3".to_string(),
            "fuchsia".to_string(),
            vec![],
            "products".to_string(),
        );
        assert_eq!(info.name_sanitized, "products_product-name");
        assert_eq!(info.version_sanitized, "1.2.3");
        assert_eq!(info.repository_sanitized, "fuchsia");

        // Test with invalid characters that should be replaced with underscores.
        let info = UniqueReleaseInfo::new(
            "product name with spaces".to_string(),
            "1!2@3".to_string(),
            "invalid/repo/path".to_string(),
            vec![],
            "products".to_string(),
        );
        assert_eq!(info.name_sanitized, "products_product_name_with_spaces");
        assert_eq!(info.version_sanitized, "1_2_3");
        assert_eq!(info.repository_sanitized, "invalid_repo_path");

        // Test with mixed-case characters.
        let info = UniqueReleaseInfo::new(
            "ProductName".to_string(),
            "VERSION".to_string(),
            "REPOSITORY".to_string(),
            vec![],
            "products".to_string(),
        );
        assert_eq!(info.name_sanitized, "products_productname");
        assert_eq!(info.version_sanitized, "version");
        assert_eq!(info.repository_sanitized, "repository");
    }

    #[test]
    fn test_sanitization_product_bundle() {
        // Test with product_bundle artifact type.
        let info = UniqueReleaseInfo::new(
            "product-name".to_string(),
            "1.2.3".to_string(),
            "unspecified".to_string(),
            vec![],
            "product_bundle".to_string(),
        );
        assert_eq!(info.name_sanitized, "product-name");
        assert_eq!(info.version_sanitized, "1.2.3");
        assert_eq!(info.repository_sanitized, "unspecified");
    }
}
