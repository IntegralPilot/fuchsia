// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Version 2 of the Product Bundle format.
//!
//! This format is drastically different from Version 1 in that all the contents
//! are expected to stay as implementation detail of ffx. The outputs of
//! assembly are fed directly into the fields of the Product Bundle, and the
//! flash and emulator manifests are not constructed until the Product Bundle
//! is read by `ffx emu start` and `ffx target flash`. This makes the format
//! simpler, and more aligned with how images are assembled.
//!
//! Note on paths
//! -------------
//! PBv2 is a directory containing images and other artifacts necessary to
//! flash, emulator, and update a product. When a Product Bundle is written to
//! disk, the paths inside _must_ all be relative to the Product Bundle itself,
//! to ensure that the directory remains portable (can be moved, zipped, tarred,
//! downloaded on another machine).

use assembled_system::Image;
use assembly_partitions_config::PartitionsConfig;
use assembly_release_info::ProductBundleReleaseInfo;
use camino::{Utf8Path, Utf8PathBuf};
use fuchsia_merkle::Hash;
use fuchsia_repo::repo_client::RepoClient;
use fuchsia_repo::repository::{Error as RepoError, FileSystemRepository};
use pathdiff::diff_utf8_paths;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashSet;
use std::str::FromStr;

/// Description of the data needed to set up (flash) a device.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProductBundleV2 {
    /// The <product>.<board> for the product bundle.
    /// TODO(https://fxbug.dev/418775628): Remove this field in
    /// favor of the name field in release_info.
    pub product_name: String,

    /// Unique version of this <product>.<board>.
    /// TODO(https://fxbug.dev/418775628): Remove this field in
    /// favor of the version field in release_info.
    pub product_version: String,

    /// The physical partitions of the target to place images into.
    pub partitions: PartitionsConfig,

    /// Fuchsia SDK version used to build this product bundle.
    /// TODO(https://fxbug.dev/418775628): Remove this field in
    /// favor of the sdk_version field in release_info.
    pub sdk_version: String,

    /// An assembled system that should be placed in slot A on the target.
    #[serde(default)]
    pub system_a: Option<Vec<Image>>,

    /// The platform tools for interacting with images in system_a.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub platform_tools_a: Vec<Utf8PathBuf>,

    /// An assembled system that should be placed in slot B on the target.
    #[serde(default)]
    pub system_b: Option<Vec<Image>>,

    /// The platform tools for interacting with images in system_b.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub platform_tools_b: Vec<Utf8PathBuf>,

    /// An assembled system that should be placed in slot R on the target.
    #[serde(default)]
    pub system_r: Option<Vec<Image>>,

    /// The platform tools for interacting with images in system_r.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub platform_tools_r: Vec<Utf8PathBuf>,

    /// Version information for this product bundle.
    pub release_info: Option<ProductBundleReleaseInfo>,

    /// The repositories that hold the TUF metadata, packages, and blobs.
    #[serde(default)]
    pub repositories: Vec<Repository>,

    /// The merkle-root of the update package that is added to the repository.
    #[serde(default)]
    pub update_package_hash: Option<Hash>,

    /// The relative path from the product bundle base directory to a file
    /// containing the Virtual Device manifest.
    #[serde(default)]
    pub virtual_devices_path: Option<Utf8PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum V2Error {
    #[error("Failed to create repo client: {0}")]
    CreateRepoClient(#[source] RepoError),

    #[error("Failed to update repo metadata: {0}")]
    UpdateRepoMetadata(#[source] RepoError),

    #[error("Failed to list packages: {0}")]
    ListPackages(#[source] RepoError),

    #[error("Failed to show package {0}: {1}")]
    ShowPackage(String, #[source] anyhow::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum CanonicalizeError {
    #[error("No parent for {0}")]
    NoParent(Utf8PathBuf),

    #[error("No file name for {0}")]
    NoFileName(Utf8PathBuf),

    #[error("Failed to create directory {0}: {1}")]
    CreateDir(Utf8PathBuf, #[source] std::io::Error),

    #[error("Failed to canonicalize path {0}: {1}")]
    Canonicalize(Utf8PathBuf, #[source] std::io::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum RelativizeError {
    #[error("Failed to rebase path {0} to {1}")]
    Rebase(Utf8PathBuf, Utf8PathBuf),
}

/// A repository that holds all the packages, blobs, and keys.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Repository {
    /// The unique name of the repository, such as "fuchsia.com".
    pub name: String,

    /// The path to the TUF repository, relative to the product bundle directory.
    pub metadata_path: Utf8PathBuf,

    /// The path to the blobs directory, relative to the product bundle
    /// directory.
    pub blobs_path: Utf8PathBuf,

    /// Specify the delivery blob type of each blob.
    /// This is used by the repo server, not the product bundle per se.
    /// See //src/storage/blobfs/delivery_blob.h for the list of types.
    #[serde(default = "default_delivery_blob_type")]
    pub delivery_blob_type: u32,

    /// The path to the file containing all the root metadata private keys.
    pub root_private_key_path: Option<Utf8PathBuf>,

    /// The path to the file containing all the targets metadata private keys.
    pub targets_private_key_path: Option<Utf8PathBuf>,

    /// The path to the file containing all the snapshot metadata private keys.
    pub snapshot_private_key_path: Option<Utf8PathBuf>,

    /// The path to the file containing all the timestamp metadata private keys.
    pub timestamp_private_key_path: Option<Utf8PathBuf>,

    /// Deprecated: The path to the update package manifest signature file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ota_manifest_signature_path: Option<Utf8PathBuf>,

    /// The path to the OTA manifest file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ota_manifest_path: Option<Utf8PathBuf>,
}

fn default_delivery_blob_type() -> u32 {
    1
}

impl Repository {
    /// Return the path to the targets.json file that contains a list of all the
    /// packages.
    pub fn targets_path(&self) -> Utf8PathBuf {
        self.metadata_path.join("targets.json")
    }

    /// Return the paths-on-host of the blobs contained in the product bundle.
    pub async fn blobs(&self) -> std::result::Result<HashSet<Utf8PathBuf>, V2Error> {
        let mut all_blobs = HashSet::<Utf8PathBuf>::new();
        let repo = FileSystemRepository::new(self.metadata_path.clone(), self.blobs_path.clone());
        let mut client =
            RepoClient::from_trusted_remote(&repo).await.map_err(V2Error::CreateRepoClient)?;
        client.update().await.map_err(V2Error::UpdateRepoMetadata)?;
        let packages = client.list_packages().await.map_err(V2Error::ListPackages)?;
        for package in &packages {
            if let Some(blobs) = client
                .show_package(&package.name, true)
                .await
                .map_err(|e| V2Error::ShowPackage(package.name.clone(), e))?
            {
                all_blobs.extend(
                    blobs.iter().filter_map(|e| e.hash.map(|hash| hash.to_string().into())),
                );
            }
        }
        Ok(all_blobs)
    }
}

/// The use-case a particular image is needed for.
#[derive(Clone, Debug, Copy, Hash, PartialEq, Eq)]
pub enum Type {
    /// No use-case.
    None,
    /// This image is needed for flashing.
    Flash,
    /// This image is needed for emulating.
    Emu,
    /// This image is needed for OTA updates.
    Update,
    /// This image is a bootloader image.
    Bootloader,
    /// This image is needed for all use-cases.
    All,
}

impl FromStr for Type {
    type Err = std::convert::Infallible;
    fn from_str(value: &str) -> std::result::Result<Type, std::convert::Infallible> {
        match value.to_lowercase().as_str() {
            "flash" => Ok(Type::Flash),
            "emu" => Ok(Type::Emu),
            "update" => Ok(Type::Update),
            "bootloader" => Ok(Type::Bootloader),
            "all" => Ok(Type::All),
            // Return None for others, to allow for infallible parsing.
            _ => Ok(Type::None),
        }
    }
}

pub(crate) trait Canonicalizer {
    fn canonicalize_path(&self, path: impl AsRef<Utf8Path>, image_types: Vec<Type>) -> Utf8PathBuf;
    fn root_path(&self) -> &Utf8PathBuf;

    fn canonicalize_system(&self, system: &mut Option<Vec<Image>>) {
        if let Some(system) = system {
            for image in system.iter_mut() {
                let image_types = match image {
                    Image::ZBI { path: _, signed: _ }
                    | Image::FxfsSparse { path: _, contents: _ } => {
                        vec![Type::Emu, Type::Flash]
                    }
                    Image::QemuKernel(_) | Image::FVM(_) => vec![Type::Emu],
                    Image::FVMFastboot(_) | Image::VBMeta(_) | Image::VBMetaSystem(_) => {
                        vec![Type::Flash]
                    }
                    _ => vec![],
                };

                image.set_source(
                    &self.canonicalize_path(&image.source().to_path_buf(), image_types),
                );
            }
        }
    }

    fn canonicalize_dir(&self, path: &Utf8Path) -> Utf8PathBuf {
        self.canonicalize_path(path, vec![Type::Update])
    }
}

pub(crate) struct DiskCanonicalizer {
    product_bundle_dir: Utf8PathBuf,
    not_supported: RefCell<HashSet<Type>>,
}

impl DiskCanonicalizer {
    pub(crate) fn new(product_bundle_dir: impl Into<Utf8PathBuf>) -> Self {
        Self {
            product_bundle_dir: product_bundle_dir.into(),
            not_supported: RefCell::new(HashSet::new()),
        }
    }
}

impl Canonicalizer for DiskCanonicalizer {
    fn root_path(&self) -> &Utf8PathBuf {
        &self.product_bundle_dir
    }

    fn canonicalize_path(&self, path: impl AsRef<Utf8Path>, image_types: Vec<Type>) -> Utf8PathBuf {
        let product_bundle_path = self.product_bundle_dir.join(path.as_ref());
        match product_bundle_path.canonicalize_utf8() {
            Ok(p) => p,
            Err(e) => {
                log::debug!("Cannot canonicalize {}: {e}", path.as_ref());
                for image_type in &image_types {
                    let mut ns = self.not_supported.borrow_mut();
                    ns.insert(image_type.clone());
                }
                self.product_bundle_dir.join(path)
            }
        }
    }
    fn canonicalize_dir(&self, path: &Utf8Path) -> Utf8PathBuf {
        let dir = self.product_bundle_dir.join(path);
        // Create the directory to ensure that canonicalize will work.
        if !dir.exists() {
            if let Err(e) = std::fs::create_dir_all(&dir) {
                eprintln!("Cannot create directory: {}, {:#?}", dir, e);
            }
        }
        self.canonicalize_path(path, vec![Type::Update])
    }
}

impl ProductBundleV2 {
    /// Convert all the paths from relative to absolute, assuming
    /// `product_bundle_dir` is the current base all the paths are relative to.
    ///
    /// Note: This function is intentionally only accessible inside the crate to
    /// ensure this method is only called during deserialization. Clients should
    /// not be canonicalizing their own paths.
    pub(crate) fn canonicalize_paths(
        &mut self,
        product_bundle_dir: impl AsRef<Utf8Path>,
    ) -> std::result::Result<(), CanonicalizeError> {
        let mut canonicalizer = DiskCanonicalizer::new(product_bundle_dir.as_ref());

        let res = self.canonicalize_paths_with(product_bundle_dir, &mut canonicalizer);

        if !canonicalizer.not_supported.borrow().is_empty() {
            log::warn!(
                "Warning: Missing artifacts. The following functionality will not work correctly: {:#?}",
                canonicalizer.not_supported
            );
        }
        res
    }

    pub(crate) fn canonicalize_paths_with<T>(
        &mut self,
        product_bundle_dir: impl AsRef<Utf8Path>,
        canonicalizer: &mut T,
    ) -> std::result::Result<(), CanonicalizeError>
    where
        T: Canonicalizer,
    {
        let product_bundle_dir = product_bundle_dir.as_ref();

        // Canonicalize the partitions.
        for part in &mut self.partitions.bootstrap_partitions {
            part.image = canonicalizer.canonicalize_path(&part.image, vec![Type::Flash]);
        }
        for part in &mut self.partitions.bootloader_partitions {
            part.image = canonicalizer.canonicalize_path(&part.image, vec![Type::Flash]);
        }
        for cred in &mut self.partitions.unlock_credentials {
            *cred = canonicalizer.canonicalize_path(&cred, vec![Type::Flash]);
        }

        // Canonicalize the systems.
        canonicalizer.canonicalize_system(&mut self.system_a);
        canonicalizer.canonicalize_system(&mut self.system_b);
        canonicalizer.canonicalize_system(&mut self.system_r);

        for tool in &mut self.platform_tools_a {
            *tool = canonicalizer.canonicalize_path(&tool, vec![]);
        }
        for tool in &mut self.platform_tools_b {
            *tool = canonicalizer.canonicalize_path(&tool, vec![]);
        }
        for tool in &mut self.platform_tools_r {
            *tool = canonicalizer.canonicalize_path(&tool, vec![]);
        }

        for repository in &mut self.repositories {
            repository.metadata_path = canonicalizer.canonicalize_dir(&repository.metadata_path);
            repository.blobs_path = canonicalizer.canonicalize_dir(&repository.blobs_path);
            if let Some(path) = &repository.root_private_key_path {
                repository.root_private_key_path = Some(canonicalizer.canonicalize_dir(path));
            }
            if let Some(path) = &repository.targets_private_key_path {
                repository.targets_private_key_path = Some(canonicalizer.canonicalize_dir(path));
            }
            if let Some(path) = &repository.snapshot_private_key_path {
                repository.snapshot_private_key_path = Some(canonicalizer.canonicalize_dir(path));
            }
            if let Some(path) = &repository.timestamp_private_key_path {
                repository.timestamp_private_key_path = Some(canonicalizer.canonicalize_dir(path));
            }
            if let Some(path) = &repository.ota_manifest_path {
                repository.ota_manifest_path = Some(canonicalizer.canonicalize_dir(path));
            }
        }

        // Canonicalize the virtual device specifications path.
        if let Some(path) = &self.virtual_devices_path {
            let virtual_devices_path = product_bundle_dir.join(path);
            let dir = virtual_devices_path
                .parent()
                .ok_or_else(|| CanonicalizeError::NoParent(virtual_devices_path.clone()))?;
            let base = virtual_devices_path
                .file_name()
                .ok_or_else(|| CanonicalizeError::NoFileName(virtual_devices_path.clone()))?;

            // Create the directory to ensure that canonicalize will work.
            if !dir.exists() {
                std::fs::create_dir_all(&dir)
                    .map_err(|e| CanonicalizeError::CreateDir(dir.to_path_buf(), e))?;
            }
            // Only canonicalize the directory.
            // This prevents problems when virtual_devices_path is a symlink.
            self.virtual_devices_path = Some(
                dir.canonicalize_utf8()
                    .map_err(|e| CanonicalizeError::Canonicalize(dir.to_path_buf(), e))?
                    .join(base),
            );
        }

        Ok(())
    }

    /// Convert all the paths from absolute to relative, assuming
    /// `product_bundle_dir` is the new base all the paths should be relative
    /// to.
    ///
    /// Note: This function is intentionally only accessible inside the crate to
    /// ensure this method is only called during deserialization. Clients should
    /// not be canonicalizing their own paths.
    pub(crate) fn relativize_paths(
        &mut self,
        product_bundle_dir: impl AsRef<Utf8Path>,
    ) -> std::result::Result<(), RelativizeError> {
        let product_bundle_dir = product_bundle_dir.as_ref();

        // Relativize the partitions.
        for part in &mut self.partitions.bootstrap_partitions {
            part.image = diff_utf8_paths(&part.image, &product_bundle_dir).ok_or_else(|| {
                RelativizeError::Rebase(part.image.clone(), product_bundle_dir.to_path_buf())
            })?;
        }
        for part in &mut self.partitions.bootloader_partitions {
            part.image = diff_utf8_paths(&part.image, &product_bundle_dir).ok_or_else(|| {
                RelativizeError::Rebase(part.image.clone(), product_bundle_dir.to_path_buf())
            })?;
        }
        for cred in &mut self.partitions.unlock_credentials {
            *cred = diff_utf8_paths(&cred, &product_bundle_dir).ok_or_else(|| {
                RelativizeError::Rebase(cred.clone(), product_bundle_dir.to_path_buf())
            })?;
        }

        // Relativize the systems.
        let relativize_system =
            |system: &mut Option<Vec<Image>>| -> std::result::Result<(), RelativizeError> {
                if let Some(system) = system {
                    for image in system.iter_mut() {
                        let path = diff_utf8_paths(&image.source(), &product_bundle_dir)
                            .ok_or_else(|| {
                                RelativizeError::Rebase(
                                    image.source().to_path_buf(),
                                    product_bundle_dir.to_path_buf(),
                                )
                            })?;
                        image.set_source(path);
                    }
                }
                Ok(())
            };
        relativize_system(&mut self.system_a)?;
        relativize_system(&mut self.system_b)?;
        relativize_system(&mut self.system_r)?;

        let relativize_tools =
            |tools: &mut Vec<Utf8PathBuf>| -> std::result::Result<(), RelativizeError> {
                for tool in tools.iter_mut() {
                    *tool = diff_utf8_paths(&*tool, &product_bundle_dir).ok_or_else(|| {
                        RelativizeError::Rebase(tool.clone(), product_bundle_dir.to_path_buf())
                    })?;
                }
                Ok(())
            };
        relativize_tools(&mut self.platform_tools_a)?;
        relativize_tools(&mut self.platform_tools_b)?;
        relativize_tools(&mut self.platform_tools_r)?;

        for repository in &mut self.repositories {
            let relativize_dir =
                |path: &Utf8PathBuf| -> std::result::Result<Utf8PathBuf, RelativizeError> {
                    diff_utf8_paths(&path, &product_bundle_dir).ok_or_else(|| {
                        RelativizeError::Rebase(path.clone(), product_bundle_dir.to_path_buf())
                    })
                };
            repository.metadata_path = relativize_dir(&repository.metadata_path)?;
            repository.blobs_path = relativize_dir(&repository.blobs_path)?;
            if let Some(path) = &repository.root_private_key_path {
                repository.root_private_key_path = Some(relativize_dir(path)?);
            }
            if let Some(path) = &repository.targets_private_key_path {
                repository.targets_private_key_path = Some(relativize_dir(path)?);
            }
            if let Some(path) = &repository.snapshot_private_key_path {
                repository.snapshot_private_key_path = Some(relativize_dir(path)?);
            }
            if let Some(path) = &repository.timestamp_private_key_path {
                repository.timestamp_private_key_path = Some(relativize_dir(path)?);
            }
            if let Some(path) = &repository.ota_manifest_path {
                repository.ota_manifest_path = Some(relativize_dir(path)?);
            }
        }

        // Relativize the virtual device specifications.
        if let Some(path) = &self.virtual_devices_path {
            self.virtual_devices_path =
                Some(diff_utf8_paths(&path, &product_bundle_dir).ok_or_else(|| {
                    RelativizeError::Rebase(path.clone(), product_bundle_dir.to_path_buf())
                })?);
        }

        Ok(())
    }

    /// Return the path to the Virtual Device Manifest.
    pub fn get_virtual_devices_path(&self) -> Option<Utf8PathBuf> {
        self.virtual_devices_path.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assembly_partitions_config::{BootloaderPartition, BootstrapPartition, Partition, Slot};
    use fuchsia_repo::test_utils;
    use std::fs::File;
    use std::io::Write;
    use tempfile::TempDir;

    // This tests backwards compatibility with the old format where a full fxfs
    // image has a 'contents' field.
    #[test]
    fn test_fxfs_has_contents() {
        let value = serde_json::json!({
            "product_name": "name",
            "product_version": "version",
            "partitions": {
                "hardware_revision": "hw",
            },
            "sdk_version": "sdk_version",
            "system_a": [
                {
                    "type": "fxfs-blk",
                    "name": "storage-full",
                    "path": "path",
                    "contents": {
                        "packages": {
                            "base": [],
                            "cache": [],
                        },
                    },
                },
            ],
        });
        let pb: ProductBundleV2 = serde_json::from_value(value).unwrap();
        assert_eq!(pb.system_a, Some(vec![Image::Fxfs("path".into())]));
    }

    #[test]
    fn test_canonicalize_no_paths() {
        let mut pb = ProductBundleV2 {
            product_name: "".to_string(),
            product_version: "".to_string(),
            partitions: PartitionsConfig {
                bootstrap_partitions: vec![],
                bootloader_partitions: vec![],
                partitions: vec![],
                hardware_revision: "board".into(),
                unlock_credentials: vec![],
                product_matches: vec![],
                ssh_key_upload_method: None,
            },
            sdk_version: "".to_string(),
            system_a: None,
            system_b: None,
            system_r: None,
            platform_tools_a: vec![],
            platform_tools_b: vec![],
            platform_tools_r: vec![],
            repositories: vec![],
            update_package_hash: None,
            virtual_devices_path: None,
            release_info: None,
        };
        let result = pb.canonicalize_paths(&Utf8PathBuf::from("path/to/product_bundle"));
        assert!(result.is_ok(), "{:?}", result.unwrap_err());
    }

    #[test]
    fn test_canonicalize_with_paths() {
        let tmp = TempDir::new().unwrap();
        // Canonicalize tempdir because on macOS /tmp is a symlink to /private/tmp
        let tempdir = Utf8Path::from_path(tmp.path()).unwrap().canonicalize_utf8().unwrap();

        let create_temp_file = |name: &str| {
            let path = tempdir.join(name);
            let mut file = File::create(path).unwrap();
            write!(file, "{}", name).unwrap();
        };

        create_temp_file("bootstrap");
        create_temp_file("bootloader");
        create_temp_file("zbi");
        create_temp_file("vbmeta");
        create_temp_file("unlock_credentials");
        create_temp_file("device");
        create_temp_file("tool");

        let mut pb = ProductBundleV2 {
            product_name: "".to_string(),
            product_version: "".to_string(),
            partitions: PartitionsConfig {
                bootstrap_partitions: vec![BootstrapPartition {
                    name: "bootstrap".into(),
                    image: "bootstrap".into(),
                    condition: None,
                }],
                bootloader_partitions: vec![BootloaderPartition {
                    partition_type: "bl2".into(),
                    name: None,
                    image: "bootloader".into(),
                }],
                partitions: vec![
                    Partition::ZBI { name: "zbi".into(), slot: Slot::A, size: None },
                    Partition::VBMeta { name: "vbmeta".into(), slot: Slot::A, size: None },
                    Partition::FVM { name: "fvm".into(), size: None },
                ],
                hardware_revision: "board".into(),
                unlock_credentials: vec!["unlock_credentials".into()],
                product_matches: vec![],
                ssh_key_upload_method: None,
            },
            sdk_version: "".to_string(),
            system_a: Some(vec![
                Image::ZBI { path: "zbi".into(), signed: false },
                Image::VBMeta("vbmeta".into()),
                Image::FVM("fvm".into()),
            ]),
            system_b: None,
            system_r: None,
            platform_tools_a: vec!["tool".into()],
            platform_tools_b: vec![],
            platform_tools_r: vec![],
            repositories: vec![],
            update_package_hash: None,
            virtual_devices_path: Some("device".into()),
            release_info: None,
        };
        let result = pb.canonicalize_paths(&tempdir);
        assert!(result.is_ok());

        // Validate system_a. Note the fvm file does not exist in tempdir.
        for image in pb.system_a.unwrap() {
            match image {
                Image::ZBI { path, signed: _ } => {
                    assert_eq!(tempdir.join("zbi"), path);
                }
                Image::VBMeta(path) => {
                    assert_eq!(tempdir.join("vbmeta"), path);
                }
                Image::FVM(path) => {
                    assert_eq!(tempdir.join("fvm"), path);
                }
                _ => assert!(false),
            }
        }
        assert_eq!(tempdir.join("tool"), pb.platform_tools_a[0]);
    }

    #[test]
    fn test_relativize_no_paths() {
        let mut pb = ProductBundleV2 {
            product_name: "".to_string(),
            product_version: "".to_string(),
            partitions: PartitionsConfig {
                bootstrap_partitions: vec![],
                bootloader_partitions: vec![],
                partitions: vec![],
                hardware_revision: "board".into(),
                unlock_credentials: vec![],
                product_matches: vec![],
                ssh_key_upload_method: None,
            },
            sdk_version: "".to_string(),
            system_a: None,
            system_b: None,
            system_r: None,
            platform_tools_a: vec![],
            platform_tools_b: vec![],
            platform_tools_r: vec![],
            repositories: vec![],
            update_package_hash: None,
            virtual_devices_path: None,
            release_info: None,
        };
        let result = pb.relativize_paths(&Utf8PathBuf::from("path/to/product_bundle"));
        assert!(result.is_ok());
    }

    #[test]
    fn test_relativize_with_paths() {
        let tmp = TempDir::new().unwrap();
        let tempdir = Utf8Path::from_path(tmp.path()).unwrap();
        let vd_manifest = tempdir.join("virtual_devices.json");
        File::create(&vd_manifest).expect("Create vd manifest file");

        let mut pb = ProductBundleV2 {
            product_name: "".to_string(),
            product_version: "".to_string(),
            partitions: PartitionsConfig {
                bootstrap_partitions: vec![BootstrapPartition {
                    name: "bootstrap".into(),
                    image: tempdir.join("bootstrap"),
                    condition: None,
                }],
                bootloader_partitions: vec![BootloaderPartition {
                    partition_type: "bl2".into(),
                    name: None,
                    image: tempdir.join("bootloader"),
                }],
                partitions: vec![
                    Partition::ZBI { name: "zbi".into(), slot: Slot::A, size: None },
                    Partition::VBMeta { name: "vbmeta".into(), slot: Slot::A, size: None },
                    Partition::FVM { name: "fvm".into(), size: None },
                ],
                hardware_revision: "board".into(),
                unlock_credentials: vec![tempdir.join("unlock_credentials")],
                product_matches: vec![],
                ssh_key_upload_method: None,
            },
            sdk_version: "".to_string(),
            system_a: Some(vec![
                Image::ZBI { path: tempdir.join("zbi"), signed: false },
                Image::VBMeta(tempdir.join("vbmeta")),
                Image::FVM(tempdir.join("fvm")),
            ]),
            system_b: None,
            system_r: None,
            platform_tools_a: vec![tempdir.join("tool")],
            platform_tools_b: vec![],
            platform_tools_r: vec![],
            repositories: vec![],
            update_package_hash: None,
            virtual_devices_path: Some(vd_manifest),
            release_info: None,
        };
        let result = pb.relativize_paths(tempdir);
        assert!(result.is_ok());
        assert_eq!(Utf8PathBuf::from("tool"), pb.platform_tools_a[0]);
    }

    #[fuchsia::test]
    async fn test_blobs() {
        let tempdir = TempDir::new().unwrap();
        let dir = Utf8Path::from_path(&tempdir.path()).unwrap();
        let _repo = test_utils::make_pm_repo_dir(&tempdir.path()).await;

        let pb = ProductBundleV2 {
            product_name: "".to_string(),
            product_version: "".to_string(),
            partitions: PartitionsConfig::default(),
            sdk_version: "".to_string(),
            system_a: None,
            system_b: None,
            system_r: None,
            platform_tools_a: vec![],
            platform_tools_b: vec![],
            platform_tools_r: vec![],
            repositories: vec![Repository {
                name: "fuchsia.com".into(),
                metadata_path: dir.join("repository"),
                blobs_path: dir.join("repository").join("blobs"),
                delivery_blob_type: 1,
                root_private_key_path: None,
                targets_private_key_path: None,
                snapshot_private_key_path: None,
                timestamp_private_key_path: None,
                ota_manifest_signature_path: None,
                ota_manifest_path: None,
            }],
            update_package_hash: None,
            virtual_devices_path: None,
            release_info: None,
        };

        let expected = HashSet::from([
            "050907f009ff634f9aa57bff541fb9e9c2c62b587c23578e77637cda3bd69458".into(),
            "2881455493b5870aaea36537d70a2adc635f516ac2092598f4b6056dabc6b25d".into(),
            "548981eb310ddc4098fb5c63692e19ac4ae287b13d0e911fbd9f7819ac22491c".into(),
            "72e1e7a504f32edf4f23e7e8a3542c1d77d12541142261cfe272decfa75f542d".into(),
            "8a8a5f07f935a4e8e1fd1a1eda39da09bb2438ec0adfb149679ddd6e7e1fbb4f".into(),
            "ecc11f7f4b763c5a21be2b4159c9818bbe22ca7e6d8100a72f6a41d3d7b827a9".into(),
        ]);

        let blobs = pb.repositories[0].blobs().await.unwrap();
        assert_eq!(expected, blobs);
    }

    #[test]
    fn test_targets_path() {
        let pb = ProductBundleV2 {
            product_name: String::default(),
            product_version: String::default(),
            partitions: PartitionsConfig::default(),
            sdk_version: String::default(),
            system_a: None,
            system_b: None,
            system_r: None,
            platform_tools_a: vec![],
            platform_tools_b: vec![],
            platform_tools_r: vec![],
            repositories: vec![Repository {
                name: "fuchsia.com".into(),
                metadata_path: "repository".into(),
                blobs_path: "blobs".into(),
                delivery_blob_type: 1,
                root_private_key_path: None,
                targets_private_key_path: None,
                snapshot_private_key_path: None,
                timestamp_private_key_path: None,
                ota_manifest_signature_path: None,
                ota_manifest_path: None,
            }],
            update_package_hash: None,
            virtual_devices_path: None,
            release_info: None,
        };
        assert_eq!("repository/targets.json", pb.repositories[0].targets_path());
    }
}
