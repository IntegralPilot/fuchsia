// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::package_reader::{PackageReader, PackagesFromUpdateReader};
use crate::package_types::PartialPackageDefinition;
use anyhow::{Context, Result, anyhow, bail, format_err};
use fuchsia_hash::Hash;
use fuchsia_url::fuchsia_pkg::AbsolutePackageUrl;
use fuchsia_url::{PackageName, PackageVariant};
use log::{info, warn};
use scrutiny_collection::model::DataModel;
use scrutiny_collection::zbi::Zbi;
use scrutiny_utils::artifact::{ArtifactReader, FileArtifactReader};
use scrutiny_utils::bootfs::{BootfsFileIndex, BootfsPackageIndex, BootfsReader};
use scrutiny_utils::key_value::parse_key_value;
use scrutiny_utils::package::PackageIndexContents;
use scrutiny_utils::url::from_package_name_variant_path;
use scrutiny_utils::zbi::{ZbiReader, ZbiSection};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use update_package::parse_image_packages_json;

/// The path of the file in bootfs that lists all the bootfs packages.
const BOOT_PACKAGE_INDEX: &str = "data/bootfs_packages";
/// The path of the file in bootfs that lists all the bootfs packages.
const IMAGES_JSON_PATH: &str = "images.json";
const IMAGES_JSON_ORIG_PATH: &str = "images.json.orig";

/// A collector that returns the zbi contents in a product.
#[derive(Default)]
pub struct ZbiCollector;

impl ZbiCollector {
    pub fn collect(&self, model: Arc<DataModel>) -> Result<()> {
        let model_config = model.config();
        let blobs_directory = &model_config.blobs_directory();
        let artifact_reader = FileArtifactReader::new(
            &PathBuf::new(),
            blobs_directory,
            model_config.delivery_blob_type,
        );
        let mut package_reader: Box<dyn PackageReader> = Box::new(PackagesFromUpdateReader::new(
            &model_config.update_package_path(),
            Box::new(artifact_reader.clone()),
        ));
        let mut artifact_reader: Box<dyn ArtifactReader> = Box::new(artifact_reader);

        let update_package = package_reader
            .read_update_package_definition()
            .context("Failed to read update package definition for package data collector")?;
        let zbi = extract_zbi_from_update_package(
            &mut artifact_reader,
            &mut package_reader,
            &update_package,
            model.config().is_recovery(),
        )?;
        model.set(zbi)?;

        Ok(())
    }
}

/// Extracts the ZBI from the update package and parses it into the ZBI
/// model.
fn extract_zbi_from_update_package(
    artifact_reader: &mut Box<dyn ArtifactReader>,
    package_reader: &mut Box<dyn PackageReader>,
    update_package: &PartialPackageDefinition,
    recovery: bool,
) -> Result<Zbi> {
    info!("Extracting the ZBI from update package");

    let zbi_hash =
        lookup_zbi_hash_in_images_json(artifact_reader, package_reader, update_package, recovery)?;

    let zbi_data = artifact_reader.read_bytes(&Path::new(&zbi_hash.to_string()))?;
    let mut zbi_reader = ZbiReader::new(zbi_data);
    let sections = zbi_reader.parse()?;

    let mut deps = artifact_reader.get_deps();
    deps.extend(package_reader.get_deps());
    parse_zbi_payload(sections, deps)
}

