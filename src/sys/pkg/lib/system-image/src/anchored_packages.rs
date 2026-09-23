// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::errors::AnchoredPackagesError;
use fuchsia_pkg::package_sets::{AnchoredPackageMap, AnchoredPackageSetType, PackageProperties};
use fuchsia_url::fuchsia_pkg::PinnedAbsolutePackageUrl;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnchoredPackages {
    contents: AnchoredPackageMap,
}

impl AnchoredPackages {
    /// Create a new empty instance of `AnchoredPackages`.
    pub fn new() -> Self {
        Self { contents: AnchoredPackageMap::new() }
    }

    /// Clears all entries of a package set type (if provided) or the entire structure of an
    /// existing, possibly populated instance of `AnchoredPackages`.
    pub fn clear(&mut self, package_set_type: Option<AnchoredPackageSetType>) -> &mut Self {
        match package_set_type {
            None => self.contents = AnchoredPackageMap::new(),
            Some(t) => {
                let _ = self.contents.remove(&t);
            }
        }
        self
    }

    // Insert a package specified by a PinnedAbsolutePackageUrl and the associated PackageSetType.
    pub fn insert(
        &mut self,
        package_set_type: AnchoredPackageSetType,
        url: PinnedAbsolutePackageUrl,
    ) -> Result<(), AnchoredPackagesError> {
        let (u, h) = url.into_unpinned_and_hash();
        if let Some(map) = self.contents.get_mut(&package_set_type) {
            if map.contains_key(&u) {
                return Err(AnchoredPackagesError::DuplicateNotSupported(u));
            }
            map.insert(u, PackageProperties { hash: h });
        } else {
            self.contents
                .insert(package_set_type, BTreeMap::from([(u, PackageProperties { hash: h })]));
        }
        Ok(())
    }

    /// Create a new instance of `AnchoredPackages` from parsing a json.
    /// If there are no anchored packages, `file_contents` must be empty.
    pub(crate) fn from_json(file_contents: &[u8]) -> Result<Self, AnchoredPackagesError> {
        if file_contents.is_empty() {
            return Ok(Self { contents: AnchoredPackageMap::new() });
        }
        let contents = parse_json(file_contents)?;
        Ok(Self { contents })
    }

    /// Returns the mapping for a given package set type as pinned absolute package URLs
    pub fn as_pinned(
        &self,
        package_set_type: AnchoredPackageSetType,
    ) -> Vec<PinnedAbsolutePackageUrl> {
        match self.contents.get(&package_set_type) {
            Some(t) => t
                .iter()
                .map(|x| PinnedAbsolutePackageUrl::from_unpinned(x.0.clone(), x.1.hash))
                .collect(),
            None => vec![],
        }
    }

    /// Serializes the complete mapping of all anchored packages to a writer
    pub fn serialize(&self, writer: impl std::io::Write) -> Result<(), serde_json::Error> {
        if self.contents.is_empty() {
            return Ok(());
        }
        serde_json::to_writer(writer, &self.contents)
    }
}

impl Default for AnchoredPackages {
    fn default() -> Self {
        Self::new()
    }
}

