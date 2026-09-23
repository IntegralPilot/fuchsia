// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::errors::AnchoredPackagesError;
use crate::{
    AnchoredPackages, CachePackages, CachePackagesInitError, StaticPackages,
    StaticPackagesInitError, get_system_image_hash,
};
use anyhow::Context as _;
use fuchsia_hash::Hash;
use package_directory::RootDir;
use std::sync::Arc;

static DISABLE_RESTRICTIONS_FILE_PATH: &str = "data/pkgfs_disable_executability_restrictions";

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ExecutabilityRestrictions {
    Enforce,
    DoNotEnforce,
}

/// System image package.
pub struct SystemImage {
    root_dir: Arc<RootDir<blobfs::Client>>,
}

impl SystemImage {
    pub async fn new(
        blobfs: blobfs::Client,
        system_image_hash: &str,
    ) -> Result<Self, anyhow::Error> {
        let hash = get_system_image_hash(system_image_hash).context("getting system_image hash")?;
        let root_dir = RootDir::new(blobfs, hash)
            .await
            .with_context(|| format!("creating RootDir for system_image: {hash}"))?;
        Ok(SystemImage { root_dir })
    }

    /// Make a `SystemImage` from a `RootDir` for the `system_image` package.
    pub fn from_root_dir(root_dir: Arc<RootDir<blobfs::Client>>) -> Self {
        Self { root_dir }
    }

    pub fn load_executability_restrictions(&self) -> ExecutabilityRestrictions {
        match self.root_dir.has_file(DISABLE_RESTRICTIONS_FILE_PATH) {
            true => ExecutabilityRestrictions::DoNotEnforce,
            false => ExecutabilityRestrictions::Enforce,
        }
    }

    /// The hash of the `system_image` package.
    pub fn hash(&self) -> &Hash {
        self.root_dir.hash()
    }

    /// Load `data/cache_packages.json`.
    pub async fn cache_packages(&self) -> Result<CachePackages, CachePackagesInitError> {
        self.root_dir
            .read_file("data/cache_packages.json")
            .await
            .map_err(CachePackagesInitError::ReadCachePackagesJson)
            .and_then(|content| CachePackages::from_json(content.as_slice()))
    }

    /// Load `data/static_packages`.
    pub async fn static_packages(&self) -> Result<StaticPackages, StaticPackagesInitError> {
        StaticPackages::deserialize(
            self.root_dir
                .read_file("data/static_packages")
                .await
                .map_err(StaticPackagesInitError::ReadStaticPackages)?
                .as_slice(),
        )
        .map_err(StaticPackagesInitError::ProcessingStaticPackages)
    }

    /// Load `data/anchored_packages.json`.
    pub async fn anchored_packages(&self) -> Result<AnchoredPackages, AnchoredPackagesError> {
        self.root_dir
            .read_file("data/anchored_packages.json")
            .await
            .map_err(AnchoredPackagesError::ReadAnchoredPackagesJson)
            .and_then(|content| AnchoredPackages::from_json(content.as_slice()))
    }

    /// Consume self and return the contained `package_directory::RootDir`.
    pub fn into_root_dir(self) -> Arc<RootDir<blobfs::Client>> {
        self.root_dir
    }

