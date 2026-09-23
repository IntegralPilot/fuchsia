// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::update::{BuildInfo, SystemInfo, paver};
use anyhow::{Context as _, anyhow};
use epoch::EpochFile;
use fidl_fuchsia_mem as fmem;
use fidl_fuchsia_paver::{Asset, BootManagerProxy, DataSinkProxy};
use fuchsia_inspect as inspect;
use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use update_package::{SystemVersion, UpdatePackage};

/// The version of the OS.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Version {
    /// The hash of the update package.
    pub update_hash: String,
    /// The hash of the system image package.
    pub system_image_hash: String,
    /// For the currently running system, the vbmeta and zbi hash are SHA256 hash of the image with
    /// trailing zeros removed, because we can't use the exact image because when reading from paver
    /// we get the entire partition back.
    /// For an update package, the hashes are SHA256 hash of the image without removing trailing
    /// zeros.
    /// For an ota manifest, the hashes are the fuchsia merkle roots of the image blobs.
    pub vbmeta_hash: String,
    pub zbi_hash: String,
    /// The version in build-info.
    pub build_version: SystemVersion,
    /// The epoch of the update package.
    pub epoch: String,
}

impl Default for Version {
    fn default() -> Self {
        Version {
            update_hash: Default::default(),
            system_image_hash: Default::default(),
            vbmeta_hash: Default::default(),
            zbi_hash: Default::default(),
            build_version: SystemVersion::Opaque("".to_string()),
            epoch: Default::default(),
        }
    }
}

impl Version {
    #[cfg(test)]
    pub fn for_hash(update_hash: impl Into<String>) -> Self {
        Self { update_hash: update_hash.into(), ..Self::default() }
    }

    #[cfg(test)]
    pub fn for_hash_and_epoch(update_hash: impl Into<String>, epoch: impl Into<String>) -> Self {
        Self { update_hash: update_hash.into(), epoch: epoch.into(), ..Self::default() }
    }

    #[cfg(test)]
    pub fn for_hash_and_empty_paver_hashes(update_hash: impl Into<String>) -> Self {
        const EMPTY_HASH: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        Self {
            update_hash: update_hash.into(),
            vbmeta_hash: EMPTY_HASH.to_owned(),
            zbi_hash: EMPTY_HASH.to_owned(),
            epoch: fuchsia_pkg_testing::SOURCE_EPOCH.to_string(),
            ..Self::default()
        }
    }

    /// Returns the Version for the given update package.
    pub async fn for_update_package(update_package: &UpdatePackage) -> Self {
        let update_hash = match update_package.hash().await {
            Ok(hash) => hash.to_string(),
            Err(e) => {
                error!("Failed to get update hash: {:#}", anyhow!(e));
                "".to_string()
            }
        };
        let system_image_hash =
            get_system_image_hash_from_update_package(update_package).await.unwrap_or_else(|e| {
                error!("Failed to get system image hash: {:#}", anyhow!(e));
                "".to_string()
            });

        let (vbmeta_hash, zbi_hash) = match update_package.images_metadata().await {
            Ok(metadata) => {
                if let Some(fuchsia) = metadata.fuchsia() {
                    (
                        fuchsia
                            .vbmeta()
                            .map(|v| v.sha256().to_string())
                            .unwrap_or_else(|| "".into()),
                        fuchsia.zbi().sha256().to_string(),
                    )
                } else {
                    ("".into(), "".into())
                }
            }
            Err(e) => {
                error!(
                    "Failed to parse images manifest while obtaining hashes for version: {:#}",
                    anyhow!(e)
                );
                ("".into(), "".into())
            }
        };

        let build_version = update_package.version().await.unwrap_or_else(|e| {
            error!("Failed to read build version: {:#}", anyhow!(e));
            SystemVersion::Opaque("".to_string())
        });
        let epoch = match update_package.epoch().await {
            Ok(Some(epoch)) => epoch.to_string(),
            Ok(None) => {
                info!("epoch.json does not exist, defaulting to zero");
                "0".to_string()
            }
            Err(e) => {
                error!("Failed to read epoch: {:#}", anyhow!(e));
                "".to_string()
            }
        };
        Self { update_hash, system_image_hash, vbmeta_hash, zbi_hash, build_version, epoch }
    }

