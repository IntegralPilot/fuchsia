// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Context as _;
use assembled_system::{AssembledSystem, Image};
use assembly_partitions_config::PartitionsConfig;
use camino::Utf8PathBuf;
use delivery_blob::DeliveryBlobType;
use epoch::EpochFile;
use fuchsia_pkg::PackageManifest;
use std::collections::{BTreeMap, BTreeSet};
use update_package::manifest::{self, OtaManifest};

fn get_all_blobs(
    packages: &[(Option<Utf8PathBuf>, PackageManifest)],
) -> anyhow::Result<BTreeMap<fuchsia_merkle::Hash, u64>> {
    let mut blobs = BTreeMap::new();
    let mut subpackages_to_visit = Vec::new();

    for (_path, manifest) in packages {
        for blob in manifest.blobs() {
            blobs.insert(blob.merkle, blob.size);
        }
        subpackages_to_visit.extend(manifest.subpackages().iter().cloned());
    }

    let mut visited_subpackages = BTreeSet::new();
    while let Some(subpackage) = subpackages_to_visit.pop() {
        if visited_subpackages.contains(&subpackage.merkle) {
            continue;
        }
        visited_subpackages.insert(subpackage.merkle);

        let manifest = PackageManifest::try_load_from(&subpackage.manifest_path)
            .with_context(|| format!("parsing subpackage manifest {}", subpackage.manifest_path))?;
        for blob in manifest.blobs() {
            blobs.insert(blob.merkle, blob.size);
        }
        subpackages_to_visit.extend(manifest.subpackages().iter().cloned());
    }

    Ok(blobs)
}