fn parse_zbi_payload(sections: Vec<ZbiSection>, deps: HashSet<PathBuf>) -> Result<Zbi> {
    let mut bootfs_files = None;
    let mut cmdline_map = HashMap::new(); // Key to setting
    info!(total = sections.len(); "Extracted sections from the ZBI");
    for section in sections.iter() {
        info!(section_type:? = section.section_type; "Extracted sections");
        if section.section_type == zbi::Type::StorageBootfs {
            if bootfs_files.is_none() {
                let mut bootfs_reader = BootfsReader::new(section.buffer.clone());
                let files = bootfs_reader.parse().context("Failed to parse bootfs from ZBI")?;
                info!(total = files.len(); "Bootfs found files");
                bootfs_files = Some(files);
            } else {
                warn!("Multiple StorageBootfs sections found in ZBI; ignoring subsequent section");
            }
        } else if section.section_type == zbi::Type::Cmdline {
            let mut cmd_buffer = section.buffer.clone();
            // The cmdline.blk contains a trailing 0.
            cmd_buffer.truncate(cmd_buffer.len() - 1);
            let cmd_str = std::str::from_utf8(&cmd_buffer)
                .context("Failed to convert kernel arguments to utf-8")?;
            parse_cmdline(cmd_str, &mut cmdline_map);
        }
    }
    let mut cmdline: Vec<String> = cmdline_map.into_values().collect();
    cmdline.sort();

    let bootfs_files = bootfs_files.unwrap_or_default();

    // Find the bootfs package index
    let bootfs_pkg_contents = bootfs_files.iter().find_map(|(file_name, data)| {
        if file_name == BOOT_PACKAGE_INDEX { Some(data) } else { None }
    });
    let bootfs_packages: Option<Result<PackageIndexContents>> = bootfs_pkg_contents.map(|data| {
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
                format_err!("Failed to parse bootfs package index name/variant=merkle: {:?}", err)
            })?
            // Collect Vec<(_, __)> into HashMap<_, __>.
            .into_iter()
            .collect::<PackageIndexContents>();
        Ok(bootfs_pkgs)
    });
    let bootfs_files = BootfsFileIndex { bootfs_files };
    let bootfs_packages = BootfsPackageIndex { bootfs_pkgs: bootfs_packages.transpose()? };
    Ok(Zbi { deps, sections, bootfs_files, bootfs_packages, cmdline })
}

// Parses settings from a CMDLINE string payload into a key->setting hashmap.
fn parse_cmdline(cmd_str: &str, cmdline_map: &mut HashMap<String, String>) {
    for setting in cmd_str.split_whitespace() {
        let key = match setting.split_once('=') {
            Some((key, _)) => key,
            None => setting,
        };
        cmdline_map.insert(key.to_string(), setting.to_string());
    }
}

fn lookup_zbi_hash_in_images_json(
    artifact_reader: &mut Box<dyn ArtifactReader>,
    package_reader: &mut Box<dyn PackageReader>,
    update_package: &PartialPackageDefinition,
    recovery: bool,
) -> Result<Hash> {
    let images_json_hash = update_package
        .contents
        .get(&PathBuf::from(IMAGES_JSON_PATH))
        .or_else(|| update_package.contents.get(&PathBuf::from(IMAGES_JSON_ORIG_PATH)))
        .ok_or_else(|| anyhow!("Update package contains no images manifest entry"))?;
    let images_json_contents = artifact_reader
        .read_bytes(&Path::new(&images_json_hash.to_string()))
        .context("Failed to open images manifest blob designated in update package")?;
    let image_packages_manifest = parse_image_packages_json(images_json_contents.as_slice())
        .context("Failed to parse images manifest in update package")?;
    let metadata = if recovery {
        image_packages_manifest.recovery().ok_or_else(|| {
            anyhow!("Update package images manifest contains no recovery boot slot images")
        })
    } else {
        image_packages_manifest.fuchsia().ok_or_else(|| {
            anyhow!("Update package images manifest contains no fuchsia boot slot images")
        })
    }?;

    let images_component_url = metadata.zbi().url();
    let images_package_url = match metadata.zbi().url().package_url() {
        AbsolutePackageUrl::Unpinned(_) => bail!("Images package is not pinned"),
        AbsolutePackageUrl::Pinned(pinned) => pinned,
    };
    let images_package =
        package_reader.read_package_definition(&images_package_url).with_context(|| {
            format!(
                "Failed to located update package images package with URL {}",
                images_package_url
            )
        })?;

    let zbi_path = PathBuf::from(images_component_url.resource().as_ref());
    images_package.contents.get(&zbi_path).map(Hash::clone).ok_or_else(|| {
        anyhow!(
            "Update package images package contains no {} zbi entry {:?}",
            if recovery { "recovery" } else { "fuchsia" },
            zbi_path
        )
    })
}

