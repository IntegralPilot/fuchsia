// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Generate a transfer manifest for uploading and downloading a product bundle.

use anyhow::{Context, Result};
use argh::FromArgs;
use assembled_system::Image;
use camino::{Utf8Path, Utf8PathBuf};
use pathdiff::diff_utf8_paths;
use product_bundle::ProductBundle;
use sdk_metadata::VirtualDeviceManifest;
use std::fs::File;
use transfer_manifest::{
    ArtifactEntry, ArtifactType, TransferEntry, TransferManifest, TransferManifestV1,
};
use walkdir::{DirEntry, WalkDir};

/// Generate a transfer manifest for uploading and downloading a product bundle.
#[derive(FromArgs, PartialEq, Debug)]
#[argh(subcommand, name = "generate-transfer-manifest")]
pub struct GenerateTransferManifest {
    /// path to a product bundle.
    #[argh(option)]
    product_bundle: Utf8PathBuf,

    /// path to the directory to write the transfer.json manifest.
    #[argh(option)]
    output: Utf8PathBuf,
}

impl GenerateTransferManifest {
    /// Generate the transfer manifest into `output`.
    pub async fn generate(self) -> Result<()> {
        let product_bundle = ProductBundle::try_load_from(&self.product_bundle)?;
        let product_bundle = match product_bundle {
            ProductBundle::V2(pb) => pb,
        };

        let out_dir = self.output.parent().unwrap();
        std::fs::create_dir_all(&out_dir).context("creating output directory")?;
        let canonical_out_dir =
            &out_dir.canonicalize_utf8().context("canonicalizing output directory")?;
        let canonical_output = canonical_out_dir.join(&self.output.file_name().unwrap());

        let canonical_product_bundle_path = &self
            .product_bundle
            .canonicalize_utf8()
            .context("canonicalizing product bundle path")?;

        let mut entries = vec![];
        // Add all the blobs to the transfer manifest.
        for repository in &product_bundle.repositories {
            let delivery_blob_path = repository.delivery_blob_type.to_string().into();
            let canonical_blobs_path = canonical_product_bundle_path
                .join(&repository.blobs_path)
                .join(&delivery_blob_path);
            let blobs = repository
                .blobs()
                .await
                .with_context(|| format!("gathering blobs from repository: {}", repository.name))?;
            let mut blob_entries: Vec<ArtifactEntry> =
                blobs.iter().map(|p| ArtifactEntry { name: p.into() }).collect();
            blob_entries.sort();
            let local = diff_utf8_paths(canonical_blobs_path, &canonical_out_dir)
                .context("rebasing blobs path")?;
            let blob_transfer = TransferEntry {
                artifact_type: ArtifactType::Blobs,
                local,
                remote: delivery_blob_path,
                entries: blob_entries,
            };
            entries.push(blob_transfer);
        }

        // Collect all the product bundle entries.
        let mut product_bundle_entries = vec![];
        product_bundle_entries.push(ArtifactEntry { name: "product_bundle.json".into() });
        for partition in &product_bundle.partitions.bootstrap_partitions {
            product_bundle_entries.push(ArtifactEntry {
                name: diff_utf8_paths(&partition.image, &canonical_product_bundle_path)
                    .context("rebasing bootstrap partition")?,
            });
        }
        for partition in &product_bundle.partitions.bootloader_partitions {
            product_bundle_entries.push(ArtifactEntry {
                name: diff_utf8_paths(&partition.image, &canonical_product_bundle_path)
                    .context("rebasing bootloader partition")?,
            });
        }
        for credential in &product_bundle.partitions.unlock_credentials {
            product_bundle_entries.push(ArtifactEntry {
                name: diff_utf8_paths(&credential, &canonical_product_bundle_path)
                    .context("rebasing unlock credential")?,
            });
        }

        // Add the images from the systems.
        let mut system = |system: &Option<Vec<Image>>| -> Result<()> {
            if let Some(system) = system {
                for image in system.iter() {
                    product_bundle_entries.push(ArtifactEntry {
                        name: diff_utf8_paths(image.source(), &canonical_product_bundle_path)
                            .context("rebasing system image")?,
                    });
                }
            }
            Ok(())
        };
        system(&product_bundle.system_a)?;
        system(&product_bundle.system_b)?;
        system(&product_bundle.system_r)?;

        // Add virtual devices.
        if let Some(manifest_path) = &product_bundle.virtual_devices_path {
            product_bundle_entries.push(ArtifactEntry {
                name: diff_utf8_paths(manifest_path, &canonical_product_bundle_path)
                    .context("rebasing virtual device manifest path")?,
            });
            let manifest_dir = manifest_path.parent().unwrap_or_else(|| "".into());
            let manifest = VirtualDeviceManifest::from_path(&product_bundle.virtual_devices_path)
                .context("manifest from_path")?;
            for path in manifest.device_paths.values() {
                let virtual_device_path = manifest_dir.join(path);
                product_bundle_entries.push(ArtifactEntry {
                    name: diff_utf8_paths(&virtual_device_path, &canonical_product_bundle_path)
                        .context("rebasing virtual device path")?,
                });
            }
        }

        // Add the tuf metadata by walking the metadata directories and listing all the files inside.
        for repository in &product_bundle.repositories {
            let entries: Result<Vec<DirEntry>, _> =
                WalkDir::new(&repository.metadata_path).into_iter().collect();
            let entries = entries.with_context(|| {
                format!("collecting tuf metadata from repository: {}", repository.name)
            })?;
            for entry in entries {
                if entry.file_type().is_file() {
                    let entry_path =
                        Utf8Path::from_path(entry.path()).context("converting to UTF-8")?;

                    product_bundle_entries.push(ArtifactEntry {
                        name: diff_utf8_paths(entry_path, canonical_product_bundle_path)
                            .context("rebasing tuf metadata")?,
                    });
                }
            }
            let mut key = |private_key_path: &Option<Utf8PathBuf>| -> Result<()> {
                if let Some(path) = private_key_path {
                    product_bundle_entries.push(ArtifactEntry {
                        name: diff_utf8_paths(path, canonical_product_bundle_path)
                            .context("rebasing tuf private key")?,
                    });
                }
                Ok(())
            };
            key(&repository.root_private_key_path)?;
            key(&repository.targets_private_key_path)?;
            key(&repository.snapshot_private_key_path)?;
            key(&repository.timestamp_private_key_path)?;
        }
        product_bundle_entries.sort();
        product_bundle_entries.dedup();

        let local = diff_utf8_paths(&canonical_product_bundle_path, &canonical_out_dir)
            .context("rebasing product_bundle path")?;
        entries.push(TransferEntry {
            artifact_type: ArtifactType::Files,
            local,
            remote: "".into(),
            entries: product_bundle_entries,
        });

        // Write the transfer manifest.
        let transfer_manifest = TransferManifest::V1(TransferManifestV1 { entries });
        let file = File::create(canonical_output).context("creating transfer manifest")?;
        serde_json::to_writer_pretty(file, &transfer_manifest)
            .context("writing transfer manifest")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use assembly_partitions_config::PartitionsConfig;
    use fuchsia_repo::test_utils;
    use product_bundle::{ProductBundleV2, Repository};
    use sdk_metadata::virtual_device::Hardware;
    use sdk_metadata::{VirtualDevice, VirtualDeviceV1};
    use std::io::Write;
    use tempfile::tempdir;

    #[fuchsia::test]
    async fn test_generate() {
        let tmp = tempdir().unwrap();
        let tempdir = Utf8Path::from_path(tmp.path()).unwrap().canonicalize_utf8().unwrap();

        let pb_path = tempdir.join("product_bundle");
        std::fs::create_dir_all(&pb_path).unwrap();

        let create_temp_file = |name: &str| -> Utf8PathBuf {
            let path = pb_path.join(name);
            let mut file = File::create(&path).unwrap();
            write!(file, "{}", name).unwrap();
            path
        };

        let _repo = test_utils::make_repo_dir(
            pb_path.join("repository").as_std_path(),
            &pb_path.join("blobs").as_std_path(),
            None,
        )
        .await;

        let vd_dir = pb_path.join("virtual_devices");
        std::fs::create_dir_all(&vd_dir).unwrap();

        let vd_manifest_path = vd_dir.join("manifest.json");
        let mut vd_manifest = VirtualDeviceManifest::default();
        vd_manifest.device_paths = [
            ("virtual_device_A".into(), "virtual_device_A.json".into()),
            ("virtual_device_B".into(), "virtual_device_B.json".into()),
            ("virtual_device_C".into(), "virtual_device_C.json".into()),
        ]
        .into();
        for (name, path) in &vd_manifest.device_paths {
            let vd = VirtualDeviceV1::new(name, Hardware::default());
            VirtualDevice::V1(vd).write(vd_dir.join(path)).unwrap();
        }
        let vd_manifest_file = File::create(vd_manifest_path.clone()).unwrap();
        serde_json::to_writer(vd_manifest_file, &vd_manifest).unwrap();

        let pb = ProductBundle::V2(ProductBundleV2 {
            product_name: "my-product-bundle".to_string(),
            product_version: "".to_string(),
            partitions: PartitionsConfig::default(),
            sdk_version: "".to_string(),
            system_a: Some(vec![
                Image::ZBI { path: create_temp_file("zbi"), signed: false },
                Image::FVM(create_temp_file("fvm")),
                Image::QemuKernel(create_temp_file("kernel")),
            ]),
            system_b: None,
            system_r: None,
            platform_tools_a: vec![],
            platform_tools_b: vec![],
            platform_tools_r: vec![],
            repositories: vec![Repository {
                name: "fuchsia.com".into(),
                metadata_path: pb_path.join("repository"),
                blobs_path: pb_path.join("blobs"),
                delivery_blob_type: 1,
                root_private_key_path: None,
                targets_private_key_path: Some(pb_path.join("keys/targets.json")),
                snapshot_private_key_path: Some(pb_path.join("keys/snapshot.json")),
                timestamp_private_key_path: Some(pb_path.join("keys/timestamp.json")),
                ota_manifest_signature_path: None,
                ota_manifest_path: None,
            }],
            update_package_hash: None,
            virtual_devices_path: Some(vd_manifest_path),
            release_info: None,
        });
        pb.write(&pb_path).unwrap();

        let output = tempdir.join("transfer.json");
        let cmd =
            GenerateTransferManifest { product_bundle: pb_path.clone(), output: output.clone() };
        cmd.generate().await.unwrap();

        let transfer_manifest_file = File::open(output).unwrap();
        let transfer_manifest: TransferManifest =
            serde_json::from_reader(transfer_manifest_file).unwrap();
        assert_eq!(
            transfer_manifest,
            TransferManifest::V1(TransferManifestV1 {
                entries: vec![
                    TransferEntry {
                        artifact_type: transfer_manifest::ArtifactType::Blobs,
                        local: "product_bundle/blobs/1".into(),
                        remote: "1".into(),
                        entries: vec![
                            ArtifactEntry { name: "050907f009ff634f9aa57bff541fb9e9c2c62b587c23578e77637cda3bd69458".into() },
                            ArtifactEntry { name: "2881455493b5870aaea36537d70a2adc635f516ac2092598f4b6056dabc6b25d".into() },
                            ArtifactEntry { name: "548981eb310ddc4098fb5c63692e19ac4ae287b13d0e911fbd9f7819ac22491c".into() },
                            ArtifactEntry { name: "72e1e7a504f32edf4f23e7e8a3542c1d77d12541142261cfe272decfa75f542d".into() },
                            ArtifactEntry { name: "8a8a5f07f935a4e8e1fd1a1eda39da09bb2438ec0adfb149679ddd6e7e1fbb4f".into() },
                            ArtifactEntry { name: "ecc11f7f4b763c5a21be2b4159c9818bbe22ca7e6d8100a72f6a41d3d7b827a9".into() },
                        ]
                    },
                    TransferEntry {
                        artifact_type: transfer_manifest::ArtifactType::Files,
                        local: "product_bundle".into(),
                        remote: "".into(),
                        entries: vec![
                            ArtifactEntry { name: "fvm".into() },
                            ArtifactEntry { name: "kernel".into() },
                            ArtifactEntry { name: "keys/snapshot.json".into() },
                            ArtifactEntry { name: "keys/targets.json".into() },
                            ArtifactEntry { name: "keys/timestamp.json".into() },
                            ArtifactEntry { name: "product_bundle.json".into() },
                            ArtifactEntry { name: "repository/1.root.json".into() },
                            ArtifactEntry { name: "repository/1.snapshot.json".into() },
                            ArtifactEntry { name: "repository/1.targets.json".into() },
                            ArtifactEntry { name: "repository/root.json".into() },
                            ArtifactEntry { name: "repository/snapshot.json".into() },
                            ArtifactEntry { name: "repository/targets/package1/2008b04d3e1c6a116619b4989973a1cee19d1fad3d89365cf2b020e65cd870d7.0".into() },
                            ArtifactEntry { name: "repository/targets/package2/1b0e8a06a242d49fbcdf24fa6bd1f8c0f2606afacafb47ba37bb1c45e700cce6.0".into() },
                            ArtifactEntry { name: "repository/targets.json".into() },
                            ArtifactEntry { name: "repository/timestamp.json".into() },
                            ArtifactEntry { name: "virtual_devices/manifest.json".into() },
                            ArtifactEntry { name: "virtual_devices/virtual_device_A.json".into() },
                            ArtifactEntry { name: "virtual_devices/virtual_device_B.json".into() },
                            ArtifactEntry { name: "virtual_devices/virtual_device_C.json".into() },
                            ArtifactEntry { name: "zbi".into() },
                        ]
                    },
                ]
            }),
        );

        // These were previously generated and should not be created now.
        assert!(!tempdir.join("all_blobs.json").exists());
        assert!(!tempdir.join("images.json").exists());
        assert!(!tempdir.join("targets.json").exists());
    }
}