/// Write the ota manifest to `out_path`.
pub fn write_ota_manifest(
    version: impl AsRef<str>,
    epoch: &EpochFile,
    delivery_blob_type: DeliveryBlobType,
    system_a: &Option<AssembledSystem>,
    system_r: &Option<AssembledSystem>,
    partitions: &PartitionsConfig,
    packages_a: &[(Option<Utf8PathBuf>, PackageManifest)],
    out_path: impl AsRef<std::path::Path>,
) -> anyhow::Result<()> {
    let version = version.as_ref().parse().context("parsing version string")?;
    let delivery_blob_type_val: u32 = delivery_blob_type.into();

    let mut images = vec![];
    let mut collect_images = |system: &AssembledSystem, slot| {
        let has_signed_zbi =
            system.images.iter().any(|image| matches!(image, Image::ZBI { signed: true, .. }));
        for image in &system.images {
            let (path, image_type) = match image {
                Image::ZBI { path, signed } => {
                    if has_signed_zbi && !signed {
                        continue;
                    }
                    (path, manifest::ImageType::Asset(manifest::AssetType::Zbi))
                }
                Image::VBMetaSystem(path) => {
                    (path, manifest::ImageType::Firmware("vbmeta_system".into()))
                }
                Image::VBMeta(path) => {
                    (path, manifest::ImageType::Asset(manifest::AssetType::Vbmeta))
                }
                Image::Dtbo(path) => (path, manifest::ImageType::Firmware("dtbo".into())),
                Image::BasePackage(_)
                | Image::BlobFS { .. }
                | Image::FVM(_)
                | Image::FVMFastboot(_)
                | Image::FVMSparse(_)
                | Image::Fxfs(_)
                | Image::FxfsSparse { .. }
                | Image::QemuKernel(_)
                | Image::TestRamdisk(_) => continue,
            };
            images.push(
                manifest::Image::from_path(path, slot, image_type)
                    .with_context(|| format!("reading image: {path}"))?,
            );
        }
        anyhow::Ok(())
    };
    if let Some(system) = system_a {
        collect_images(system, manifest::Slot::AB)?;
    }

    if let Some(system) = system_r {
        match partitions.recovery_style().context("getting recovery style")? {
            assembly_partitions_config::RecoveryStyle::AB => {
                let has_signed_zbi = system
                    .images
                    .iter()
                    .any(|image| matches!(image, Image::ZBI { signed: true, .. }));
                for image in &system.images {
                    let (path, firmware_type) = match image {
                        Image::ZBI { path, signed } => {
                            if has_signed_zbi && !signed {
                                continue;
                            }
                            (path, "recovery_zbi")
                        }
                        Image::VBMeta(path) => (path, "recovery_vbmeta"),
                        Image::VBMetaSystem(_)
                        | Image::BasePackage(_)
                        | Image::BlobFS { .. }
                        | Image::Dtbo(_)
                        | Image::FVM(_)
                        | Image::FVMFastboot(_)
                        | Image::FVMSparse(_)
                        | Image::Fxfs(_)
                        | Image::FxfsSparse { .. }
                        | Image::QemuKernel(_)
                        | Image::TestRamdisk(_) => continue,
                    };
                    images.push(
                        manifest::Image::from_path(
                            path,
                            manifest::Slot::AB,
                            manifest::ImageType::Firmware(firmware_type.into()),
                        )
                        .with_context(|| format!("reading image: {path}"))?,
                    );
                }
            }
            assembly_partitions_config::RecoveryStyle::R => {
                collect_images(system, manifest::Slot::R)?;
            }
            assembly_partitions_config::RecoveryStyle::NoRecovery => {
                anyhow::bail!("Has recovery images but no recovery partitions");
            }
        }
    }

    for bootloader in &partitions.bootloader_partitions {
        let image = manifest::Image::from_path(
            &bootloader.image,
            manifest::Slot::AB,
            manifest::ImageType::Firmware(bootloader.partition_type.clone()),
        )
        .with_context(|| format!("reading image: {:?}", bootloader.image))?;

        // De-duplicate bootloader images if they contain the exact same information.
        // A/B firmware images currently need to be added separately for each slot for flashing but
        // want to be de-duped when building the OTA package (b/434935097).
        //
        // The number of images is small so this O(N) check should be fine.
        if !images.contains(&image) {
            images.push(image);
        }
    }

    let blobs = get_all_blobs(packages_a)
        .context("getting all blobs from packages")?
        .into_iter()
        .map(|(merkle, size)| manifest::Blob {
            uncompressed_size: size,
            fuchsia_merkle_root: merkle,
        })
        .collect();

    let manifest = OtaManifest {
        product_bundle_version: version,
        board: partitions.hardware_revision.clone(),
        epoch: match epoch {
            EpochFile::Version1 { epoch } => *epoch,
        },
        mode: update_package::UpdateMode::Normal,
        // Relative to the OTA manifest URL.
        blob_base_url: format!("blobs/{delivery_blob_type_val}"),
        images,
        blobs,
    };
    if let Some(parent) = out_path.as_ref().parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating directory {}", parent.display()))?;
    }

    let pem = pem::parse(update_package::MANIFEST_DEV_KEY_PEM).context("parsing pem")?;
    let key_pair = ring::signature::Ed25519KeyPair::from_pkcs8_maybe_unchecked(&pem.contents())
        .map_err(|e| anyhow::anyhow!("parsing pkcs8: {e}"))?;
    let signed_manifest_bytes =
        update_package::signed_manifest::generate(manifest, &key_pair, None)?;
    std::fs::write(&out_path, &signed_manifest_bytes).context("writing signed ota manifest")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use assembly_partitions_config::{BootloaderPartition, Partition, Slot as PartitionSlot};
    use assembly_release_info::SystemReleaseInfo;
    use assembly_test_util::generate_test_manifest_with_blobs;
    use camino::Utf8Path;
    use fuchsia_pkg::{
        BlobInfo, MetaPackage, PackageManifest, PackageManifestBuilder, SubpackageInfo,
    };
    use pretty_assertions::assert_eq;
    use ring::signature::KeyPair as _;
    use tempfile::NamedTempFile;

    fn dev_public_key() -> ring::signature::UnparsedPublicKey<Vec<u8>> {
        let pem = pem::parse(update_package::MANIFEST_DEV_KEY_PEM).unwrap();
        let key_pair =
            ring::signature::Ed25519KeyPair::from_pkcs8_maybe_unchecked(pem.contents()).unwrap();
        ring::signature::UnparsedPublicKey::new(
            &ring::signature::ED25519,
            key_pair.public_key().as_ref().to_vec(),
        )
    }

    #[test]
    fn build_ota_manifest() {
        let fake_zbi = NamedTempFile::new().unwrap();
        let fake_vbmeta = NamedTempFile::new().unwrap();
        let system_a = Some(AssembledSystem {
            images: vec![
                Image::ZBI { path: "unsigned zbi".to_owned().into(), signed: false },
                Image::ZBI {
                    path: Utf8Path::from_path(fake_zbi.path()).unwrap().to_path_buf(),
                    signed: true,
                },
                Image::VBMeta(Utf8Path::from_path(fake_vbmeta.path()).unwrap().to_path_buf()),
            ],
            board_name: "board".into(),
            partitions_config: None,
            system_release_info: SystemReleaseInfo::new_for_testing(),
            platform_tools: vec![],
        });

        let partitions = PartitionsConfig {
            bootstrap_partitions: vec![],
            unlock_credentials: vec![],
            bootloader_partitions: vec![],
            partitions: vec![
                Partition::ZBI { name: "zircon_a".into(), slot: PartitionSlot::A, size: None },
                Partition::VBMeta { name: "vbmeta_a".into(), slot: PartitionSlot::A, size: None },
            ],
            hardware_revision: "board".into(),
            product_matches: vec![],
            ssh_key_upload_method: None,
        };

        let meta_far_merkle = "0".repeat(64).parse().unwrap();
        let meta_far = BlobInfo {
            merkle: meta_far_merkle,
            size: 100,
            path: PackageManifest::META_FAR_BLOB_PATH.into(),
            source_path: "src_path0".into(),
        };
        let blob1_merkle = "1".repeat(64).parse().unwrap();
        let blob1 = BlobInfo {
            merkle: blob1_merkle,
            size: 200,
            path: "path1".into(),
            source_path: "src_path1".into(),
        };

        let pkg1 = generate_test_manifest_with_blobs("pkg1", [meta_far, blob1]);

        let manifest_file = NamedTempFile::new().unwrap();

        write_ota_manifest(
            "1.2.3.4",
            &EpochFile::Version1 { epoch: 1 },
            DeliveryBlobType::Type1,
            &system_a,
            &None,
            &partitions,
            &[(None, pkg1)],
            manifest_file.path(),
        )
        .unwrap();

        let manifest_bytes = std::fs::read(manifest_file.path()).unwrap();
        let manifest =
            update_package::signed_manifest::parse_and_verify(&manifest_bytes, &[dev_public_key()])
                .unwrap();

        assert_eq!(manifest.product_bundle_version, "1.2.3.4".parse().unwrap());
        assert_eq!(manifest.epoch, 1);
        assert_eq!(manifest.board, "board");
        assert_eq!(manifest.mode, update_package::UpdateMode::Normal);
        assert_eq!(manifest.blob_base_url, "blobs/1");
        assert_eq!(manifest.images.len(), 2);
        assert_eq!(
            manifest.images[0],
            manifest::Image {
                slot: manifest::Slot::AB,
                image_type: manifest::ImageType::Asset(manifest::AssetType::Zbi),
                blob: manifest::Blob {
                    uncompressed_size: 0,
                    fuchsia_merkle_root:
                        "15ec7bf0b50732b49f8228e07d24365338f9e3ab994b00af08e5a3bffe55fd8b"
                            .parse()
                            .unwrap(),
                },
            }
        );
        assert_eq!(
            manifest.images[1],
            manifest::Image {
                slot: manifest::Slot::AB,
                image_type: manifest::ImageType::Asset(manifest::AssetType::Vbmeta),
                blob: manifest::Blob {
                    uncompressed_size: 0,
                    fuchsia_merkle_root:
                        "15ec7bf0b50732b49f8228e07d24365338f9e3ab994b00af08e5a3bffe55fd8b"
                            .parse()
                            .unwrap(),
                },
            }
        );
        assert_eq!(manifest.blobs.len(), 2);
        assert_eq!(
            manifest.blobs[0],
            manifest::Blob { uncompressed_size: 100, fuchsia_merkle_root: meta_far_merkle }
        );
        assert_eq!(
            manifest.blobs[1],
            manifest::Blob { uncompressed_size: 200, fuchsia_merkle_root: blob1_merkle }
        );
    }

    #[test]
    fn build_ota_manifest_with_subpackages_and_recovery() {
        // System A images
        let fake_zbi_a = NamedTempFile::new().unwrap();
        let fake_vbmeta_a = NamedTempFile::new().unwrap();
        let fake_dtbo = NamedTempFile::new().unwrap();
        let system_a = Some(AssembledSystem {
            images: vec![
                Image::ZBI { path: "unsigned zbi".to_owned().into(), signed: false },
                Image::ZBI {
                    path: Utf8Path::from_path(fake_zbi_a.path()).unwrap().to_path_buf(),
                    signed: true,
                },
                Image::VBMeta(Utf8Path::from_path(fake_vbmeta_a.path()).unwrap().to_path_buf()),
                Image::Dtbo(Utf8Path::from_path(fake_dtbo.path()).unwrap().to_path_buf()),
            ],
            board_name: "board".into(),
            partitions_config: None,
            system_release_info: SystemReleaseInfo::new_for_testing(),
            platform_tools: vec![],
        });

        // System R images
        let fake_zbi_r = NamedTempFile::new().unwrap();
        let fake_vbmeta_r = NamedTempFile::new().unwrap();
        let system_r = Some(AssembledSystem {
            images: vec![
                Image::ZBI {
                    path: Utf8Path::from_path(fake_zbi_r.path()).unwrap().to_path_buf(),
                    signed: true,
                },
                Image::ZBI { path: "unsigned zbi".to_owned().into(), signed: false },
                Image::VBMeta(Utf8Path::from_path(fake_vbmeta_r.path()).unwrap().to_path_buf()),
            ],
            board_name: "board".into(),
            partitions_config: None,
            system_release_info: SystemReleaseInfo::new_for_testing(),
            platform_tools: vec![],
        });

        let fake_bootloader = NamedTempFile::new().unwrap();
        let partitions = PartitionsConfig {
            bootstrap_partitions: vec![],
            unlock_credentials: vec![],
            bootloader_partitions: vec![BootloaderPartition {
                partition_type: "bl_type".into(),
                name: Some("bootloader".into()),
                image: Utf8Path::from_path(fake_bootloader.path()).unwrap().to_path_buf(),
            }],
            partitions: vec![
                Partition::ZBI { name: "zircon_a".into(), slot: PartitionSlot::A, size: None },
                Partition::VBMeta { name: "vbmeta_a".into(), slot: PartitionSlot::A, size: None },
                Partition::Dtbo { name: "dtbo_a".into(), slot: PartitionSlot::A, size: None },
                Partition::ZBI { name: "zircon_r".into(), slot: PartitionSlot::R, size: None },
                Partition::VBMeta { name: "vbmeta_r".into(), slot: PartitionSlot::R, size: None },
            ],
            hardware_revision: "board".into(),
            product_matches: vec![],
            ssh_key_upload_method: None,
        };

        // Packages with subpackages
        let meta_far_merkle = "0".repeat(64).parse().unwrap();
        let meta_far = BlobInfo {
            merkle: meta_far_merkle,
            size: 100,
            path: PackageManifest::META_FAR_BLOB_PATH.into(),
            source_path: "src_path0".into(),
        };
        let blob1_merkle = "1".repeat(64).parse().unwrap();
        let blob1 = BlobInfo {
            merkle: blob1_merkle,
            size: 200,
            path: "path1".into(),
            source_path: "src_path1".into(),
        };

        let blob2_merkle = "2".repeat(64).parse().unwrap();
        let blob2 = BlobInfo {
            merkle: blob2_merkle,
            size: 300,
            path: "path2".into(),
            source_path: "src_path2".into(),
        };

        let mut subpackage_manifest_file = NamedTempFile::new().unwrap();
        let subpackage_manifest_path = subpackage_manifest_file.path().to_str().unwrap().to_owned();
        let subpackage_merkle = "3".repeat(64).parse().unwrap();
        let subpackage_metafar = BlobInfo {
            merkle: subpackage_merkle,
            size: 400,
            path: PackageManifest::META_FAR_BLOB_PATH.into(),
            source_path: subpackage_manifest_path.clone(),
        };
        let subpackage_manifest =
            generate_test_manifest_with_blobs("subpkg", [subpackage_metafar, blob2]);
        serde_json::to_writer(&mut subpackage_manifest_file, &subpackage_manifest).unwrap();

        let meta_package = MetaPackage::from_name_and_variant_zero("pkg1".parse().unwrap());
        let subpackage_info = SubpackageInfo {
            name: "subpkg".to_string(),
            merkle: subpackage_merkle,
            manifest_path: subpackage_manifest_path,
        };
        let pkg1 = PackageManifestBuilder::new(meta_package)
            .add_blob(meta_far)
            .add_blob(blob1)
            .add_subpackage(subpackage_info)
            .build();

        let manifest_file = NamedTempFile::new().unwrap();
        write_ota_manifest(
            "1.2.3.4",
            &EpochFile::Version1 { epoch: 1 },
            DeliveryBlobType::Type1,
            &system_a,
            &system_r,
            &partitions,
            &[(None, pkg1)],
            manifest_file.path(),
        )
        .unwrap();

        let manifest_bytes = std::fs::read(manifest_file.path()).unwrap();
        let manifest =
            update_package::signed_manifest::parse_and_verify(&manifest_bytes, &[dev_public_key()])
                .unwrap();

        assert_eq!(manifest.product_bundle_version, "1.2.3.4".parse().unwrap());
        assert_eq!(manifest.epoch, 1);
        assert_eq!(manifest.board, "board");
        assert_eq!(manifest.images.len(), 6); // 3 from A, 2 from R, 1 bootloader
        assert_eq!(manifest.images[0].slot, manifest::Slot::AB);
        assert_eq!(
            manifest.images[0].image_type,
            manifest::ImageType::Asset(manifest::AssetType::Zbi),
        );
        assert_eq!(manifest.images[1].slot, manifest::Slot::AB);
        assert_eq!(
            manifest.images[1].image_type,
            manifest::ImageType::Asset(manifest::AssetType::Vbmeta),
        );
        assert_eq!(manifest.images[2].slot, manifest::Slot::AB);
        assert_eq!(manifest.images[2].image_type, manifest::ImageType::Firmware("dtbo".into()));
        assert_eq!(manifest.images[3].slot, manifest::Slot::R);
        assert_eq!(
            manifest.images[3].image_type,
            manifest::ImageType::Asset(manifest::AssetType::Zbi),
        );
        assert_eq!(manifest.images[4].slot, manifest::Slot::R);
        assert_eq!(
            manifest.images[4].image_type,
            manifest::ImageType::Asset(manifest::AssetType::Vbmeta),
        );
        assert_eq!(manifest.images[5].slot, manifest::Slot::AB);
        assert_eq!(manifest.images[5].image_type, manifest::ImageType::Firmware("bl_type".into()));

        assert_eq!(manifest.blobs.len(), 4);
        assert_eq!(
            manifest.blobs[0],
            manifest::Blob { uncompressed_size: 100, fuchsia_merkle_root: meta_far_merkle }
        );
        assert_eq!(
            manifest.blobs[1],
            manifest::Blob { uncompressed_size: 200, fuchsia_merkle_root: blob1_merkle }
        );
        assert_eq!(
            manifest.blobs[2],
            manifest::Blob { uncompressed_size: 300, fuchsia_merkle_root: blob2_merkle }
        );
        assert_eq!(
            manifest.blobs[3],
            manifest::Blob { uncompressed_size: 400, fuchsia_merkle_root: subpackage_merkle }
        );
    }

    #[test]
    fn build_ota_manifest_with_ab_recovery() {
        // System A images
        let fake_zbi_a = NamedTempFile::new().unwrap();
        let fake_vbmeta_a = NamedTempFile::new().unwrap();
        let system_a = Some(AssembledSystem {
            images: vec![
                Image::ZBI {
                    path: Utf8Path::from_path(fake_zbi_a.path()).unwrap().to_path_buf(),
                    signed: false,
                },
                Image::VBMeta(Utf8Path::from_path(fake_vbmeta_a.path()).unwrap().to_path_buf()),
            ],
            board_name: "board".into(),
            partitions_config: None,
            system_release_info: SystemReleaseInfo::new_for_testing(),
            platform_tools: vec![],
        });

        // System R images
        let fake_zbi_r = NamedTempFile::new().unwrap();
        let fake_vbmeta_r = NamedTempFile::new().unwrap();
        let system_r = Some(AssembledSystem {
            images: vec![
                Image::ZBI { path: "unsigned zbi".to_owned().into(), signed: false },
                Image::ZBI {
                    path: Utf8Path::from_path(fake_zbi_r.path()).unwrap().to_path_buf(),
                    signed: true,
                },
                Image::VBMeta(Utf8Path::from_path(fake_vbmeta_r.path()).unwrap().to_path_buf()),
            ],
            board_name: "board".into(),
            partitions_config: None,
            system_release_info: SystemReleaseInfo::new_for_testing(),
            platform_tools: vec![],
        });

        let fake_bootloader = NamedTempFile::new().unwrap();
        let partitions = PartitionsConfig {
            bootstrap_partitions: vec![],
            unlock_credentials: vec![],
            bootloader_partitions: vec![BootloaderPartition {
                partition_type: "bl_type".into(),
                name: Some("bootloader".into()),
                image: Utf8Path::from_path(fake_bootloader.path()).unwrap().to_path_buf(),
            }],
            partitions: vec![
                Partition::ZBI { name: "zircon_a".into(), slot: PartitionSlot::A, size: None },
                Partition::VBMeta { name: "vbmeta_a".into(), slot: PartitionSlot::A, size: None },
                Partition::RecoveryZBI {
                    name: "recovery_zbi_a".into(),
                    slot: PartitionSlot::A,
                    size: None,
                },
                Partition::RecoveryVBMeta {
                    name: "recovery_vbmeta_a".into(),
                    slot: PartitionSlot::A,
                    size: None,
                },
            ],
            hardware_revision: "board".into(),
            product_matches: vec![],
            ssh_key_upload_method: None,
        };

        let manifest_file = NamedTempFile::new().unwrap();
        write_ota_manifest(
            "1.2.3.4",
            &EpochFile::Version1 { epoch: 1 },
            DeliveryBlobType::Type1,
            &system_a,
            &system_r,
            &partitions,
            &[],
            manifest_file.path(),
        )
        .unwrap();

        let manifest_bytes = std::fs::read(manifest_file.path()).unwrap();
        let manifest =
            update_package::signed_manifest::parse_and_verify(&manifest_bytes, &[dev_public_key()])
                .unwrap();

        assert_eq!(manifest.images.len(), 5);
        assert_eq!(manifest.blobs.len(), 0);
        assert_eq!(manifest.images[0].slot, manifest::Slot::AB);
        assert_eq!(
            manifest.images[0].image_type,
            manifest::ImageType::Asset(manifest::AssetType::Zbi),
        );
        assert_eq!(manifest.images[1].slot, manifest::Slot::AB);
        assert_eq!(
            manifest.images[1].image_type,
            manifest::ImageType::Asset(manifest::AssetType::Vbmeta),
        );
        assert_eq!(manifest.images[2].slot, manifest::Slot::AB);
        assert_eq!(
            manifest.images[2].image_type,
            manifest::ImageType::Firmware("recovery_zbi".into()),
        );
        assert_eq!(manifest.images[3].slot, manifest::Slot::AB);
        assert_eq!(
            manifest.images[3].image_type,
            manifest::ImageType::Firmware("recovery_vbmeta".into()),
        );
        assert_eq!(manifest.images[4].slot, manifest::Slot::AB);
        assert_eq!(manifest.images[4].image_type, manifest::ImageType::Firmware("bl_type".into()));
    }

    #[test]
    fn build_ota_manifest_with_duplicate_bootloaders() {
        let system_a = None;
        let system_r = None;

        let fake_bootloader = NamedTempFile::new().unwrap();
        let partitions = PartitionsConfig {
            bootstrap_partitions: vec![],
            unlock_credentials: vec![],
            bootloader_partitions: vec![
                // Same "type" and image contents, different partition name should de-duplicate.
                // Name only matters for flashing, not OTA, so these are identical for our purposes.
                BootloaderPartition {
                    partition_type: "bl_type".into(),
                    name: Some("bootloader_a".into()),
                    image: Utf8Path::from_path(fake_bootloader.path()).unwrap().to_path_buf(),
                },
                BootloaderPartition {
                    partition_type: "bl_type".into(),
                    name: Some("bootloader_b".into()),
                    image: Utf8Path::from_path(fake_bootloader.path()).unwrap().to_path_buf(),
                },
            ],
            partitions: vec![],
            hardware_revision: "board".into(),
            product_matches: vec![],
            ssh_key_upload_method: None,
        };

        let manifest_file = NamedTempFile::new().unwrap();
        write_ota_manifest(
            "1.2.3.4",
            &EpochFile::Version1 { epoch: 1 },
            DeliveryBlobType::Type1,
            &system_a,
            &system_r,
            &partitions,
            &[],
            manifest_file.path(),
        )
        .unwrap();

        let manifest_bytes = std::fs::read(manifest_file.path()).unwrap();
        let manifest =
            update_package::signed_manifest::parse_and_verify(&manifest_bytes, &[dev_public_key()])
                .unwrap();

        // Bootloader image should have de-duped so there's only one.
        assert_eq!(manifest.images.len(), 1);
        assert_eq!(manifest.images[0].slot, manifest::Slot::AB);
        assert_eq!(manifest.images[0].image_type, manifest::ImageType::Firmware("bl_type".into()));
    }
}
