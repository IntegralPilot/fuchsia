// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result, anyhow, format_err};
use fuchsia_hash::Hash;
use fuchsia_url::{PackageName, PackageVariant};
use scrutiny_utils::bootfs::*;
use scrutiny_utils::key_value::parse_key_value;
use scrutiny_utils::url::from_package_name_variant_path;
use scrutiny_utils::zbi::*;
use serde_json::json;
use serde_json::value::Value;
use std::collections::HashMap;
use std::fs::File;
use std::io::prelude::*;
use std::path::PathBuf;
use std::str::FromStr;

static BOOT_PACKAGE_INDEX: &str = "data/bootfs_packages";

pub struct ZbiListBootfsController {}

impl ZbiListBootfsController {
    pub fn extract(input: PathBuf) -> Result<Value> {
        let mut zbi_file = File::open(input)?;
        let mut zbi_buffer = Vec::new();
        zbi_file.read_to_end(&mut zbi_buffer)?;
        let mut reader = ZbiReader::new(zbi_buffer);
        let zbi_sections = reader.parse()?;

        for section in zbi_sections.iter() {
            if section.section_type == zbi::Type::StorageBootfs {
                let mut bootfs_reader = BootfsReader::new(section.buffer.clone());
                let bootfs_data = bootfs_reader.parse().context("failed to parse bootfs")?;
                let bootfs_files: Vec<String> =
                    bootfs_data.iter().map(|(k, _)| k.clone()).collect();
                return Ok(json!(bootfs_files));
            }
        }
        Err(anyhow!("Failed to find a bootfs section in the provided ZBI"))
    }
}

pub struct ZbiExtractBootfsPackageIndex {}

impl ZbiExtractBootfsPackageIndex {
    pub fn extract(input: PathBuf) -> Result<Value> {
        let mut zbi_file = File::open(input)?;
        let mut zbi_buffer = Vec::new();
        zbi_file.read_to_end(&mut zbi_buffer)?;
        let mut reader = ZbiReader::new(zbi_buffer);
        let zbi_sections = reader.parse()?;

        for section in zbi_sections.iter() {
            if section.section_type == zbi::Type::StorageBootfs {
                let mut bootfs_reader = BootfsReader::new(section.buffer.clone());
                let bootfs_files = bootfs_reader.parse()?;
                for (file_name, data) in bootfs_files.iter() {
                    if file_name == BOOT_PACKAGE_INDEX {
                        let bootfs_pkg_contents = std::str::from_utf8(&data)?;

                        let bootfs_pkgs = parse_key_value(bootfs_pkg_contents)?;

                        let bootfs_pkgs = bootfs_pkgs
                            .into_iter()
                            .map(|(name_and_variant, merkle)| {
                                let url = from_package_name_variant_path(name_and_variant)?;
                                let merkle = Hash::from_str(&merkle)?;
                                Ok(((url.name().clone(), url.variant().map(|v| v.clone())), merkle))
                            })
                            // Handle errors via collect
                            // Iter<Result<_, __>> into Result<Vec<_>, __>.
                            .collect::<Result<Vec<((PackageName, Option<PackageVariant>), Hash)>>>()
                            .map_err(|err| {
                                format_err!(
                                    "Failed to parse bootfs package index name/variant=merkle: {:?}",
                                    err)
                            })?
                            // Collect Vec<(_, __)> into HashMap<_, __>.
                            .into_iter()
                            .collect::<HashMap<(PackageName, Option<PackageVariant>), Hash>>();

                        return Ok(json!(BootfsPackageIndex { bootfs_pkgs: Some(bootfs_pkgs) }));
                    }
                }

                // TODO(https://fxbug.dev/42179754) After the first bootfs package is migrated to a component, an
                // absence of a bootfs package index is an error.
                return Ok(json!(BootfsPackageIndex { bootfs_pkgs: None }));
            }
        }
        Err(anyhow!("Failed to find a bootfs section in the provided ZBI"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maplit::hashmap;

    #[fuchsia::test]
    fn test_bootfs_package_serde() {
        let bootfs_package_index = BootfsPackageIndex {
            bootfs_pkgs: Some(hashmap! {
                (PackageName::from_str("alpha").unwrap(), Some(PackageVariant::zero())) =>
                    Hash::from_str("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef").unwrap(),
                (PackageName::from_str("beta").unwrap(), Some(PackageVariant::zero())) =>
                Hash::from_str("fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210").unwrap(),
            }),
        };
        let expected_value: serde_json::Value = serde_json::from_str(
            "
        {
            \"bootfs_pkgs\": {
                \"beta/0\": \"fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210\",
                \"alpha/0\": \"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\"
            }
        }",
        )
        .unwrap();

        let result = serde_json::to_value(&bootfs_package_index).unwrap();
        assert_eq!(expected_value, result);

        let deserialized_bootfs_packages: BootfsPackageIndex = serde_json::from_str(
            "
        {
            \"bootfs_pkgs\": {
                \"beta/0\": \"fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210\",
                \"alpha/0\": \"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\"
            }
        }",
        )
        .unwrap();

        assert_eq!(bootfs_package_index, deserialized_bootfs_packages);
    }
}