    /// The package path of the system image package.
    pub fn package_path() -> fuchsia_pkg::PackagePath {
        fuchsia_pkg::PackagePath::from_name_and_variant(
            "system_image".parse().expect("valid package name"),
            fuchsia_pkg::PackageVariant::zero(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use blobfs_ramdisk::BlobfsRamdisk;
    use fuchsia_hash::GenericDigest;
    use fuchsia_pkg::PackagePath;
    use fuchsia_pkg::package_sets::AnchoredPackageSetType;
    use fuchsia_pkg_testing::{Package, PackageBuilder, SystemImageBuilder};
    use package_directory::NonMetaStorage;
    use std::collections::HashSet;
    use std::str::FromStr;

    struct TestEnv {
        _blobfs: blobfs_ramdisk::BlobfsRamdisk,
    }

    impl TestEnv {
        async fn new(system_image: SystemImageBuilder) -> (Self, SystemImage) {
            let blobfs = blobfs_ramdisk::BlobfsRamdisk::start().await.unwrap();
            let system_image = system_image.build().await;
            system_image.write_to_blobfs(&blobfs).await;
            let root_dir = RootDir::new(blobfs.client(), *system_image.hash()).await.unwrap();
            (Self { _blobfs: blobfs }, SystemImage { root_dir })
        }
    }

    #[fuchsia::test]
    async fn cache_packages_fails_without_config_files() {
        let (_env, system_image) = TestEnv::new(SystemImageBuilder::new()).await;
        assert_matches!(
            system_image.cache_packages().await,
            Err(CachePackagesInitError::ReadCachePackagesJson(
                package_directory::ReadFileError::NoFileAtPath { .. }
            ))
        );
    }

    #[fuchsia::test]
    async fn cache_packages_deserialize_valid_line_oriented() {
        let (_env, system_image) = TestEnv::new(
            SystemImageBuilder::new()
                .cache_package("name/variant".parse().unwrap(), [0; 32].into()),
        )
        .await;

        assert_eq!(
            system_image.cache_packages().await.unwrap(),
            CachePackages::from_entries(
                vec!["fuchsia-pkg://fuchsia.com/name/variant?hash=0000000000000000000000000000000000000000000000000000000000000000"
                    .parse()
                    .unwrap()
                ]
            )
        );
    }

    #[fuchsia::test]
    async fn static_packages_deserialize_valid_line_oriented() {
        let (_env, system_image) = TestEnv::new(
            SystemImageBuilder::new()
                .static_package("name/variant".parse().unwrap(), [0; 32].into()),
        )
        .await;

        assert_eq!(
            system_image.static_packages().await.unwrap(),
            StaticPackages::from([("name/variant".parse().unwrap(), [0; 32].into())])
        );
    }

    #[fuchsia::test]
    async fn anchored_packages_fails_without_config_files() {
        let (_env, system_image) = TestEnv::new(SystemImageBuilder::new()).await;
        assert_matches!(
            system_image.anchored_packages().await,
            Err(AnchoredPackagesError::ReadAnchoredPackagesJson(
                package_directory::ReadFileError::NoFileAtPath { .. }
            ))
        );
    }

    #[fuchsia::test]
    async fn anchored_packages_deserialize_valid() {
        let (_env, system_image) = TestEnv::new(SystemImageBuilder::new().anchored_package(
            AnchoredPackageSetType::OnDemand,
            "name/variant".parse().unwrap(),
            [0; 32].into(),
        ))
        .await;
        let json = r#" {
            "anchored_on_demand": {
                "fuchsia-pkg://fuchsia.com/name/variant": {
                    "hash": "0000000000000000000000000000000000000000000000000000000000000000"
                }
            }
        }"#;
        assert_eq!(
            system_image.anchored_packages().await.unwrap(),
            AnchoredPackages::from_json(json.as_bytes()).unwrap()
        );
    }

    /// Constructs a test package to be used in the system image builder tests below.
    async fn make_test_package(name: &str) -> Package {
        // This will generate deterministic package hashes and blobs which the test functions
        // are using to verify that write/read roundtrip of the constructed system image works.
        let builder = PackageBuilder::new(name).add_resource_at("pkgname", name.as_bytes());
        builder.build().await.expect("build package")
    }

    #[fuchsia::test]
    async fn static_package_in_system_image_read_write_roundtrip() {
        let static_test_package = make_test_package("test-package").await;
        let system_image_package =
            SystemImageBuilder::new().static_packages(&[&static_test_package]).build().await;
        let blobfs = BlobfsRamdisk::builder().start().await.expect("started blobfs");
        static_test_package.write_to_blobfs(&blobfs).await;
        system_image_package.write_to_blobfs(&blobfs).await;

        let client = blobfs.client();

        let blobs_got = client.list_known_blobs().await.unwrap();

        let mut blobs_expected = HashSet::new();
        for digest in [
            "055dc718192ef18007ac4268e223224232f29bd4840eaac713da3e930bdc9c47",
            "2462c914da92ac7c87de422cf072049fb58442434732dfd5f39ee6de6011d2c3",
            "81c6073abd7938367b78cf50eff3cfefeb733902951efc89312cae03d8ac16a7",
            "b6f3dc74baf53aa20a439925117811fbd54ba2641ffbd9e10838ac8bb5dba1d8",
        ] {
            blobs_expected.insert(GenericDigest::from_str(digest).unwrap());
        }

        assert_eq!(blobs_got, blobs_expected);
        assert_eq!(
            "test-package",
            String::from_utf8(
                client
                    .read_blob(
                        &GenericDigest::from_str(
                            "b6f3dc74baf53aa20a439925117811fbd54ba2641ffbd9e10838ac8bb5dba1d8"
                        )
                        .unwrap()
                    )
                    .await
                    .expect("reading blob")
            )
            .unwrap()
        );
    }

    #[fuchsia::test]
    async fn static_and_anchored_package_in_system_image_read_write_roundtrip() {
        let static_test_package = make_test_package("test-package").await;
        let anchored_automatic_test_package = make_test_package("test-package-anchored").await;
        let system_image_package = SystemImageBuilder::new()
            .static_packages(&[&static_test_package])
            .anchored_package(
                AnchoredPackageSetType::Automatic,
                PackagePath::from_name_and_variant(
                    anchored_automatic_test_package.name().clone(),
                    "0".parse().unwrap(),
                ),
                *anchored_automatic_test_package.hash(),
            )
            .build()
            .await;
        let blobfs = BlobfsRamdisk::builder().start().await.expect("started blobfs");
        static_test_package.write_to_blobfs(&blobfs).await;
        anchored_automatic_test_package.write_to_blobfs(&blobfs).await;
        system_image_package.write_to_blobfs(&blobfs).await;

        let client = blobfs.client();

        let blobs_got = client.list_known_blobs().await.unwrap();
        let mut blobs_expected = HashSet::new();
        for digest in [
            "055dc718192ef18007ac4268e223224232f29bd4840eaac713da3e930bdc9c47",
            "211ac169e3f0422e09d9b51a0fe2c89122478a4eabc1300f51383842e9ecc54d",
            "797f25d8a6d0163a562d1e4a7850f128cea69bf0d68db1a3e472b3f79d5c125b",
            "81c6073abd7938367b78cf50eff3cfefeb733902951efc89312cae03d8ac16a7",
            "914cca7ed08e5675692fecc0d1a74a7f5536996b562fe3f09d41167425cbe50a",
            "a1c9428b24ad12f7a4cd20b8cdda877fbc65b93339462a7c7cda2b18dea334d9",
            "b6f3dc74baf53aa20a439925117811fbd54ba2641ffbd9e10838ac8bb5dba1d8",
        ] {
            blobs_expected.insert(GenericDigest::from_str(digest).unwrap());
        }

        assert_eq!(blobs_got, blobs_expected);
        assert_eq!(
            "test-package",
            String::from_utf8(
                client
                    .read_blob(
                        &GenericDigest::from_str(
                            "b6f3dc74baf53aa20a439925117811fbd54ba2641ffbd9e10838ac8bb5dba1d8"
                        )
                        .unwrap()
                    )
                    .await
                    .expect("reading blob")
            )
            .unwrap()
        );
        assert_eq!(
            "test-package-anchored",
            String::from_utf8(
                client
                    .read_blob(
                        &GenericDigest::from_str(
                            "a1c9428b24ad12f7a4cd20b8cdda877fbc65b93339462a7c7cda2b18dea334d9"
                        )
                        .unwrap()
                    )
                    .await
                    .expect("reading blob")
            )
            .unwrap()
        );
    }
}