fn parse_json(contents: &[u8]) -> Result<AnchoredPackageMap, AnchoredPackagesError> {
    serde_json::from_slice(contents).map_err(AnchoredPackagesError::JsonError)
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use fuchsia_hash::Hash;
    use fuchsia_url::fuchsia_pkg::{AbsolutePackageUrl, UnpinnedAbsolutePackageUrl};
    use std::str::FromStr;

    fn populated_anchored_packages() -> (AnchoredPackages, &'static str) {
        let json = r#" {
            "anchored_on_demand": {
                "fuchsia-pkg://fuchsia.com/package0": {
                    "hash": "0000000000000000000000000000000000000000000000000000000000000000"
                },
                "fuchsia-pkg://fuchsia.com/package1": {
                    "hash": "1111111111111111111111111111111111111111111111111111111111111111"
                }
            },
            "anchored_permanent": {
                "fuchsia-pkg://fuchsia.com/package2": {
                    "hash": "2222222222222222222222222222222222222222222222222222222222222222"
                },
                "fuchsia-pkg://fuchsia.com/package3": {
                    "hash": "3333333333333333333333333333333333333333333333333333333333333333"
                }
            }
         }"#;

        let mut anchored_packages_map = BTreeMap::new();
        anchored_packages_map.insert(AnchoredPackageSetType::OnDemand, BTreeMap::new());
        anchored_packages_map.insert(AnchoredPackageSetType::Permanent, BTreeMap::new());
        anchored_packages_map.get_mut(&AnchoredPackageSetType::OnDemand).unwrap().insert(
            UnpinnedAbsolutePackageUrl::from_str("fuchsia-pkg://fuchsia.com/package1").unwrap(),
            PackageProperties {
                hash: Hash::from_str(
                    "1111111111111111111111111111111111111111111111111111111111111111",
                )
                .unwrap(),
            },
        );
        anchored_packages_map.get_mut(&AnchoredPackageSetType::OnDemand).unwrap().insert(
            UnpinnedAbsolutePackageUrl::from_str("fuchsia-pkg://fuchsia.com/package0").unwrap(),
            PackageProperties {
                hash: Hash::from_str(
                    "0000000000000000000000000000000000000000000000000000000000000000",
                )
                .unwrap(),
            },
        );
        anchored_packages_map.get_mut(&AnchoredPackageSetType::Permanent).unwrap().insert(
            UnpinnedAbsolutePackageUrl::from_str("fuchsia-pkg://fuchsia.com/package3").unwrap(),
            PackageProperties {
                hash: Hash::from_str(
                    "3333333333333333333333333333333333333333333333333333333333333333",
                )
                .unwrap(),
            },
        );
        anchored_packages_map.get_mut(&AnchoredPackageSetType::Permanent).unwrap().insert(
            UnpinnedAbsolutePackageUrl::from_str("fuchsia-pkg://fuchsia.com/package2").unwrap(),
            PackageProperties {
                hash: Hash::from_str(
                    "2222222222222222222222222222222222222222222222222222222222222222",
                )
                .unwrap(),
            },
        );
        (AnchoredPackages { contents: anchored_packages_map }, json)
    }

    #[test]
    fn empty_anchored_package_list_in_json() {
        let empty: &[u8] = &[0; 0];
        let r = AnchoredPackages::from_json(empty);
        assert!(r.is_ok());
    }

    #[test]
    fn default_anchored_package_structure() {
        let empty = AnchoredPackages::new();
        let default = AnchoredPackages { ..Default::default() };
        assert_eq!(empty, default);
    }

    #[test]
    fn insert_into_anchored_packages_structure() {
        let mut packages = AnchoredPackages::new();
        assert!(packages.insert(AnchoredPackageSetType::Automatic,
            PinnedAbsolutePackageUrl::parse("fuchsia-pkg://fuchsia.com/package0?hash=0000000000000000000000000000000000000000000000000000000000000000").unwrap()).is_ok());
    }

    #[test]
    fn clear_non_empty_anchored_package_structure() {
        let mut packages = AnchoredPackages::new();
        assert!(packages.insert(AnchoredPackageSetType::Automatic,
            PinnedAbsolutePackageUrl::parse("fuchsia-pkg://fuchsia.com/package0?hash=0000000000000000000000000000000000000000000000000000000000000000").unwrap()).is_ok());
        assert!(packages.insert(AnchoredPackageSetType::OnDemand,
            PinnedAbsolutePackageUrl::parse("fuchsia-pkg://fuchsia.com/package1?hash=1111111111111111111111111111111111111111111111111111111111111111").unwrap()).is_ok());
        packages.clear(None);
        let empty = AnchoredPackages::new();
        assert_eq!(empty, packages);
    }

    #[test]
    fn partially_clear_non_empty_anchored_package_structure() {
        let mut packages = AnchoredPackages::new();
        assert!(packages.insert(AnchoredPackageSetType::Automatic,
            PinnedAbsolutePackageUrl::parse("fuchsia-pkg://fuchsia.com/package0?hash=0000000000000000000000000000000000000000000000000000000000000000").unwrap()).is_ok());
        assert!(packages.insert(AnchoredPackageSetType::OnDemand,
            PinnedAbsolutePackageUrl::parse("fuchsia-pkg://fuchsia.com/package1?hash=1111111111111111111111111111111111111111111111111111111111111111").unwrap()).is_ok());
        packages.clear(Some(AnchoredPackageSetType::Automatic));
        let mut packages_cmp = AnchoredPackages::new();
        assert_ne!(packages_cmp, packages);
        assert!(packages_cmp.insert(AnchoredPackageSetType::OnDemand,
            PinnedAbsolutePackageUrl::parse("fuchsia-pkg://fuchsia.com/package1?hash=1111111111111111111111111111111111111111111111111111111111111111").unwrap()).is_ok());
        assert_eq!(packages_cmp, packages);
    }

    #[test]
    fn insert_into_structure_and_json_deserialize_yield_identical_mappings() {
        let json = r#" {
            "anchored_automatic": {
                "fuchsia-pkg://fuchsia.com/package0": {
                    "hash": "0000000000000000000000000000000000000000000000000000000000000000"
                }
            },
            "anchored_on_demand": {
                "fuchsia-pkg://fuchsia.com/package1": {
                    "hash": "1111111111111111111111111111111111111111111111111111111111111111"
                }
            }
        }"#;
        let from_json = AnchoredPackages::from_json(json.as_bytes()).unwrap();
        let mut from_insert = AnchoredPackages::new();
        assert!(from_insert.insert(AnchoredPackageSetType::Automatic,
            PinnedAbsolutePackageUrl::parse("fuchsia-pkg://fuchsia.com/package0?hash=0000000000000000000000000000000000000000000000000000000000000000").unwrap()).is_ok());
        assert!(from_insert.insert(AnchoredPackageSetType::OnDemand,
            PinnedAbsolutePackageUrl::parse("fuchsia-pkg://fuchsia.com/package1?hash=1111111111111111111111111111111111111111111111111111111111111111").unwrap()).is_ok());
        assert_eq!(from_json, from_insert);
    }

    #[test]
    fn missing_hash_in_anchored_packages() {
        let json = r#" {
            "anchored_automatic": {
                "fuchsia-pkg://fuchsia.com/package0": {
                    "wrong_key": "0000000000000000000000000000000000000000000000000000000000000000"
                },
                "fuchsia-pkg://fuchsia.com/package1": {
                    "hash": "1111111111111111111111111111111111111111111111111111111111111111"
                }
            }
        }"#;

        assert_matches!(
            AnchoredPackages::from_json(json.as_bytes()),
            Err(AnchoredPackagesError::JsonError(_))
        );
    }

    #[test]
    fn pinned_package_url_in_anchored_package_list() {
        let json = r#" {
            "anchored_automatic": {
                "fuchsia-pkg://fuchsia.com/package0?hash=0000000000000000000000000000000000000000000000000000000000000000": {
                    "hash": "0000000000000000000000000000000000000000000000000000000000000000"
                }
            }
        }"#;

        assert_matches!(
            AnchoredPackages::from_json(json.as_bytes()),
            Err(AnchoredPackagesError::JsonError(_))
        );
    }

    #[test]
    fn multiple_package_sets_in_list() {
        let (expect, json) = populated_anchored_packages();
        let got = AnchoredPackages::from_json(json.as_bytes()).unwrap();
        assert_eq!(got, expect);
    }

    #[test]
    fn additional_unused_properties_in_anchored_packages_list() {
        let json = r#" {
            "anchored_automatic": {
                "fuchsia-pkg://fuchsia.com/package0": {
                    "hash": "0000000000000000000000000000000000000000000000000000000000000000",
                    "foo": "0"
                },
                "fuchsia-pkg://fuchsia.com/package1": {
                    "hash": "1111111111111111111111111111111111111111111111111111111111111111",
                    "bar": "1"
                }
            }
        }"#;
        let got = AnchoredPackages::from_json(json.as_bytes()).unwrap();
        let mut expect = AnchoredPackages::new();
        assert!(expect.insert(AnchoredPackageSetType::Automatic,
            PinnedAbsolutePackageUrl::parse("fuchsia-pkg://fuchsia.com/package1?hash=1111111111111111111111111111111111111111111111111111111111111111").unwrap()).is_ok());
        assert!(expect.insert(AnchoredPackageSetType::Automatic,
            PinnedAbsolutePackageUrl::parse("fuchsia-pkg://fuchsia.com/package0?hash=0000000000000000000000000000000000000000000000000000000000000000").unwrap()).is_ok());
        assert_eq!(got, expect);
    }

    #[test]
    fn anchored_packages_as_pinned_urls() {
        let (_, json) = populated_anchored_packages();
        let mut got_on_demand = AnchoredPackages::from_json(json.as_bytes())
            .unwrap()
            .as_pinned(AnchoredPackageSetType::OnDemand);
        got_on_demand.sort();
        let mut got_permanent = AnchoredPackages::from_json(json.as_bytes())
            .unwrap()
            .as_pinned(AnchoredPackageSetType::Permanent);
        got_permanent.sort();
        let expected_on_demand = vec![
            AbsolutePackageUrl::parse("fuchsia-pkg://fuchsia.com/package0?hash=0000000000000000000000000000000000000000000000000000000000000000").unwrap().pinned().unwrap(),
            AbsolutePackageUrl::parse("fuchsia-pkg://fuchsia.com/package1?hash=1111111111111111111111111111111111111111111111111111111111111111").unwrap().pinned().unwrap(),
        ];
        let expected_permanent = vec![
            AbsolutePackageUrl::parse("fuchsia-pkg://fuchsia.com/package2?hash=2222222222222222222222222222222222222222222222222222222222222222").unwrap().pinned().unwrap(),
            AbsolutePackageUrl::parse("fuchsia-pkg://fuchsia.com/package3?hash=3333333333333333333333333333333333333333333333333333333333333333").unwrap().pinned().unwrap(),
        ];
        assert_eq!(got_on_demand, expected_on_demand);
        assert_eq!(got_permanent, expected_permanent);
    }

    #[test]
    fn test_serialize_deserialize_round_trip() {
        let mut bytes = vec![];

        let (packages, _) = populated_anchored_packages();
        packages.serialize(&mut bytes).unwrap();

        assert_eq!(AnchoredPackages::from_json(&bytes).unwrap(), packages);
    }
}