#[cfg(test)]
mod tests {
    use super::ZbiCollector;
    use scrutiny_collection::model::DataModel;
    use scrutiny_collection::model_config::ModelConfig;
    use scrutiny_collection::zbi::Zbi;
    use std::sync::Arc;

    const PRODUCT_BUNDLE_PATH: &str = env!("PRODUCT_BUNDLE_PATH");

    #[test]
    fn bootfs() {
        let model = ModelConfig::from_product_bundle(PRODUCT_BUNDLE_PATH).unwrap();
        let data_model = Arc::new(DataModel::new(model).unwrap());
        let collector = ZbiCollector {};
        collector.collect(data_model.clone()).unwrap();
        let collection = data_model.get::<Zbi>().unwrap();
        assert!(
            collection.bootfs_files.bootfs_files.contains_key(&"bin/component_manager".to_string())
        );
    }

    #[fuchsia::test]
    fn cmdline() {
        let model = ModelConfig::from_product_bundle(PRODUCT_BUNDLE_PATH).unwrap();
        let data_model = Arc::new(DataModel::new(model).unwrap());
        let collector = ZbiCollector {};
        collector.collect(data_model.clone()).unwrap();
        let collection = data_model.get::<Zbi>().unwrap();
        assert!(collection.cmdline.iter().any(|arg| !arg.is_empty()));
    }

    #[test]
    fn test_parse_cmdline() {
        use super::parse_cmdline;
        use std::collections::HashMap;

        // Test simple parsing
        let mut map = HashMap::new();
        parse_cmdline("foo=bar baz=qux", &mut map);
        assert_eq!(map.len(), 2);
        assert_eq!(map.get("foo"), Some(&"foo=bar".to_string()));
        assert_eq!(map.get("baz"), Some(&"baz=qux".to_string()));

        // Test last wins for same key
        let mut map = HashMap::new();
        parse_cmdline("foo=bar foo=baz", &mut map);
        assert_eq!(map.len(), 1);
        assert_eq!(map.get("foo"), Some(&"foo=baz".to_string()));

        // Test key without value (boolean flag) is preserved
        let mut map = HashMap::new();
        parse_cmdline("foo", &mut map);
        assert_eq!(map.len(), 1);
        assert_eq!(map.get("foo"), Some(&"foo".to_string()));

        // Test mixed keys
        let mut map = HashMap::new();
        parse_cmdline("foo=bar foo baz=qux baz", &mut map);
        assert_eq!(map.len(), 2);
        assert_eq!(map.get("foo"), Some(&"foo".to_string()));
        assert_eq!(map.get("baz"), Some(&"baz".to_string()));

        // Test mixed keys reverse order
        let mut map = HashMap::new();
        parse_cmdline("foo foo=bar baz baz=qux", &mut map);
        assert_eq!(map.len(), 2);
        assert_eq!(map.get("foo"), Some(&"foo=bar".to_string()));
        assert_eq!(map.get("baz"), Some(&"baz=qux".to_string()));

        // Test multiple spaces
        let mut map = HashMap::new();
        parse_cmdline("foo=bar   baz=qux", &mut map);
        assert_eq!(map.len(), 2);
        assert_eq!(map.get("foo"), Some(&"foo=bar".to_string()));
        assert_eq!(map.get("baz"), Some(&"baz=qux".to_string()));

        // Test multiple sections (multiple calls)
        let mut map = HashMap::new();
        parse_cmdline("foo=bar", &mut map);
        parse_cmdline("foo=baz baz=qux", &mut map);
        assert_eq!(map.len(), 2);
        assert_eq!(map.get("foo"), Some(&"foo=baz".to_string()));
        assert_eq!(map.get("baz"), Some(&"baz=qux".to_string()));
    }