    pub fn for_manifest(manifest: &update_package::manifest::OtaManifest) -> Self {
        use update_package::manifest::AssetType;
        let [vbmeta_hash, zbi_hash] = [AssetType::Vbmeta, AssetType::Zbi].map(|asset_type| {
            manifest
                .images
                .iter()
                .find(|image| {
                    image.image_type == update_package::manifest::ImageType::Asset(asset_type)
                })
                .map(|image| image.blob.fuchsia_merkle_root.to_string())
                .unwrap_or_default()
        });
        let build_version = manifest.product_bundle_version.clone();
        let epoch = manifest.epoch.to_string();
        Self { vbmeta_hash, zbi_hash, build_version, epoch, ..Self::default() }
    }

    /// Returns the Version for the current running system.
    pub async fn current(
        last_target_version: Option<&Version>,
        data_sink: &DataSinkProxy,
        boot_manager: &BootManagerProxy,
        build_info: &impl BuildInfo,
        system_info: &impl SystemInfo,
        source_epoch_raw: &str,
    ) -> Self {
        let system_image_hash = match system_info.system_image_hash().await {
            Ok(Some(hash)) => hash.to_string(),
            Ok(None) => {
                warn!(
                    "Current system has no system_image package, so there is no system_image \
                     package hash to associate with this update attempt."
                );
                "".into()
            }
            Err(e) => {
                error!("Failed to read system image hash: {:#}", anyhow!(e));
                "".into()
            }
        };
        let (vbmeta_hash, zbi_hash) =
            get_vbmeta_and_zbi_hash_from_environment(data_sink, boot_manager).await;
        let build_version = match build_info.version().await {
            Ok(Some(version)) => version,
            Ok(None) => {
                error!("Build version not found");
                "".into()
            }
            Err(e) => {
                error!("Failed to read build version: {:#}", anyhow!(e));
                "".into()
            }
        };

        let build_version = SystemVersion::from_str(&build_version).unwrap();
        let update_hash = match last_target_version {
            Some(version) => {
                if vbmeta_hash == version.vbmeta_hash
                    && (system_image_hash.is_empty()
                        || system_image_hash == version.system_image_hash)
                    && (zbi_hash.is_empty() || zbi_hash == version.zbi_hash)
                    && (build_version.is_empty() || build_version == version.build_version)
                {
                    version.update_hash.clone()
                } else {
                    "".into()
                }
            }
            None => "".into(),
        };
        let epoch = match serde_json::from_str(source_epoch_raw) {
            Ok(EpochFile::Version1 { epoch }) => epoch.to_string(),
            Err(e) => {
                error!("Failed to parse source epoch: {:#}", anyhow!(e));
                "".into()
            }
        };

        Self { update_hash, system_image_hash, vbmeta_hash, zbi_hash, build_version, epoch }
    }

    pub fn write_to_inspect(&self, node: &inspect::Node) {
        // This destructure exists to use the compiler to guarantee we are copying all the
        // UpdateAttempt fields to inspect.
        let Version { update_hash, system_image_hash, vbmeta_hash, zbi_hash, build_version, epoch } =
            self;
        node.record_string("update_hash", update_hash);
        node.record_string("system_image_hash", system_image_hash);
        node.record_string("vbmeta_hash", vbmeta_hash);
        node.record_string("zbi_hash", zbi_hash);
        node.record_string("build_version", build_version.to_string());
        node.record_string("epoch", epoch);
    }
}

async fn get_system_image_hash_from_update_package(
    update_package: &UpdatePackage,
) -> Result<String, anyhow::Error> {
    let packages = update_package.packages().await?;
    let system_image = packages
        .into_iter()
        .find(|url| url.path() == "system_image/0")
        .ok_or_else(|| anyhow!("system image not found"))?;
    Ok(system_image.hash().to_string())
}

async fn get_vbmeta_and_zbi_hash_from_environment(
    data_sink: &DataSinkProxy,
    boot_manager: &BootManagerProxy,
) -> (String, String) {
    let configuration = match async {
        paver::query_current_configuration(boot_manager)
            .await?
            .to_configuration()
            .ok_or_else(|| anyhow!("device does not support ABR"))
    }
    .await
    {
        Ok(configuration) => configuration,
        Err(e) => {
            info!(
                "Failed to determine current vbmeta and zbi hash. \
                 This info is only used for diagnostic purposes, it is not necessary for OTA: {:#}",
                anyhow!(e)
            );
            return ("".into(), "".into());
        }
    };

    let vbmeta_hash = match async {
        let vbmeta_buffer = paver::read_image(
            data_sink,
            configuration,
            super::super::ImageType::Asset(Asset::VerifiedBootMetadata),
        )
        .await
        .context("reading vbmeta")?;
        sha256_hash_ignore_trailing_zeros(vbmeta_buffer).context("hashing vbmeta")
    }
    .await
    {
        Ok(vbmeta_hash) => vbmeta_hash.to_string(),
        Err(e) => {
            info!(
                "Failed to determine current vbmeta hash. \
                 This info is only used for diagnostic purposes, it is not necessary for OTA: {:#}",
                anyhow!(e)
            );
            "".into()
        }
    };

    let zbi_hash = match async {
        let zbi_buffer = paver::read_image(
            data_sink,
            configuration,
            super::super::ImageType::Asset(Asset::Kernel),
        )
        .await
        .context("reading kernel")?;
        sha256_hash_ignore_trailing_zeros(zbi_buffer).context("hashing kernel")
    }
    .await
    {
        Ok(zbi_hash) => zbi_hash.to_string(),
        Err(e) => {
            info!(
                "Failed to determine current zbi hash. \
                 This info is only used for diagnostic purposes, it is not necessary for OTA: {:#}",
                anyhow!(e)
            );
            "".into()
        }
    };

    (vbmeta_hash, zbi_hash)
}

