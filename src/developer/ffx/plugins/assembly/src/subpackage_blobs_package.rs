// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use assembled_system::{AssembledSystem, PackagesMetadata};
use assembly_subpackage_blobs_package::SubpackageBlobsPackageBuilder;
use camino::Utf8Path;
use fuchsia_pkg::PackageManifest;
use log::info;
use std::fs::File;

#[derive(Debug)]
pub struct SubpackageBlobsPackage {
    pub manifest: PackageManifest,
}

pub fn construct_subpackage_blobs_package(
    assembled_system: &AssembledSystem,
    outdir: impl AsRef<Utf8Path>,
    gendir: impl AsRef<Utf8Path>,
    name: impl AsRef<str>,
) -> Result<SubpackageBlobsPackage> {
    let outdir = outdir.as_ref().join("subpackage_blobs");
    if !outdir.exists() {
        std::fs::create_dir_all(&outdir)?;
    }

    let mut subpackage_blobs_pkg_builder = SubpackageBlobsPackageBuilder::default();

    for image in &assembled_system.images {
        if let Some(contents) = image.get_blobfs_contents() {
            let PackagesMetadata { base, cache } = &contents.packages;

            for package in base.metadata.iter().chain(cache.metadata.iter()) {
                let manifest = PackageManifest::try_load_from(&package.manifest)?;
                subpackage_blobs_pkg_builder.add_subpackages_from_package(manifest)?;
            }
        }
    }

    let subpackage_blobs_package_path = outdir.join("meta.far");
    let build_results = subpackage_blobs_pkg_builder
        .build(&outdir, gendir, name, &subpackage_blobs_package_path)
        .context("Failed to build the subpackage blobs package")?;

    let subpackage_blobs_package = File::open(&subpackage_blobs_package_path)
        .context("Failed to open the subpackage blobs package")?;
    let subpackage_blobs_merkle = fuchsia_merkle::root_from_reader(subpackage_blobs_package)
        .context("Failed to calculate the subpackage blobs merkle")?;
    info!("SubpackageBlobs merkle: {}", subpackage_blobs_merkle);

    Ok(SubpackageBlobsPackage { manifest: build_results.manifest })
}

#[cfg(test)]
mod tests {
    use super::*;

    use assembled_system::{BlobfsContents, Image};
    use assembly_release_info::SystemReleaseInfo;
    use camino::Utf8PathBuf;
    use fuchsia_archive::Utf8Reader;
    use fuchsia_pkg::PackageBuilder;
    use pretty_assertions::assert_eq;
    use std::collections::{BTreeSet, HashMap};
    use tempfile::tempdir;
    use utf8_path::path_relative_from_current_dir;

    fn create_package(
        root: &Utf8Path,
        name: &str,
        subpackages: &[(&PackageManifest, &Utf8Path)],
    ) -> (PackageManifest, Utf8PathBuf) {
        let dir = root.join(name);

        // Hardcode the ABI so it doesn't change when the ABI revision is bumped.
        let mut builder = PackageBuilder::new(name, 0x57904F5A17FA3B22.into());

        let blob_name = format!("{}-blob", name);
        builder.add_contents_as_blob(&blob_name, &blob_name, &dir).unwrap();

        let meta_name = format!("meta/{}-meta", name);
        builder.add_contents_to_far(&meta_name, &meta_name, &dir).unwrap();

        for (subpackage_manifest, subpackage_path) in subpackages {
            builder
                .add_subpackage(
                    &subpackage_manifest.name().clone().into(),
                    subpackage_manifest.hash(),
                    subpackage_path.into(),
                )
                .unwrap();
        }

        let manifest = builder.build(&dir, dir.join("meta.far")).unwrap();

        let manifest_path = dir.join("package_manifest.json");
        serde_json::to_writer(File::create(&manifest_path).unwrap(), &manifest).unwrap();

        (manifest, manifest_path)
    }

    #[test]
    fn construct() {
        let tmp = tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        // Prepare package manifests to add to the subpackage blobs package.
        let (base_child_manifest, base_child_path) = create_package(dir, "base_child", &[]);
        let (base_parent_manifest, base_parent_path) =
            create_package(dir, "base_parent", &[(&base_child_manifest, &base_child_path)]);

        let (cache_child_manifest, cache_child_path) = create_package(dir, "cache_child", &[]);
        let (cache_parent_manifest, cache_parent_path) =
            create_package(dir, "cache_parent", &[(&cache_child_manifest, &cache_child_path)]);

        let merkle_size_map = base_child_manifest
            .blobs()
            .iter()
            .chain(base_parent_manifest.blobs().iter())
            .chain(cache_child_manifest.blobs().iter())
            .chain(cache_parent_manifest.blobs().iter())
            .map(|blob| (blob.merkle.to_string(), blob.size))
            .collect::<HashMap<_, _>>();

        let mut blobfs_contents = BlobfsContents::default();
        blobfs_contents.add_base_package(&base_parent_path, &merkle_size_map).unwrap();
        blobfs_contents.add_cache_package(&cache_parent_path, &merkle_size_map).unwrap();

        let assembled_system = AssembledSystem {
            images: vec![Image::BlobFS {
                path: "does-not-exist".into(),
                contents: blobfs_contents,
            }],
            board_name: "my_board".into(),
            partitions_config: None,
            system_release_info: SystemReleaseInfo::new_for_testing(),
            platform_tools: vec![],
        };

        // Construct the subpackage blobs package.
        construct_subpackage_blobs_package(&assembled_system, dir, dir, "subpackage_blobs")
            .unwrap();

        let subpackage_blobs_package_path =
            path_relative_from_current_dir(dir.join("subpackage_blobs/meta.far")).unwrap();

        // Collect all the expected subpackage merkles from the subpackages.
        let expected_merkles = base_child_manifest
            .blobs()
            .iter()
            .chain(cache_child_manifest.blobs().iter())
            .map(|blob| blob.merkle.to_string())
            .collect::<BTreeSet<_>>();

        // Read the base package, and assert the contents are correct.
        let subpackage_blobs_package_file = File::open(subpackage_blobs_package_path).unwrap();
        let mut far_reader = Utf8Reader::new(&subpackage_blobs_package_file).unwrap();
        let contents = far_reader.read_file("meta/contents").unwrap();
        let contents = std::str::from_utf8(&contents).unwrap();

        let mut expected_contents = String::new();
        for merkle in expected_merkles {
            expected_contents.push_str(&format!("{}={}\n", merkle, merkle));
        }
        assert_eq!(contents, expected_contents);
    }
}
