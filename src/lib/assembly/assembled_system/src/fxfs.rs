// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::BlobfsContents;
use crate::base_package::BasePackage;

use anyhow::{Context, Result};
use assembly_fxfs::{CompressionAlgorithm, FxfsBuilder};
use assembly_images_config::{Fxfs, FxfsBlobFormat};
use camino::{Utf8Path, Utf8PathBuf};
use image_assembly_config::ImageAssemblyConfig;
use std::collections::HashMap;

pub struct ConstructedFxfs {
    /// The path to the raw Fxfs image, which can be mounted and used directly.
    pub image_path: Utf8PathBuf,
    /// The path to the Fxfs image in the Android Sparse format, which can be used for flashing and
    /// paving.
    pub sparse_image_path: Utf8PathBuf,
    /// A description of all blob contents in the Fxfs images.
    pub contents: BlobfsContents,
}

/// Constructs an Fxfs image containing all requested base packages.
pub async fn construct_fxfs(
    gendir: impl AsRef<Utf8Path>,
    image_config: &ImageAssemblyConfig,
    base_package: &BasePackage,
    fxfs_config: &Fxfs,
) -> Result<ConstructedFxfs> {
    let mut contents = BlobfsContents {
        maximum_contents_size: fxfs_config.maximum_contents_size,
        ..Default::default()
    };
    let blob_format = match fxfs_config.blob_format {
        FxfsBlobFormat::Uncompressed => None,
        FxfsBlobFormat::Zstd => Some(CompressionAlgorithm::Zstd),
        FxfsBlobFormat::Lz4 => Some(CompressionAlgorithm::Lz4),
    };
    let mut fxfs_builder = FxfsBuilder::new(blob_format);
    if let Some(size) = fxfs_config.size_bytes {
        fxfs_builder.set_size(size);
    }

    // Add the base and cache packages.
    for package_manifest_path in &image_config.base {
        fxfs_builder.add_package_from_path(package_manifest_path)?;
    }
    for package_manifest_path in &image_config.cache {
        fxfs_builder.add_package_from_path(package_manifest_path)?;
    }

    // Add the base package and its contents.
    fxfs_builder.add_package_from_path(&base_package.manifest_path)?;

    // Build the fxfs and store the merkle to size map.
    let image_path = gendir.as_ref().join("fxfs.blk");
    let sparse_image_path = gendir.as_ref().join("fxfs.sparse.blk");
    let blobs_json_path = fxfs_builder
        .build(gendir, &image_path, Some(&sparse_image_path))
        .await
        .context("Failed to build the Fxfs image")?;
    let merkle_size_map = assembly_fxfs::read_blobs_json(blobs_json_path)
        .map(|blobs_json| {
            blobs_json
                .iter()
                .map(|e| (e.merkle.to_string(), e.used_space_in_blobfs))
                .collect::<HashMap<String, u64>>()
        })
        .context("Failed to parse blobs JSON")?;
    for package_manifest_path in &image_config.base {
        contents.add_base_package(package_manifest_path, &merkle_size_map)?;
    }
    for package_manifest_path in &image_config.cache {
        contents.add_cache_package(package_manifest_path, &merkle_size_map)?;
    }
    contents.add_base_package(&base_package.manifest_path, &merkle_size_map)?;
    Ok(ConstructedFxfs { image_path, sparse_image_path, contents })
}

#[cfg(test)]
mod tests {
    use super::{ConstructedFxfs, construct_fxfs};

    use crate::AssembledSystem;
    use crate::base_package::construct_base_package;

    use assembly_images_config::Fxfs;
    use assembly_release_info::SystemReleaseInfo;
    use camino::{Utf8Path, Utf8PathBuf};
    use image_assembly_config::ImageAssemblyConfig;
    use serde_json::json;
    use std::fs::File;
    use std::io::Write;
    use tempfile::tempdir;

    // Generates a package manifest to be used for testing. The file is written
    // into `dir`, and the location is returned. The `name` is used in the blob
    // file names to make each manifest somewhat unique.
    // TODO(https://fxbug.dev/42156958): See if we can share this with BasePackage.
    pub fn generate_test_manifest_file(
        dir: impl AsRef<Utf8Path>,
        name: impl AsRef<str>,
    ) -> Utf8PathBuf {
        // Create a data file for the package.
        let data_file_name = format!("{}_data.txt", name.as_ref());
        let data_path = dir.as_ref().join(&data_file_name);
        let data_file = File::create(&data_path).unwrap();
        write!(&data_file, "bleh").unwrap();

        // Create the manifest.
        let manifest_path = dir.as_ref().join(format!("{}.json", name.as_ref()));
        let manifest_file = File::create(&manifest_path).unwrap();
        serde_json::to_writer(
            &manifest_file,
            &json!({
                    "version": "1",
                    "repository": "testrepository.com",
                    "package": {
                        "name": name.as_ref(),
                        "version": "1",
                    },
                    "blobs": [
                        {
                            "source_path": format!("path/to/{}/meta.far", name.as_ref()),
                            "path": "meta/",
                            "merkle":
                                "0000000000000000000000000000000000000000000000000000000000000000",
                            "size": 1
                        },
                        {
                            "source_path": &data_path,
                            "path": &data_file_name,
                            "merkle":
                                "1111111111111111111111111111111111111111111111111111111111111111",
                            "size": 1
                        },
                    ],
                }
            ),
        )
        .unwrap();
        manifest_path
    }

    #[fuchsia_async::run_singlethreaded(test)]
    async fn construct() {
        let tmp = tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        let image_config = ImageAssemblyConfig::new_for_testing("kernel");

        // Create a fake base package.
        let system_manifest = generate_test_manifest_file(dir, "extra_base");
        let base_manifest = generate_test_manifest_file(dir, "test_static");
        let cache_manifest = generate_test_manifest_file(dir, "test_cache");
        let mut product_config = ImageAssemblyConfig::new_for_testing("kernel");
        product_config.system.push(system_manifest);
        product_config.base.push(base_manifest);
        product_config.cache.push(cache_manifest);

        // Construct the base package.
        let mut assembled_system = AssembledSystem {
            images: Default::default(),
            board_name: "my_board".into(),
            partitions_config: None,
            system_release_info: SystemReleaseInfo::new_for_testing(),
            platform_tools: vec![],
        };
        let base_package =
            construct_base_package(&mut assembled_system, dir, "system_image", &product_config)
                .unwrap();

        let size_byteses = vec![None, Some(32 * 1024 * 1024)];
        for size_bytes in size_byteses {
            let ConstructedFxfs { image_path, sparse_image_path, contents: blobs } =
                construct_fxfs(
                    dir,
                    &image_config,
                    &base_package,
                    &Fxfs { size_bytes, ..Default::default() },
                )
                .await
                .unwrap();

            // Ensure something was created.
            assert!(image_path.exists());
            assert!(sparse_image_path.exists());
            if let Some(size) = size_bytes {
                assert_eq!(size, std::fs::metadata(image_path).unwrap().len());
            }

            // Ensure the blobs match expectations.
            let blobs = blobs.relativize(dir).unwrap();
            assert!(!blobs.packages.base.metadata.is_empty());
            assert!(blobs.packages.cache.metadata.is_empty());
        }
    }
}