// Compute the SHA256 hash of the buffer with the trailing zeros ignored.
fn sha256_hash_ignore_trailing_zeros(
    fmem::Buffer { vmo, size }: fmem::Buffer,
) -> Result<fuchsia_hash::Sha256, anyhow::Error> {
    let mapping =
        mapped_vmo::ImmutableMapping::create_from_vmo(&vmo, true).context("mapping the buffer")?;
    let size: usize = size.try_into().context("buffer size as usize")?;
    if size > mapping.len() {
        anyhow::bail!("buffer size {size} larger than vmo size {}", mapping.len());
    }
    let n = mapping[..size].iter().rposition(|b| *b != 0).map(|p| p + 1).unwrap_or_else(|| {
        warn!(size; "entire buffer is 0");
        0
    });
    Ok(From::from(*AsRef::<[u8; 32]>::as_ref(&<sha2::Sha256 as sha2::Digest>::digest(
        &mapping[..n],
    ))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::environment::NamespaceBuildInfo;
    use fidl_fuchsia_paver::Configuration;
    use fuchsia_hash::Hash;
    use fuchsia_pkg_testing::{FakeUpdatePackage, make_epoch_json};
    use mock_paver::{MockPaverServiceBuilder, hooks as mphooks};
    use omaha_client::version::Version as SemanticVersion;
    use pretty_assertions::assert_eq;
    use std::sync::Arc;
    use zx::Vmo;

    #[fuchsia::test]
    async fn version_for_invalid_update_package() {
        let update_pkg = FakeUpdatePackage::new();
        assert_eq!(
            Version::for_update_package(&update_pkg).await,
            Version { epoch: "0".to_string(), ..Version::default() }
        );
    }

    #[fuchsia::test]
    async fn version_for_valid_update_package() {
        let zbi_hash = [5; 32].into();
        let vbmeta_hash = [3; 32].into();
        let images_json = update_package::ImagePackagesManifest::builder()
            .fuchsia_package(
                update_package::ImageMetadata::new(
                    0,
                    zbi_hash,
                    format!(
                        "fuchsia-pkg://fuchsia.com/update-images-fuchsia/0?hash={}#zbi",
                        Hash::from([9; 32])
                    )
                    .parse()
                    .unwrap(),
                ),
                Some(update_package::ImageMetadata::new(
                    0,
                    vbmeta_hash,
                    format!(
                        "fuchsia-pkg://fuchsia.com/update-images-fuchsia/0?hash={}#vbmeta",
                        Hash::from([9; 32])
                    )
                    .parse()
                    .unwrap(),
                )),
            )
            .clone()
            .build();

        let update_pkg = FakeUpdatePackage::new()
            .hash("2937013f2181810606b2a799b05bda2849f3e369a20982a4138f0e0a55984ce4")
            .await
            .add_package("fuchsia-pkg://fuchsia.com/system_image/0?hash=838b5199d12c8ff4ef92bfd9771d2f8781b7b8fd739dd59bcf63f353a1a93f67")
            .await
            .add_file("version", "1.2.3.4")
            .await
            .add_file("images.json", serde_json::to_string(&images_json).unwrap())
            .await
            .add_file("epoch.json", make_epoch_json(42)).await;
        assert_eq!(
            Version::for_update_package(&update_pkg).await,
            Version {
                update_hash: "2937013f2181810606b2a799b05bda2849f3e369a20982a4138f0e0a55984ce4"
                    .to_string(),
                system_image_hash:
                    "838b5199d12c8ff4ef92bfd9771d2f8781b7b8fd739dd59bcf63f353a1a93f67".to_string(),
                vbmeta_hash: vbmeta_hash.to_string(),
                zbi_hash: zbi_hash.to_string(),
                build_version: SystemVersion::Semantic(SemanticVersion::from([1, 2, 3, 4])),
                epoch: "42".to_string()
            }
        );
    }

    #[fuchsia::test]
    async fn version_for_valid_update_package_no_vbmeta() {
        let zbi_hash = [5; 32].into();
        let images_json = update_package::ImagePackagesManifest::builder()
            .fuchsia_package(
                update_package::ImageMetadata::new(
                    0,
                    zbi_hash,
                    format!(
                        "fuchsia-pkg://fuchsia.com/update-images-fuchsia/0?hash={}#zbi",
                        Hash::from([9; 32])
                    )
                    .parse()
                    .unwrap(),
                ),
                None,
            )
            .clone()
            .build();

        let update_pkg = FakeUpdatePackage::new()
            .hash("2937013f2181810606b2a799b05bda2849f3e369a20982a4138f0e0a55984ce4")
            .await
            .add_package("fuchsia-pkg://fuchsia.com/system_image/0?hash=838b5199d12c8ff4ef92bfd9771d2f8781b7b8fd739dd59bcf63f353a1a93f67")
            .await
            .add_file("version", "1.2.3.4")
            .await
            .add_file("images.json", serde_json::to_string(&images_json).unwrap())
            .await
            .add_file("epoch.json", make_epoch_json(42)).await;
        assert_eq!(
            Version::for_update_package(&update_pkg).await,
            Version {
                update_hash: "2937013f2181810606b2a799b05bda2849f3e369a20982a4138f0e0a55984ce4"
                    .to_string(),
                system_image_hash:
                    "838b5199d12c8ff4ef92bfd9771d2f8781b7b8fd739dd59bcf63f353a1a93f67".to_string(),
                vbmeta_hash: "".to_string(),
                zbi_hash: zbi_hash.to_string(),
                build_version: SystemVersion::Semantic(SemanticVersion::from([1, 2, 3, 4])),
                epoch: "42".to_string()
            }
        );
    }

    #[fuchsia::test]
    async fn version_for_valid_update_package_no_fuchsia_package() {
        let hash = [5; 32].into();
        let images_json = update_package::ImagePackagesManifest::builder()
            .recovery_package(
                update_package::ImageMetadata::new(
                    0,
                    hash,
                    format!(
                        "fuchsia-pkg://fuchsia.com/update-images-recovery/0?hash={}#zbi",
                        Hash::from([9; 32])
                    )
                    .parse()
                    .unwrap(),
                ),
                None,
            )
            .clone()
            .build();

        let update_pkg = FakeUpdatePackage::new()
            .hash("2937013f2181810606b2a799b05bda2849f3e369a20982a4138f0e0a55984ce4")
            .await
            .add_package("fuchsia-pkg://fuchsia.com/system_image/0?hash=838b5199d12c8ff4ef92bfd9771d2f8781b7b8fd739dd59bcf63f353a1a93f67")
            .await
            .add_file("version", "1.2.3.4")
            .await
            .add_file("images.json", serde_json::to_string(&images_json).unwrap())
            .await
            .add_file("epoch.json", make_epoch_json(42)).await;
        assert_eq!(
            Version::for_update_package(&update_pkg).await,
            Version {
                update_hash: "2937013f2181810606b2a799b05bda2849f3e369a20982a4138f0e0a55984ce4"
                    .to_string(),
                system_image_hash:
                    "838b5199d12c8ff4ef92bfd9771d2f8781b7b8fd739dd59bcf63f353a1a93f67".to_string(),
                vbmeta_hash: "".to_string(),
                zbi_hash: "".to_string(),
                build_version: SystemVersion::Semantic(SemanticVersion::from([1, 2, 3, 4])),
                epoch: "42".to_string()
            }
        );
    }

    #[fuchsia::test]
    async fn version_for_valid_update_package_uses_images_json_if_present_v1() {
        let zbi_hash = [5; 32].into();
        let vbmeta_hash = [3; 32].into();
        let images_json = update_package::ImagePackagesManifest::builder()
            .fuchsia_package(
                update_package::ImageMetadata::new(
                    0,
                    zbi_hash,
                    format!(
                        "fuchsia-pkg://fuchsia.com/update-images-fuchsia/0?hash={}#zbi",
                        Hash::from([9; 32])
                    )
                    .parse()
                    .unwrap(),
                ),
                Some(update_package::ImageMetadata::new(
                    0,
                    vbmeta_hash,
                    format!(
                        "fuchsia-pkg://fuchsia.com/update-images-fuchsia/0?hash={}#vbmeta",
                        Hash::from([9; 32])
                    )
                    .parse()
                    .unwrap(),
                )),
            )
            .clone()
            .build();

        let update_pkg = FakeUpdatePackage::new()
            .hash("2937013f2181810606b2a799b05bda2849f3e369a20982a4138f0e0a55984ce4")
            .await
            .add_package("fuchsia-pkg://fuchsia.com/system_image/0?hash=838b5199d12c8ff4ef92bfd9771d2f8781b7b8fd739dd59bcf63f353a1a93f67")
            .await
            .add_file("version", "1.2.3.4")
            .await
            .add_file("images.json", serde_json::to_string(&images_json).unwrap())
            .await
            .add_file("epoch.json", make_epoch_json(42))
            .await
            .add_file("fuchsia.vbmeta", "vbmeta")
            .await
            .add_file("zbi", "zbi")
            .await;
        assert_eq!(
            Version::for_update_package(&update_pkg).await,
            Version {
                update_hash: "2937013f2181810606b2a799b05bda2849f3e369a20982a4138f0e0a55984ce4"
                    .to_string(),
                system_image_hash:
                    "838b5199d12c8ff4ef92bfd9771d2f8781b7b8fd739dd59bcf63f353a1a93f67".to_string(),
                vbmeta_hash: vbmeta_hash.to_string(),
                zbi_hash: zbi_hash.to_string(),
                build_version: SystemVersion::Semantic(SemanticVersion::from([1, 2, 3, 4])),
                epoch: "42".to_string()
            }
        );
    }

    #[fuchsia::test]
    async fn version_current_copy_update_hash() {
        let paver = Arc::new(
            MockPaverServiceBuilder::new()
                .active_config(Configuration::A)
                .insert_hook(mphooks::read_asset(|configuration, asset| {
                    assert_eq!(configuration, Configuration::A);
                    match asset {
                        Asset::Kernel => Ok(vec![0x7a, 0x62, 0x69]),
                        Asset::VerifiedBootMetadata => Ok(vec![0x76, 0x62, 0x6d, 0x65, 0x74, 0x61]),
                    }
                }))
                .build(),
        );
        let data_sink = paver.spawn_data_sink_service();
        let boot_manager = paver.spawn_boot_manager_service();
        let system_info = crate::update::environment::FakeSystemInfo(Some(
            "838b5199d12c8ff4ef92bfd9771d2f8781b7b8fd739dd59bcf63f353a1a93f67".parse().unwrap(),
        ));
        let last_target_version = Version {
            update_hash: "2937013f2181810606b2a799b05bda2849f3e369a20982a4138f0e0a55984ce4".into(),
            system_image_hash: "838b5199d12c8ff4ef92bfd9771d2f8781b7b8fd739dd59bcf63f353a1a93f67"
                .into(),
            vbmeta_hash: "a0c6f07a4b3a17fb9348db981de3c5602e2685d626599be1bd909195c694a57b".into(),
            zbi_hash: "a7124150e065aa234710ab3875230f17deb36a9249938e11f2f3656954412ab8".into(),
            build_version: SystemVersion::Opaque("".into()),
            epoch: "42".into(),
        };
        assert_eq!(
            Version::current(
                Some(&last_target_version),
                &data_sink,
                &boot_manager,
                &NamespaceBuildInfo,
                &system_info,
                &make_epoch_json(42)
            )
            .await,
            last_target_version,
        );
    }

    #[test]
    fn sha256_hash_empty_buffer() {
        let buffer = fmem::Buffer { vmo: Vmo::create(0).unwrap(), size: 0 };
        assert_eq!(
            sha256_hash_ignore_trailing_zeros(buffer).unwrap().to_string(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn sha256_hash_all_zero_buffer() {
        let vmo = Vmo::create(100).unwrap();
        vmo.write(&[0; 100], 0).unwrap();
        let buffer = fmem::Buffer { vmo, size: 100 };
        assert_eq!(
            sha256_hash_ignore_trailing_zeros(buffer).unwrap().to_string(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn sha256_hash_removed_trailing_zeros() {
        let vmo = Vmo::create(100).unwrap();
        vmo.write(&[0; 100], 0).unwrap();
        vmo.write(&[1; 1], 0).unwrap();
        let buffer = fmem::Buffer { vmo, size: 100 };
        assert_eq!(
            sha256_hash_ignore_trailing_zeros(buffer).unwrap().to_string(),
            "4bf5122f344554c53bde2ebb8cd2b7e3d1600ad631c385a5d7cce23c7785459a"
        );
    }
}