    fn make_bootfs_bytes(files: &[(&str, &[u8])]) -> Vec<u8> {
        use scrutiny_utils::bootfs::BOOTFS_MAGIC;

        let mut dir_entries = Vec::new();
        let mut file_payloads = Vec::new();

        for (name, data) in files {
            let name_bytes = name.as_bytes();
            let name_len = name_bytes.len() as u32;
            let data_len = data.len() as u32;
            let entry_start = dir_entries.len();
            dir_entries.extend_from_slice(&name_len.to_le_bytes());
            dir_entries.extend_from_slice(&data_len.to_le_bytes());
            dir_entries.extend_from_slice(&0u32.to_le_bytes());
            dir_entries.extend_from_slice(name_bytes);
            let entry_len = dir_entries.len() - entry_start;
            if entry_len % 4 != 0 {
                let padding = 4 - (entry_len % 4);
                dir_entries.resize(dir_entries.len() + padding, 0);
            }
            file_payloads.push(*data);
        }

        let header_size: u32 = 16;
        let dir_size = dir_entries.len() as u32;
        let mut offset = header_size + dir_size;

        let mut cursor = 0;
        for (i, (_, data)) in files.iter().enumerate() {
            let name_len = files[i].0.as_bytes().len() as u32;
            dir_entries[cursor + 8..cursor + 12].copy_from_slice(&offset.to_le_bytes());
            offset += data.len() as u32;
            let mut entry_len = 12 + name_len as usize;
            if entry_len % 4 != 0 {
                entry_len += 4 - (entry_len % 4);
            }
            cursor += entry_len;
        }

        let mut result = Vec::new();
        result.extend_from_slice(&BOOTFS_MAGIC.to_le_bytes());
        result.extend_from_slice(&dir_size.to_le_bytes());
        result.extend_from_slice(&0u32.to_le_bytes());
        result.extend_from_slice(&0u32.to_le_bytes());
        result.extend_from_slice(&dir_entries);
        for payload in file_payloads {
            result.extend_from_slice(payload);
        }
        result
    }

    #[test]
    fn test_multiple_storage_bootfs_uses_first_section() {
        use super::parse_zbi_payload;
        use scrutiny_utils::zbi::ZbiSection;
        use std::collections::HashSet;

        let bootfs1 = make_bootfs_bytes(&[("bin/first", b"first_content")]);
        let bootfs2 = make_bootfs_bytes(&[("bin/second", b"second_content")]);

        let sections = vec![
            ZbiSection { section_type: zbi::Type::StorageBootfs, buffer: bootfs1 },
            ZbiSection { section_type: zbi::Type::StorageBootfs, buffer: bootfs2 },
        ];

        let zbi = parse_zbi_payload(sections, HashSet::new()).unwrap();
        assert!(zbi.bootfs_files.bootfs_files.contains_key("bin/first"));
        assert_eq!(zbi.bootfs_files.bootfs_files.get("bin/first").unwrap(), b"first_content");
        assert!(!zbi.bootfs_files.bootfs_files.contains_key("bin/second"));
    }

    #[test]
    fn test_multiple_storage_bootfs_first_corrupted_fails_and_does_not_fallback() {
        use super::parse_zbi_payload;
        use scrutiny_utils::zbi::ZbiSection;
        use std::collections::HashSet;

        let corrupted_bootfs = vec![0u8; 32];
        let bootfs2 = make_bootfs_bytes(&[("bin/second", b"second_content")]);

        let sections = vec![
            ZbiSection { section_type: zbi::Type::StorageBootfs, buffer: corrupted_bootfs },
            ZbiSection { section_type: zbi::Type::StorageBootfs, buffer: bootfs2 },
        ];

        let result = parse_zbi_payload(sections, HashSet::new());
        assert!(result.is_err());
    }
}
