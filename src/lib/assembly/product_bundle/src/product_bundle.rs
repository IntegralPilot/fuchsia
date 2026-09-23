// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Representation of the product_bundle metadata.

use crate::v2::{CanonicalizeError, Canonicalizer, ProductBundleV2, RelativizeError, Type};

use anyhow::{Context as _, Result, anyhow};
use assembled_system::Image;
use camino::{Utf8Path, Utf8PathBuf};
use fuchsia_repo::repository::FileSystemRepository;
use rayon::prelude::*;
use sdk_metadata::{VirtualDevice, VirtualDeviceManifest, VirtualDeviceV1};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{BufRead, Write};
use std::ops::Deref;
use std::process::Command;
use zip::read::ZipArchive;

/// Errors that can occur when loading a product bundle.
#[derive(Debug, thiserror::Error)]
pub enum ProductBundleLoadError {
    /// The path is not a directory.
    #[error("{0} is not a directory")]
    NotADirectory(String),

    /// Failed to open a file.
    #[error("Failed to open file {0}: {1}")]
    OpenFile(Utf8PathBuf, #[source] std::io::Error),

    /// Failed to parse product bundle JSON.
    #[error("Failed to parse product bundle: {0}")]
    Parse(#[from] serde_json::Error),

    /// Product bundle v1 is no longer supported.
    #[error("Product bundle v1 is no longer supported")]
    V1NotSupported,

    /// Canonicalization error.
    #[error("Canonicalization error: {0}")]
    Canonicalization(#[from] CanonicalizeError),

    /// Zip error.
    #[error("Zip error: {0}")]
    Zip(#[from] zip::result::ZipError),

    /// File 'product_bundle.json' not found in zip archive.
    #[error("File 'product_bundle.json' not found in zip archive")]
    NotFoundInZip,

    /// Malformed zip archive.
    #[error("Malformed zip archive: {0}")]
    MalformedZip(String),
}

/// Errors that can occur when writing a product bundle.
#[derive(Debug, thiserror::Error)]
pub enum ProductBundleWriteError {
    /// Failed to create a file.
    #[error("Failed to create file {0}: {1}")]
    CreateFile(Utf8PathBuf, #[source] std::io::Error),

    /// Failed to write to a file.
    #[error("Failed to write to file: {0}")]
    WriteFile(#[from] serde_json::Error),

    /// Relativize paths error.
    #[error("Relativize paths error: {0}")]
    RelativizePaths(#[from] RelativizeError),
}

/// Errors that can occur when extracting blobs from a product bundle.
#[derive(Debug, thiserror::Error)]
pub enum ProductBundleExtractError {
    /// System does not exist for the specified slot.
    #[error("System does not exist for the specified slot")]
    SystemNotFound,

    /// System does not contain an fxfs image.
    #[error("System does not contain an fxfs image")]
    FxfsImageNotFound,

    /// fxfs_pbtool not found in platform_tools.
    #[error("fxfs_pbtool not found in platform_tools")]
    ToolNotFound,

    /// Failed to run extraction tool.
    #[error("Failed to run extraction tool: {0}")]
    RunTool(#[source] std::io::Error),

    /// Extraction tool failed with status.
    #[error("Extraction tool failed with status {0}.\nstdout: {1}\nstderr: {2}")]
    ToolFailed(std::process::ExitStatus, String, String),

    /// Failed to extract or process blobs.
    #[error("Failed to extract or process blobs: {0}")]
    ExtractionError(#[source] anyhow::Error),
}

/// Errors that can occur when operating on virtual devices in a product bundle.
#[derive(Debug, thiserror::Error)]
pub enum ProductBundleDeviceError {
    /// Failed to load virtual device manifest.
    #[error("Failed to load virtual device manifest: {0}")]
    ManifestError(#[source] anyhow::Error),

    /// No default virtual device is available.
    #[error("No default virtual device is available, please specify one by name.")]
    NoDefaultDevice,

    /// Device not found in manifest and failed to parse as path.
    #[error(
        "No virtual device matches '{device}': {name_err}\nWe were also not able to parse '{device}' as a virtual device file: {file_err}"
    )]
    DeviceNotFound { device: String, name_err: anyhow::Error, file_err: anyhow::Error },
}

/// Errors that can occur when getting repositories from a product bundle.
#[derive(Debug, thiserror::Error)]
pub enum GetRepositoriesError {
    /// Failed to load product bundle.
    #[error("Failed to load product bundle: {0}")]
    LoadProductBundle(#[from] ProductBundleLoadError),

    /// Failed to canonicalize path.
    #[error("Failed to canonicalize path {0}: {1}")]
    Canonicalize(Utf8PathBuf, #[source] std::io::Error),

    /// Failed to convert delivery blob type.
    #[error("Failed to convert delivery blob type: {0}")]
    DeliveryBlobType(#[from] delivery_blob::DeliveryBlobError),
}

fn try_load_product_bundle(r: impl BufRead) -> Result<ProductBundle, ProductBundleLoadError> {
    let helper: SerializationHelper =
        serde_json::from_reader(r).map_err(ProductBundleLoadError::Parse)?;
    match helper {
        SerializationHelper::V1 { schema_id: _ } => {
            return Err(ProductBundleLoadError::V1NotSupported);
        }
        SerializationHelper::V2(v2) => {
            let SerializationHelperVersioned::V2(data) = *v2;
            Ok(ProductBundle::V2(data))
        }
    }
}

/// A product bundle that was read from a zip file.
#[derive(Clone, Debug, PartialEq)]
pub struct ZipLoadedProductBundle {
    product_bundle: ProductBundle,
}

impl ZipLoadedProductBundle {
    /// Read a prdouct bundle from a zip file.
    pub fn try_load_from(
        product_bundle_zip_path: impl AsRef<Utf8Path>,
    ) -> Result<Self, ProductBundleLoadError> {
        let path = product_bundle_zip_path.as_ref();
        let file = File::open(path)
            .map_err(|e| ProductBundleLoadError::OpenFile(path.to_path_buf(), e))?;
        let zip = ZipArchive::new(file).map_err(ProductBundleLoadError::Zip)?;
        Self::load_from(zip)
    }

    /// Load a product bundle from an already parsed ZipArchive.
    pub fn load_from(mut zip: ZipArchive<File>) -> Result<Self, ProductBundleLoadError> {
        let product_bundle_manifest_name = zip
            .file_names()
            .find(|x| x == &"product_bundle.json" || x.ends_with("/product_bundle.json"))
            .ok_or(ProductBundleLoadError::NotFoundInZip)?
            .to_owned();

        let product_bundle_parent_path =
            product_bundle_manifest_name.strip_suffix("product_bundle.json").ok_or_else(|| {
                ProductBundleLoadError::MalformedZip(
                    "product_bundle.json path missing suffix".to_string(),
                )
            })?;

        let product_bundle_manifest =
            zip.by_name(&product_bundle_manifest_name).map_err(ProductBundleLoadError::Zip)?;
        let product_bundle_manifest = std::io::BufReader::new(product_bundle_manifest);
        // Still need to canonicalize paths as the path to the product bundle'suffix
        // parent directory may be arbitrarily deep in the zip file
        match try_load_product_bundle(product_bundle_manifest)? {
            ProductBundle::V2(data) => {
                let mut data = data;
                let mut canonicalizer = ZipCanonicalizer::new(product_bundle_parent_path);
                data.canonicalize_paths_with(product_bundle_parent_path, &mut canonicalizer)?;
                Ok(Self::new(ProductBundle::V2(data)))
            }
        }
    }

    /// Construct a new product bundle.
    pub fn new(product_bundle: ProductBundle) -> Self {
        Self { product_bundle }
    }
}

impl Deref for ZipLoadedProductBundle {
    type Target = ProductBundle;
    fn deref(&self) -> &Self::Target {
        &self.product_bundle
    }
}

impl Into<ProductBundle> for ZipLoadedProductBundle {
    fn into(self) -> ProductBundle {
        self.product_bundle
    }
}

struct ZipCanonicalizer {
    product_bundle_dir: Utf8PathBuf,
}

impl Canonicalizer for ZipCanonicalizer {
    fn root_path(&self) -> &Utf8PathBuf {
        &self.product_bundle_dir
    }

    fn canonicalize_path(
        &self,
        path: impl AsRef<Utf8Path>,
        _image_types: Vec<Type>,
    ) -> Utf8PathBuf {
        self.root_path().join(path)
    }
}

impl ZipCanonicalizer {
    fn new(product_bundle_dir: impl Into<Utf8PathBuf>) -> Self {
        Self { product_bundle_dir: product_bundle_dir.into() }
    }
}

/// Returns a representation of a ProductBundle that has been loaded from disk.
///
/// The loaded product bundle holds a reference to the path that it was loaded
/// from so it can be referenced later. This helps when understanding how a
/// product bundle was loaded when it might have come from a default path.
///
/// Most users of the product bundle will not need to know, or care, where it
/// came from so they can just convert into a Product bundle using into().
#[derive(Clone, Debug, PartialEq)]
pub struct LoadedProductBundle {
    product_bundle: ProductBundle,
    from_path: Utf8PathBuf,
}

impl LoadedProductBundle {
    /// Load a ProductBundle from a directory containing product_bundle.json
    /// on disk. This method will return a LoadedProductBundle which keeps
    /// track of where it was loaded from.
    pub fn try_load_from(path: impl AsRef<Utf8Path>) -> Result<Self, ProductBundleLoadError> {
        if !path.as_ref().is_dir() {
            return Err(ProductBundleLoadError::NotADirectory(path.as_ref().to_string()));
        }
        let product_bundle_path = path.as_ref().join("product_bundle.json");
        let file = File::open(&product_bundle_path)
            .map_err(|e| ProductBundleLoadError::OpenFile(product_bundle_path.clone(), e))?;
        let file = std::io::BufReader::new(file);

        match try_load_product_bundle(file)? {
            ProductBundle::V2(data) => {
                let mut data = data;
                data.canonicalize_paths(path.as_ref())?;
                Ok(LoadedProductBundle::new(ProductBundle::V2(data), path))
            }
        }
    }

    /// Creates a new LoadedProductBundle.
    ///
    /// Users should prefer the try_load_from method over creating this struct
    /// directly.
    pub fn new(product_bundle: ProductBundle, from_path: impl AsRef<Utf8Path>) -> Self {
        LoadedProductBundle { product_bundle, from_path: from_path.as_ref().into() }
    }

    /// Returns the path which the bundle was loaded from.
    pub fn loaded_from_path(&self) -> &Utf8Path {
        self.from_path.as_path()
    }
}

impl Deref for LoadedProductBundle {
    type Target = ProductBundle;
    fn deref(&self) -> &Self::Target {
        &self.product_bundle
    }
}

impl Into<ProductBundle> for LoadedProductBundle {
    fn into(self) -> ProductBundle {
        self.product_bundle
    }
}

/// Versioned product bundle.
#[derive(Clone, Debug, PartialEq)]
pub enum ProductBundle {
    /// Version 2 of the product bundle format.
    V2(ProductBundleV2),
}

/// Private helper for serializing the ProductBundle. A ProductBundle cannot be deserialized
/// without going through `try_from_path` in order to require that we use this helper, and the
/// `directory` field gets populated.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
enum SerializationHelper {
    V1 { schema_id: String },
    V2(Box<SerializationHelperVersioned>),
}

/// Helper for serializing the new system of versioning product bundles using the "version" tag.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "version")]
enum SerializationHelperVersioned {
    #[serde(rename = "2")]
    V2(ProductBundleV2),
}

impl ProductBundle {
    /// Read a product bundle from a path, whether it be a zip file or a
    /// directory.
    pub fn try_load_from(path: impl AsRef<Utf8Path>) -> Result<Self, ProductBundleLoadError> {
        let path = path.as_ref();
        if path.is_file() && path.extension() == Some("zip") {
            ZipLoadedProductBundle::try_load_from(path).map(|v| v.into())
        } else {
            LoadedProductBundle::try_load_from(path).map(|v| v.into())
        }
    }

    /// Write a product bundle to a directory on disk at `path`.
    /// Note that this only writes the manifest file, and not the artifacts, images, blobs.
    pub fn write(&self, path: impl AsRef<Utf8Path>) -> Result<(), ProductBundleWriteError> {
        let helper = match self {
            Self::V2(data) => {
                let mut data = data.clone();
                data.relativize_paths(path.as_ref())?;
                SerializationHelper::V2(Box::new(SerializationHelperVersioned::V2(data)))
            }
        };
        let product_bundle_path = path.as_ref().join("product_bundle.json");
        let file = File::create(&product_bundle_path)
            .map_err(|e| ProductBundleWriteError::CreateFile(product_bundle_path.clone(), e))?;
        serde_json::to_writer_pretty(file, &helper)?;
        Ok(())
    }

    /// Get the list of logical device names.
    pub fn device_refs(&self) -> Result<Vec<String>, ProductBundleDeviceError> {
        match self {
            Self::V2(data) => {
                let path = data.get_virtual_devices_path();
                let manifest = VirtualDeviceManifest::from_path(&path)
                    .map_err(ProductBundleDeviceError::ManifestError)?;
                Ok(manifest.device_names())
            }
        }
    }

    /// Get the product bundle name
    pub fn get_product_bundle_name(&self) -> String {
        match self {
            Self::V2(pb) => pb.product_name.clone(),
        }
    }

    /// Returns true if the product bundle uses FVM as its image system in the given slot.
    fn is_fvm(&self, slot: assembly_partitions_config::Slot) -> bool {
        let ProductBundle::V2(pb) = self;
        let system = match slot {
            assembly_partitions_config::Slot::A => &pb.system_a,
            assembly_partitions_config::Slot::B => &pb.system_b,
            assembly_partitions_config::Slot::R => &pb.system_r,
        };
        if let Some(system) = system {
            system.iter().any(|image| {
                matches!(image, Image::FVM(_) | Image::FVMSparse(_) | Image::FVMFastboot(_))
            })
        } else {
            false
        }
    }

    /// Attempts to load a `VirtualDeviceV1` from the product bundle with the
    /// given `device` name.
    /// If `device` is empty, loads the default recommended device instead.
    /// If `device` does not exist within the product bundle, `device` is
    /// instead interpreted as a virtual device file path.
    pub fn get_device(
        &self,
        device: &Option<String>,
    ) -> Result<VirtualDeviceV1, ProductBundleDeviceError> {
        let Self::V2(pb) = self;

        // Determine the correct device name from the user, or default to the "recommended"
        // device, if one is provided in the product bundle.
        let path = pb.get_virtual_devices_path();
        let manifest = VirtualDeviceManifest::from_path(&path)
            .map_err(ProductBundleDeviceError::ManifestError)?;
        let result = match device.as_deref() {
            // If no device is given, return the default specified in the manifest.
            None | Some("") => {
                manifest.default_device().map_err(ProductBundleDeviceError::ManifestError)?
            }

            // Otherwise, find the virtual device by name in the product bundle.
            Some(device) => manifest
                .device(device)
                .or_else(|name_err| {
                    // If we cannot find it in the product bundle, attempt to parse
                    // `device` as a virtual device file path.
                    VirtualDevice::try_load_from(Utf8Path::new(device)).map_err(|file_err| {
                        ProductBundleDeviceError::DeviceNotFound {
                            device: device.to_string(),
                            name_err,
                            file_err,
                        }
                    })
                })
                .map(|d| Some(d))?,
        };
        match result {
            Some(VirtualDevice::V1(virtual_device)) => Ok(virtual_device),
            None => Err(ProductBundleDeviceError::NoDefaultDevice),
        }
    }

    /// This is true if the product bundle is not FVM and has fxfs_pbtool.
    pub fn supports_extract_blobs(&self, slot: assembly_partitions_config::Slot) -> bool {
        let ProductBundle::V2(pb) = self;
        if self.is_fvm(slot) {
            return false;
        }
        let tools = match slot {
            assembly_partitions_config::Slot::A => &pb.platform_tools_a,
            assembly_partitions_config::Slot::B => &pb.platform_tools_b,
            assembly_partitions_config::Slot::R => &pb.platform_tools_r,
        };
        tools.iter().any(|p| p.file_name() == Some("fxfs_pbtool"))
    }

    /// Extract all blobs for the specified slot, sourcing them from the system's
    /// Fxfs image and copying some from the product bundle's repositories.
    pub fn extract_blobs(
        &self,
        slot: assembly_partitions_config::Slot,
        out_dir: impl AsRef<Utf8Path>,
        delivery_blob_type: Option<u32>,
    ) -> Result<(), ProductBundleExtractError> {
        let ProductBundle::V2(pb) = self;
        let parsed_delivery_blob_type = match delivery_blob_type {
            Some(t) => Some(
                delivery_blob::DeliveryBlobType::try_from(t)
                    .map_err(|e| ProductBundleExtractError::ExtractionError(anyhow!(e)))?,
            ),
            None => None,
        };

        let (system, platform_tools) = match slot {
            assembly_partitions_config::Slot::A => (&pb.system_a, &pb.platform_tools_a),
            assembly_partitions_config::Slot::B => (&pb.system_b, &pb.platform_tools_b),
            assembly_partitions_config::Slot::R => (&pb.system_r, &pb.platform_tools_r),
        };

        let out_dir_ref = out_dir.as_ref();
        let target_dir = match delivery_blob_type {
            Some(t) => out_dir_ref.join(t.to_string()),
            None => out_dir_ref.to_path_buf(),
        };

        std::fs::create_dir_all(&target_dir)
            .with_context(|| format!("Failed to create directory {target_dir}"))
            .map_err(ProductBundleExtractError::ExtractionError)?;

        let system_ref = system.as_ref();
        let image_path = system_ref.and_then(|s| {
            s.iter().find_map(|image| match image {
                Image::Fxfs(path) | Image::FxfsSparse { path, .. } => Some(path),
                _ => None,
            })
        });
        let tool = platform_tools.iter().find(|p| p.file_name() == Some("fxfs_pbtool"));

        let mut extracted_any_from_fxfs = false;

        // Try extracting from Fxfs image if supported
        if let (Some(image_path), Some(tool)) = (image_path, tool) {
            // Extract raw blobs from Fxfs image directly to target_dir
            let output = Command::new(tool)
                .arg("extract")
                .arg("--image")
                .arg(image_path)
                .arg("--out")
                .arg(&target_dir)
                .output()
                .map_err(ProductBundleExtractError::RunTool)?;

            if !output.status.success() {
                return Err(ProductBundleExtractError::ToolFailed(
                    output.status,
                    String::from_utf8_lossy(&output.stdout).to_string(),
                    String::from_utf8_lossy(&output.stderr).to_string(),
                ));
            }

            extracted_any_from_fxfs = true;

            if let Some(t) = parsed_delivery_blob_type {
                // Process extracted raw blobs (compress them in-place)
                let entries = std::fs::read_dir(&target_dir)
                    .context("Failed to read target dir")
                    .map_err(ProductBundleExtractError::ExtractionError)?
                    .collect::<Result<Vec<_>, std::io::Error>>()
                    .context("Failed to collect target dir entries")
                    .map_err(ProductBundleExtractError::ExtractionError)?;

                // Compress the extracted raw blobs to delivery blobs in-place concurrently.
                entries.par_iter().try_for_each(|entry| {
                    let path = entry.path();
                    if path.is_file() {
                        let file_name = path.file_name().ok_or_else(|| {
                            ProductBundleExtractError::ExtractionError(anyhow!(
                                "No file name for entry in target dir"
                            ))
                        })?;
                        let hash_str = file_name.to_string_lossy().to_string();
                        if <fuchsia_merkle::Hash as std::str::FromStr>::from_str(&hash_str).is_err()
                        {
                            return Ok(());
                        }

                        let raw_bytes = std::fs::read(&path)
                            .with_context(|| format!("Failed to read file {path:?}"))
                            .map_err(ProductBundleExtractError::ExtractionError)?;
                        let mut file = std::fs::File::create(&path)
                            .with_context(|| format!("Failed to create file {path:?}"))
                            .map_err(ProductBundleExtractError::ExtractionError)?;
                        delivery_blob::generate_to(t, &raw_bytes, &mut file)
                            .with_context(|| format!("Failed to write delivery blob to {path:?}"))
                            .map_err(ProductBundleExtractError::ExtractionError)?;
                    }
                    Ok(())
                })?;
            }
        }

        // Process remaining repository blobs that were NOT in the Fxfs image
        // In some cases (e.g. the scrutiny tool), update package blobs must be decompressed.
        let mut copied_any = false;
        if let Some(repo) = pb.repositories.first() {
            let dir = repo.blobs_path.join(repo.delivery_blob_type.to_string());
            if dir.exists() {
                if let Ok(entries) = std::fs::read_dir(&dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.is_file() {
                            let file_name = path.file_name().ok_or_else(|| {
                                ProductBundleExtractError::ExtractionError(anyhow!(
                                    "No file name for repository blob entry"
                                ))
                            })?;
                            let hash_str = file_name.to_string_lossy().to_string();

                            let dest_path = target_dir.join(&hash_str);
                            if dest_path.exists() {
                                continue;
                            }

                            let file_bytes = std::fs::read(&path)
                                .with_context(|| format!("Failed to read file {path:?}"))
                                .map_err(ProductBundleExtractError::ExtractionError)?;
                            let mut file = std::fs::File::create(&dest_path)
                                .with_context(|| format!("Failed to create file {dest_path}"))
                                .map_err(ProductBundleExtractError::ExtractionError)?;
                            match parsed_delivery_blob_type {
                                Some(t) if u32::from(t) == repo.delivery_blob_type => {
                                    file.write_all(&file_bytes)
                                        .with_context(|| {
                                            format!("Failed to write file {dest_path}")
                                        })
                                        .map_err(ProductBundleExtractError::ExtractionError)?;
                                }
                                Some(t) => {
                                    let decompressed = delivery_blob::decompress(&file_bytes)
                                        .context("Failed to decompress blob")
                                        .map_err(ProductBundleExtractError::ExtractionError)?;
                                    delivery_blob::generate_to(t, &decompressed, &mut file)
                                        .context("Failed to generate delivery blob")
                                        .map_err(ProductBundleExtractError::ExtractionError)?;
                                }
                                None => {
                                    delivery_blob::decompress_to(&file_bytes, &mut file)
                                        .context("Failed to decompress blob")
                                        .map_err(ProductBundleExtractError::ExtractionError)?;
                                }
                            }
                            copied_any = true;
                        }
                    }
                }
            }
        }

        // If no blobs were extracted from Fxfs, and no blobs were copied from
        // repositories, return error
        if !extracted_any_from_fxfs && !copied_any {
            if system_ref.is_some() {
                if image_path.is_none() {
                    return Err(ProductBundleExtractError::FxfsImageNotFound);
                }
                if tool.is_none() {
                    return Err(ProductBundleExtractError::ToolNotFound);
                }
            }
            return Err(ProductBundleExtractError::SystemNotFound);
        }

        Ok(())
    }
}

/// Construct a Vec<FileSystemRepository> from product bundle.
pub fn get_repositories(
    product_bundle_dir: Utf8PathBuf,
) -> Result<Vec<FileSystemRepository>, GetRepositoriesError> {
    let pb = match ProductBundle::try_load_from(&product_bundle_dir)? {
        ProductBundle::V2(pb) => pb,
    };

    let mut repos = Vec::<FileSystemRepository>::new();
    for repo in pb.repositories {
        let repo_builder = FileSystemRepository::builder(
            repo.metadata_path
                .canonicalize_utf8()
                .map_err(|e| GetRepositoriesError::Canonicalize(repo.metadata_path.clone(), e))?,
            repo.blobs_path
                .canonicalize_utf8()
                .map_err(|e| GetRepositoriesError::Canonicalize(repo.blobs_path.clone(), e))?,
        )
        .alias(repo.name)
        .delivery_blob_type(repo.delivery_blob_type.try_into()?);
        repos.push(repo_builder.build());
    }
    Ok(repos)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use assembled_system::AssembledSystem;
    use assembly_cli_args::{AssemblyMode, ValidationMode};
    use assembly_container::AssemblyContainer;
    use assembly_partitions_config::Slot;
    use assembly_tool::PlatformToolProvider;
    use image_assembly_config_builder::ImageAssemblyConfigBuilder;
    use serde_json::json;
    use std::fs;
    use std::io::Write;
    use tempfile::TempDir;
    use zip::write::SimpleFileOptions as FileOptions;
    use zip::{CompressionMethod, ZipWriter};

    const VIRTUAL_DEVICE_VALID: &str =
        include_str!("../../../../../build/sdk/meta/test_data/virtual_device.json");

    fn make_sample_pbv1(name: &str) -> serde_json::Value {
        json!({
            "schema_id": "http://fuchsia.com/schemas/sdk/product_bundle-6320eef1.json",
            "data": {
                "name": name,
                "type": "product_bundle",
                "device_refs": [name],
                "images": [{
                    "base_uri": "file://fuchsia/development/0.20201216.2.1/images/generic-x64.tgz",
                    "format": "tgz"
                }],
                "manifests": {
                },
                "packages": [{
                    "format": "tgz",
                    "repo_uri": "file://fuchsia/development/0.20201216.2.1/packages/generic-x64.tar.gz"
                }]
            }
        })
    }

    /// Macro to create a v1 product bundle in the tmp directory
    macro_rules! make_pb_v1_in {
        ($dir:expr,$name:expr) => {{
            let pb_dir = Utf8Path::from_path($dir.path()).unwrap();

            let pb_file = File::create(pb_dir.join("product_bundle.json")).unwrap();
            serde_json::to_writer(&pb_file, &make_sample_pbv1($name)).unwrap();

            pb_dir
        }};
    }

    fn make_sample_pbv2(name: &str, virtual_devices_path: Option<String>) -> serde_json::Value {
        json!({
            "version": "2",
            "product_name": name,
            "product_version": "fake.pb-version",
            "sdk_version": "fake.sdk-version",
            "partitions": {
                "hardware_revision": "board",
                "bootstrap_partitions": [],
                "bootloader_partitions": [],
                "partitions": [],
                "unlock_credentials": [],
            },
            "virtual_devices_path": virtual_devices_path,
        })
    }
    /// Macro to create a v1 product bundle in the tmp directory
    macro_rules! make_pb_v2_in {
        ($dir:expr,$name:expr) => {{
            let pb_dir = Utf8Path::from_path($dir.path()).unwrap();

            let pb_file = File::create(pb_dir.join("product_bundle.json")).unwrap();
            serde_json::to_writer(&pb_file, &make_sample_pbv2($name, Some("virtual_device_manifest.json".into()))).unwrap();

            let dev_manifest = pb_dir.join("virtual_device_manifest.json");
            fs::write(&dev_manifest,r#"
            {"recommended":"virtual_device_1","device_paths":{"virtual_device_1":"virtual_device_1.json","virtual_device_2":"virtual_device_2.json"}}
            "#).unwrap();

            fs::write(pb_dir.join("virtual_device_1.json"), VIRTUAL_DEVICE_VALID)
                .expect("writing device json");

            pb_dir
        }};
    }

    #[test]
    fn test_parse_v1() {
        let tmp = TempDir::new().unwrap();
        let pb_dir = make_pb_v1_in!(tmp, "generic-x64");
        assert!(LoadedProductBundle::try_load_from(pb_dir).is_err());
    }

    #[test]
    fn test_parse_v2() {
        let tmp = TempDir::new().unwrap();
        let pb_dir = Utf8Path::from_path(tmp.path()).unwrap();

        let pb_file = File::create(pb_dir.join("product_bundle.json")).unwrap();
        serde_json::to_writer(&pb_file, &make_sample_pbv2(&"fake.pb-name", None)).unwrap();
        let pb = LoadedProductBundle::try_load_from(pb_dir).unwrap();
        assert!(matches!(pb.deref(), &ProductBundle::V2 { .. }));
    }

    #[test]
    fn test_loaded_from_path() {
        let tmp = TempDir::new().unwrap();
        let pb_dir = make_pb_v2_in!(tmp, "generic-x64");
        let pb = LoadedProductBundle::try_load_from(pb_dir).unwrap();
        assert_eq!(pb_dir, pb.loaded_from_path());
    }

    #[test]
    fn test_loaded_product_bundle_into() {
        let tmp = TempDir::new().unwrap();
        let pb_dir = make_pb_v2_in!(tmp, "generic-x64");
        let pb: ProductBundle = LoadedProductBundle::try_load_from(pb_dir).unwrap().into();
        assert!(matches!(pb, ProductBundle::V2 { .. }));
    }

    #[test]
    fn test_loaded_from_product_bundle_deref() {
        let tmp = TempDir::new().unwrap();
        let pb_dir = make_pb_v2_in!(tmp, "generic-x64");
        let pb = LoadedProductBundle::try_load_from(pb_dir).unwrap();

        fn check_deref(_inner_pb: &ProductBundle) {
            // Just make sure we have a compile time check.
            assert!(true);
        }

        check_deref(&pb);
        assert!(matches!(*pb.deref(), ProductBundle::V2 { .. }));
    }

    #[test]
    fn test_zip_loaded() -> anyhow::Result<()> {
        let tmp = TempDir::new().unwrap();

        let pb = make_sample_pbv2("generic-x64", None);
        let pb_filename = tmp.path().join("pb.zip");
        let pb_file = File::create(pb_filename.clone())?;

        let mut zip = ZipWriter::new(pb_file);
        let options = FileOptions::default().compression_method(CompressionMethod::Stored);
        zip.start_file("product_bundle.json", options)?;
        let buf = serde_json::to_vec(&pb)?;
        let _ = zip.write(&buf)?;
        zip.flush()?;
        let _ = zip.finish()?;

        let _ = ZipLoadedProductBundle::try_load_from(Utf8Path::from_path(&pb_filename).unwrap())?;

        Ok(())
    }

    #[test]
    fn test_zip_product_bundle_into() -> anyhow::Result<()> {
        let tmp = TempDir::new().unwrap();
        let pb = make_sample_pbv2("generic-x64", None);
        let pb_filename = tmp.path().join("pb.zip");
        let pb_file = File::create(pb_filename.clone())?;

        let mut zip = ZipWriter::new(pb_file);
        let options = FileOptions::default().compression_method(CompressionMethod::Stored);
        zip.start_file("product_bundle.json", options)?;
        let buf = serde_json::to_vec(&pb)?;
        let _ = zip.write(&buf)?;
        zip.flush()?;
        let _ = zip.finish()?;
        let pb: ProductBundle =
            ZipLoadedProductBundle::try_load_from(Utf8Path::from_path(&pb_filename).unwrap())?
                .into();
        assert!(matches!(pb, ProductBundle::V2 { .. }));
        Ok(())
    }

    #[test]
    fn test_zip_from_product_bundle_deref() -> anyhow::Result<()> {
        let tmp = TempDir::new().unwrap();
        let pb = make_sample_pbv2("generic-x64", None);
        let pb_filename = tmp.path().join("pb.zip");
        let pb_file = File::create(pb_filename.clone())?;

        let mut zip = ZipWriter::new(pb_file);
        let options = FileOptions::default().compression_method(CompressionMethod::Stored);
        zip.start_file("product_bundle.json", options)?;
        let buf = serde_json::to_vec(&pb)?;
        let _ = zip.write(&buf)?;
        zip.flush()?;
        let _ = zip.finish()?;

        let pb = ZipLoadedProductBundle::try_load_from(Utf8Path::from_path(&pb_filename).unwrap())?;

        fn check_deref(_inner_pb: &ProductBundle) {
            // Just make sure we have a compile time check.
            assert!(true);
        }

        check_deref(&pb);
        assert!(matches!(*pb.deref(), ProductBundle::V2 { .. }));
        Ok(())
    }

    #[test]
    fn test_product_bundle_try_load_from_for_zip() -> anyhow::Result<()> {
        let tmp = TempDir::new().unwrap();

        let pb = make_sample_pbv2("generic-x64", None);
        let pb_filename = tmp.path().join("pb.zip");
        let pb_file = File::create(pb_filename.clone())?;

        let mut zip = ZipWriter::new(pb_file);
        let options = FileOptions::default().compression_method(CompressionMethod::Stored);
        zip.start_file("product_bundle.json", options)?;
        let buf = serde_json::to_vec(&pb)?;
        let _ = zip.write(&buf)?;
        zip.flush()?;
        let _ = zip.finish()?;

        // This should detect zip file and load from the zip
        let _ = ProductBundle::try_load_from(Utf8Path::from_path(&pb_filename).unwrap())?;

        Ok(())
    }

    #[test]
    fn test_no_file_fail_zip() -> anyhow::Result<()> {
        let tmp = TempDir::new().unwrap();

        let pb = make_sample_pbv2("generic-x64", None);
        let pb_filename = tmp.path().join("pb.zip");
        let pb_file = File::create(pb_filename.clone())?;

        let mut zip = ZipWriter::new(pb_file);
        let options = FileOptions::default().compression_method(CompressionMethod::Stored);
        zip.start_file("for_sure_not_a_product_bundle.json", options)?;
        let buf = serde_json::to_vec(&pb)?;
        let _ = zip.write(&buf)?;
        zip.flush()?;
        let _ = zip.finish()?;

        // This should detect zip file and load from the zip
        assert!(ProductBundle::try_load_from(Utf8Path::from_path(&pb_filename).unwrap()).is_err());

        Ok(())
    }

    #[test]
    fn test_product_bundle_try_load_from_for_zip_deep_path() -> anyhow::Result<()> {
        let tmp = TempDir::new().unwrap();

        let pb = make_sample_pbv2("generic-x64", None);
        let pb_filename = tmp.path().join("pb.zip");
        let pb_file = File::create(pb_filename.clone())?;

        let mut zip = ZipWriter::new(pb_file);
        let options = FileOptions::default().compression_method(CompressionMethod::Stored);
        // Now start the file deeper in the tree
        zip.start_file("foo/bar/baz/biz/product_bundle.json", options)?;
        let buf = serde_json::to_vec(&pb)?;
        let _ = zip.write(&buf)?;
        zip.flush()?;
        let _ = zip.finish()?;

        // This should detect zip file and load from the zip
        let _ = ProductBundle::try_load_from(Utf8Path::from_path(&pb_filename).unwrap())?;

        Ok(())
    }

    #[test]
    fn test_parse_v1_from_zip_fails() -> anyhow::Result<()> {
        let tmp = TempDir::new().unwrap();
        let pb = make_sample_pbv1("generic-x64");
        let pb_filename = tmp.path().join("pb.zip");
        let pb_file = File::create(pb_filename.clone())?;

        let mut zip = ZipWriter::new(pb_file);
        let options = FileOptions::default().compression_method(CompressionMethod::Stored);
        zip.start_file("for_sure_not_a_product_bundle.json", options)?;
        let buf = serde_json::to_vec(&pb)?;
        let _ = zip.write(&buf)?;
        zip.flush()?;
        let _ = zip.finish()?;

        // This should fail as pbv1 is no longer supported.
        assert!(ProductBundle::try_load_from(Utf8Path::from_path(&pb_filename).unwrap()).is_err());
        Ok(())
    }

    #[test]
    fn test_product_bundle_try_load_from_for_dir() -> anyhow::Result<()> {
        let tmp = TempDir::new().unwrap();
        let pb_dir = make_pb_v2_in!(tmp, "generic-x64");
        let _ = ProductBundle::try_load_from(pb_dir).unwrap();
        Ok(())
    }

    #[test]
    fn test_get_device_from_path_absolute() -> Result<()> {
        let temp_dir = tempfile::TempDir::new().expect("creating temp dir");
        let pb_dir = make_pb_v2_in!(temp_dir, "generic-x64");

        let VirtualDevice::V1(expected) =
            VirtualDevice::try_load_from(pb_dir.join("virtual_device_1.json")).unwrap();

        let absolute_path = pb_dir.join("virtual_device_1.json");
        let pb = LoadedProductBundle::try_load_from(pb_dir).unwrap();
        let actual = pb.get_device(&Some(absolute_path.into_string()));

        assert!(actual.is_ok());
        assert_eq!(expected, actual?);
        Ok(())
    }

    #[test]
    fn test_get_device_from_path_relative() -> Result<()> {
        let temp_dir = tempfile::TempDir::new().expect("creating temp dir");
        let pb_dir = make_pb_v2_in!(temp_dir, "generic-x64");

        let VirtualDevice::V1(expected) =
            VirtualDevice::try_load_from(pb_dir.join("virtual_device_1.json")).unwrap();

        let absolute_path = pb_dir.join("virtual_device_1.json");
        let relative_path =
            pathdiff::diff_paths(absolute_path, std::env::current_dir().unwrap()).unwrap();
        let pb = LoadedProductBundle::try_load_from(pb_dir).unwrap();
        let actual = pb.get_device(&Some(relative_path.to_str().unwrap().to_string()));

        assert!(actual.is_ok());
        assert_eq!(expected, actual?);
        Ok(())
    }

    #[test]
    fn test_get_device_from_name() -> Result<()> {
        let temp_dir = tempfile::TempDir::new().expect("creating temp dir");
        let pb_dir = make_pb_v2_in!(temp_dir, "generic-x64");

        let VirtualDevice::V1(expected) =
            VirtualDevice::try_load_from(pb_dir.join("virtual_device_1.json")).unwrap();

        let pb = LoadedProductBundle::try_load_from(pb_dir).unwrap();
        let actual = pb.get_device(&Some("virtual_device_1".into()));

        assert!(actual.is_ok());
        assert_eq!(expected, actual?);
        Ok(())
    }

    #[test]
    fn test_get_device_from_name_nondefault_device() -> Result<()> {
        let temp_dir = tempfile::TempDir::new().expect("creating temp dir");
        let pb_dir = make_pb_v2_in!(temp_dir, "generic-x64");
        fs::rename(pb_dir.join("virtual_device_1.json"), pb_dir.join("virtual_device_2.json"))
            .expect("remove default virtual device, create non-default virtual device");

        let VirtualDevice::V1(expected) =
            VirtualDevice::try_load_from(pb_dir.join("virtual_device_2.json")).unwrap();

        let pb = LoadedProductBundle::try_load_from(pb_dir).unwrap();
        let actual = pb.get_device(&Some("virtual_device_2".into()));

        assert!(actual.is_ok());
        assert_eq!(expected, actual?);
        Ok(())
    }

    #[test]
    fn test_get_device_default_device() -> Result<()> {
        let temp_dir = tempfile::TempDir::new().expect("creating temp dir");
        let pb_dir = make_pb_v2_in!(temp_dir, "generic-x64");

        let VirtualDevice::V1(expected) =
            VirtualDevice::try_load_from(pb_dir.join("virtual_device_1.json")).unwrap();

        let pb = LoadedProductBundle::try_load_from(pb_dir).unwrap();
        let actual = pb.get_device(&None);

        assert!(actual.is_ok());
        assert_eq!(expected, actual?);
        Ok(())
    }

    #[test]
    fn test_get_device_from_invalid_name() -> Result<()> {
        let temp_dir = tempfile::TempDir::new().expect("creating temp dir");
        let pb_dir = make_pb_v2_in!(temp_dir, "generic-x64");

        let pb = LoadedProductBundle::try_load_from(pb_dir).unwrap();
        let actual = pb.get_device(&Some("invalid_device".into()));

        assert!(actual.is_err());
        Ok(())
    }

    #[test]
    fn test_parse_pb_17_20240101_0_1() {
        let pb_json = include_str!("../test_data/17.20240101.0.1/product_bundle.json");
        let pb = try_load_product_bundle(pb_json.as_bytes()).unwrap();
        assert!(matches!(pb, ProductBundle::V2 { .. }));
    }

    #[test]
    fn test_parse_pb_19_20240401_0_1() {
        let pb_json = include_str!("../test_data/19.20240401.0.1/product_bundle.json");
        let pb = try_load_product_bundle(pb_json.as_bytes()).unwrap();
        assert!(matches!(pb, ProductBundle::V2 { .. }));
    }

    #[test]
    fn test_parse_pb_22_20240701_0_1() {
        let pb_json = include_str!("../test_data/22.20240701.0.1/product_bundle.json");
        let pb = try_load_product_bundle(pb_json.as_bytes()).unwrap();
        assert!(matches!(pb, ProductBundle::V2 { .. }));
    }

    #[test]
    fn test_parse_pb_27_20250401_0_1() {
        let pb_json = include_str!("../test_data/27.20250401.0.1/product_bundle.json");
        let pb = try_load_product_bundle(pb_json.as_bytes()).unwrap();
        assert!(matches!(pb, ProductBundle::V2 { .. }));
    }

    #[test]
    fn test_parse_pb_29_20251001_0_1() {
        let pb_json = include_str!("../test_data/29.20251001.0.1/product_bundle.json");
        let pb = try_load_product_bundle(pb_json.as_bytes()).unwrap();
        assert!(matches!(pb, ProductBundle::V2 { .. }));
    }

    #[test]
    fn test_parse_pb_31_20260301_0_1() {
        let pb_json = include_str!("../test_data/31.20260301.0.1/product_bundle.json");
        let pb = try_load_product_bundle(pb_json.as_bytes()).unwrap();
        assert!(matches!(pb, ProductBundle::V2 { .. }));
    }

    #[test]
    fn test_parse_pb_32_99991231_0_1() {
        let pb_json = include_str!("../test_data/32.99991231.0.1/product_bundle.json");
        let pb = try_load_product_bundle(pb_json.as_bytes()).unwrap();
        assert!(matches!(pb, ProductBundle::V2 { .. }));
    }

    #[test]
    fn test_supports_extract_blobs() -> Result<()> {
        let temp_dir = tempfile::TempDir::new().expect("creating temp dir");
        let pb_dir = make_pb_v2_in!(temp_dir, "generic-x64");

        let pb = LoadedProductBundle::try_load_from(pb_dir).unwrap();
        // Default should be false as we don't have tools.
        assert!(!pb.supports_extract_blobs(Slot::A));

        // Now add a tool.
        let pb_file = File::create(pb_dir.join("product_bundle.json")).unwrap();
        let mut pb_json = make_sample_pbv2("generic-x64", None);
        pb_json["platform_tools_a"] = serde_json::json!(["path/to/fxfs_pbtool"]);
        serde_json::to_writer(&pb_file, &pb_json).unwrap();

        let pb = LoadedProductBundle::try_load_from(pb_dir).unwrap();
        assert!(pb.supports_extract_blobs(Slot::A));
        assert!(!pb.supports_extract_blobs(Slot::B));

        Ok(())
    }

    #[fuchsia::test]
    async fn test_extract_blobs_with_image() {
        let tmp = TempDir::new().unwrap();
        let out_dir = tmp.path().join("out");
        fs::create_dir(&out_dir).unwrap();

        let artifacts_dir = Utf8PathBuf::from(env!("PLATFORM_ARTIFACTS_DIR"));
        let tools = PlatformToolProvider::new(artifacts_dir.clone());

        // Create temporary directories for generated artifacts.
        let package_dir = tmp.path().join("package");
        fs::create_dir(&package_dir).unwrap();
        let partitions_dir = Utf8PathBuf::from_path_buf(tmp.path().join("partitions")).unwrap();
        fs::create_dir(&partitions_dir).unwrap();
        let image_assembly_config_dir = tmp.path().join("image_assembly_config");
        let image_assembly_config_dir_utf8 =
            Utf8PathBuf::from_path_buf(image_assembly_config_dir.clone()).unwrap();
        fs::create_dir(&image_assembly_config_dir).unwrap();
        let assembled_system_dir = tmp.path().join("assembled_system");
        let assembled_system_dir_utf8 =
            Utf8PathBuf::from_path_buf(assembled_system_dir.clone()).unwrap();
        fs::create_dir(&assembled_system_dir).unwrap();

        // Create a mock package manifest for a base package to trigger Fxfs generation.
        let package_manifest_path =
            Utf8PathBuf::from_path_buf(package_dir.join("my_pkg_manifest.json")).unwrap();
        let meta_far_path = package_dir.join("meta.far");
        let mut package_builder =
            fuchsia_pkg::PackageBuilder::new_platform_internal_package("my_pkg");
        let blob_data = b"blob contents";
        package_builder.add_contents_as_blob("data/file", blob_data, &package_dir).unwrap();
        package_builder.manifest_path(package_manifest_path.clone());
        package_builder.build(&package_dir, &meta_far_path).unwrap();

        // Create a partitions config.
        let mut partitions = assembly_partitions_config::PartitionsConfig::default();
        partitions.hardware_revision = "test".into();
        partitions.write_to_dir(&partitions_dir, None::<Utf8PathBuf>).unwrap();

        let mut builder = ImageAssemblyConfigBuilder::new(
            assembly_config_schema::platform_settings::BuildType::Eng,
            assembly_config_schema::FeatureSetLevel::Standard,
            "test".into(),
            None,
            assembly_images_config::FilesystemImageMode::Partition,
            AssemblyMode::BuildEverything,
            assembly_release_info::SystemReleaseInfo::new_for_testing(),
        );
        builder
            .add_package_from_path(
                package_manifest_path,
                image_assembly_config_builder::PackageOrigin::Product,
                &assembly_config_schema::PackageSet::Base,
            )
            .unwrap();
        builder
            .set_images_config(assembly_images_config::ImagesConfig {
                images: vec![
                    assembly_images_config::Image::Zbi(Default::default()),
                    assembly_images_config::Image::Fxfs(Default::default()),
                ],
            })
            .unwrap();

        builder.add_bundle(artifacts_dir.join("zircon_eng/assembly_config.json")).unwrap();
        builder.add_bundle(artifacts_dir.join("emulator_support/assembly_config.json")).unwrap();
        let (mut image_assembly_config, _) = builder
            .build_and_validate(&image_assembly_config_dir_utf8, &tools, ValidationMode::Off)
            .unwrap();
        image_assembly_config.partitions_config = Some(partitions_dir);

        let system = AssembledSystem::new(
            image_assembly_config,
            false,
            &assembled_system_dir_utf8,
            &tools,
            None,
            AssemblyMode::BuildEverything,
        )
        .await
        .unwrap();

        // Create a product bundle.
        let pb = crate::ProductBundleBuilder::new("name")
            .system(system, assembly_partitions_config::Slot::A)
            .build(Box::new(tools), Utf8Path::from_path(&out_dir).unwrap())
            .await
            .unwrap();

        // Extract blobs and verify that the blob from the base package is present.
        let extracted_dir = Utf8PathBuf::from_path_buf(tmp.path().join("extracted")).unwrap();
        pb.extract_blobs(assembly_partitions_config::Slot::A, &extracted_dir, None)
            .expect("extract_blobs failed");
        let reader = fs::read_dir(&extracted_dir).unwrap();
        let mut found = false;
        for entry in reader {
            let entry = entry.unwrap();
            let contents = fs::read(entry.path()).unwrap();
            if contents == blob_data {
                found = true;
                break;
            }
        }
        assert!(found, "Did not find extracted blob with matching content");
    }

    #[test]
    fn test_extract_blobs_without_fxfs_image() {
        let tmp = TempDir::new().unwrap();
        let product_bundle_dir = tmp.path().join("out");

        let image_path = Utf8PathBuf::from_path_buf(tmp.path().join("zbi")).unwrap();
        let tool_path = tmp.path().join("fxfs_pbtool");

        let mut config = assembly_partitions_config::PartitionsConfig::default();
        config.hardware_revision = "test".into();
        let pb = ProductBundle::V2(ProductBundleV2 {
            product_name: "test".into(),
            product_version: "test".into(),
            partitions: config,
            sdk_version: "test".into(),
            system_a: Some(vec![Image::ZBI { path: image_path, signed: false }]),
            platform_tools_a: vec![Utf8PathBuf::from_path_buf(tool_path).unwrap()],
            system_b: None,
            platform_tools_b: vec![],
            system_r: None,
            platform_tools_r: vec![],
            repositories: vec![],
            update_package_hash: None,
            virtual_devices_path: None,
            release_info: None,
        });

        let result = pb.extract_blobs(
            assembly_partitions_config::Slot::A,
            Utf8Path::from_path(&product_bundle_dir).unwrap(),
            None,
        );

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().to_string(), "System does not contain an fxfs image");
    }

    #[test]
    fn test_extract_blobs_without_system() {
        let tmp = TempDir::new().unwrap();
        let product_bundle_dir = tmp.path().join("out");

        let mut config = assembly_partitions_config::PartitionsConfig::default();
        config.hardware_revision = "test".into();
        let pb = ProductBundle::V2(ProductBundleV2 {
            product_name: "test".into(),
            product_version: "test".into(),
            partitions: config,
            sdk_version: "test".into(),
            system_a: None,
            platform_tools_a: vec![],
            system_b: None,
            platform_tools_b: vec![],
            system_r: None,
            platform_tools_r: vec![],
            repositories: vec![],
            update_package_hash: None,
            virtual_devices_path: None,
            release_info: None,
        });

        let result = pb.extract_blobs(
            assembly_partitions_config::Slot::A,
            Utf8Path::from_path(&product_bundle_dir).unwrap(),
            None,
        );

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().to_string(), "System does not exist for the specified slot");
    }
}
