// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use ffx_package_ota_manifest_generate_args::GenerateCommand;
use ffx_writer::SimpleWriter;
use fho::{FfxContext as _, FfxMain, FfxTool, Result};
use update_package::manifest::{AssetType, Image, ImageType, Slot};

#[derive(FfxTool)]
#[target(None)]
pub struct GenerateTool {
    #[command]
    pub cmd: GenerateCommand,
}

fho::embedded_plugin!(GenerateTool);

fn replace_or_add_asset_image(
    images: &mut Vec<Image>,
    path: &(impl AsRef<std::path::Path> + std::fmt::Display),
    asset_type: AssetType,
) -> Result<()> {
    let image = Image::from_path(path, Slot::AB, ImageType::Asset(asset_type))
        .with_user_message(|| format!("Reading {asset_type:?} image: {path}"))?;
    for img in images.iter_mut() {
        if img.image_type == ImageType::Asset(asset_type) && img.slot == Slot::AB {
            *img = image;
            return Ok(());
        }
    }
    images.push(image);
    Ok(())
}

#[async_trait(?Send)]
impl FfxMain for GenerateTool {
    type Writer = SimpleWriter;

    type Error = fho::Error;

    async fn main(self, _writer: SimpleWriter) -> Result<()> {
        let manifest_path = if self.cmd.target.is_dir() {
            let pb = product_bundle::ProductBundle::try_load_from(&self.cmd.target)
                .with_user_message(|| format!("Loading product bundle from {}", self.cmd.target))?;
            match pb {
                product_bundle::ProductBundle::V2(pb) => {
                    let repo = pb
                        .repositories
                        .first()
                        .user_message("Product bundle has no repositories")?;
                    repo.ota_manifest_path.clone().user_message(
                        "Product bundle repository does not specify ota_manifest_path",
                    )?
                }
            }
        } else {
            self.cmd.target
        };

        let manifest_bytes = std::fs::read(&manifest_path)
            .with_user_message(|| format!("Reading input manifest file: {manifest_path}"))?;
        let raw = update_package::signed_manifest::parse_raw(&manifest_bytes)
            .user_message("Parsing input signed manifest")?;
        let mut manifest = update_package::manifest::parse_ota_manifest(raw.manifest_payload)
            .user_message("Parsing OTA manifest payload")?;

        if let Some(url) = self.cmd.blob_base_url {
            manifest.blob_base_url = url;
        }
        if let Some(vbmeta_path) = self.cmd.vbmeta {
            replace_or_add_asset_image(&mut manifest.images, &vbmeta_path, AssetType::Vbmeta)?;
        }
        if let Some(zbi_path) = self.cmd.zbi {
            replace_or_add_asset_image(&mut manifest.images, &zbi_path, AssetType::Zbi)?;
        }

        let key_pair = if let Some(key_path) = &self.cmd.key {
            let key_bytes = std::fs::read(key_path)
                .with_user_message(|| format!("Reading private key: {key_path}"))?;
            let pem = pem::parse(&key_bytes);
            let pkcs8 = match pem {
                Ok(ref pem) => pem.contents(),
                Err(_) => &key_bytes,
            };
            ring::signature::Ed25519KeyPair::from_pkcs8_maybe_unchecked(pkcs8)
                .map_err(|e| anyhow::anyhow!("{e}"))
                .user_message("Parsing pkcs8 private key")?
        } else {
            let pem = pem::parse(update_package::MANIFEST_DEV_KEY_PEM)
                .bug_context("Parsing default dev pem")?;
            ring::signature::Ed25519KeyPair::from_pkcs8_maybe_unchecked(pem.contents())
                .map_err(|e| anyhow::anyhow!("{e}"))
                .bug_context("Parsing default dev pkcs8")?
        };

        let key_signature = if let Some(sig_path) = &self.cmd.key_signature {
            Some(
                std::fs::read(sig_path)
                    .with_user_message(|| format!("Reading key signature: {sig_path}"))?,
            )
        } else {
            None
        };

        let out_bytes = update_package::signed_manifest::generate(
            manifest,
            &key_pair,
            key_signature.as_deref(),
        )
        .user_message("Generating signed manifest")?;

        if let Some(parent) = self.cmd.output.parent() {
            if !parent.as_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .with_user_message(|| format!("Creating directory: {parent}"))?;
            }
        }
        std::fs::write(&self.cmd.output, out_bytes)
            .with_user_message(|| format!("Writing new manifest file: {}", self.cmd.output))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8PathBuf;
    use ring::signature::KeyPair as _;
    use tempfile::NamedTempFile;
    use update_package::UpdateMode;
    use update_package::manifest::{Blob, OtaManifest};

    fn make_ota_manifest() -> OtaManifest {
        OtaManifest {
            product_bundle_version: "1.2.3.4".parse().unwrap(),
            board: "test-board".to_string(),
            epoch: 1,
            mode: UpdateMode::Normal,
            blob_base_url: "http://example.com".to_string(),
            images: vec![
                Image {
                    slot: Slot::AB,
                    image_type: ImageType::Firmware("test-fw".to_string()),
                    blob: Blob {
                        uncompressed_size: 1234,
                        fuchsia_merkle_root: "1".repeat(64).parse().unwrap(),
                    },
                },
                Image {
                    slot: Slot::AB,
                    image_type: ImageType::Asset(AssetType::Zbi),
                    blob: Blob {
                        uncompressed_size: 100,
                        fuchsia_merkle_root: "3".repeat(64).parse().unwrap(),
                    },
                },
                Image {
                    slot: Slot::AB,
                    image_type: ImageType::Asset(AssetType::Vbmeta),
                    blob: Blob {
                        uncompressed_size: 200,
                        fuchsia_merkle_root: "4".repeat(64).parse().unwrap(),
                    },
                },
            ],
            blobs: vec![Blob {
                uncompressed_size: 5678,
                fuchsia_merkle_root: "2".repeat(64).parse().unwrap(),
            }],
        }
    }

    fn make_keypair() -> ring::signature::Ed25519KeyPair {
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = ring::signature::Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        ring::signature::Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap()
    }

    fn parse_and_verify(bytes: &[u8]) -> OtaManifest {
        let pem = pem::parse(update_package::MANIFEST_DEV_KEY_PEM).unwrap();
        let keypair =
            ring::signature::Ed25519KeyPair::from_pkcs8_maybe_unchecked(pem.contents()).unwrap();
        let public_key = ring::signature::UnparsedPublicKey::new(
            &ring::signature::ED25519,
            keypair.public_key().as_ref().to_vec(),
        );
        update_package::signed_manifest::parse_and_verify(bytes, &[public_key]).unwrap()
    }

    #[fuchsia::test]
    async fn test_generate_modify_blob_base_url() {
        let keypair = make_keypair();
        let manifest = make_ota_manifest();
        let in_bytes =
            update_package::signed_manifest::generate(manifest.clone(), &keypair, None).unwrap();

        let in_file = NamedTempFile::new().unwrap();
        std::fs::write(in_file.path(), &in_bytes).unwrap();
        let in_path = Utf8PathBuf::try_from(in_file.path().to_path_buf()).unwrap();

        let out_file = NamedTempFile::new().unwrap();
        let out_path = Utf8PathBuf::try_from(out_file.path().to_path_buf()).unwrap();

        let cmd = GenerateCommand {
            target: in_path,
            output: out_path.clone(),
            blob_base_url: Some("blobs/1".to_string()),
            vbmeta: None,
            zbi: None,
            key: None,
            key_signature: None,
        };
        let tool = GenerateTool { cmd };
        let test_buffers = ffx_writer::TestBuffers::default();
        let writer = ffx_writer::SimpleWriter::new_test(&test_buffers);

        tool.main(writer).await.unwrap();

        let out_bytes = std::fs::read(&out_path).unwrap();
        let parsed = parse_and_verify(&out_bytes);

        let mut expected = manifest;
        expected.blob_base_url = "blobs/1".to_string();
        assert_eq!(parsed, expected);
    }

    #[fuchsia::test]
    async fn test_generate_from_product_bundle() {
        let tmp = tempfile::tempdir().unwrap();
        let pb_dir = camino::Utf8Path::from_path(tmp.path()).unwrap().to_path_buf();
        let metadata_dir = pb_dir.join("repository");
        std::fs::create_dir_all(&metadata_dir).unwrap();

        let keypair = make_keypair();
        let manifest = make_ota_manifest();
        let in_bytes =
            update_package::signed_manifest::generate(manifest.clone(), &keypair, None).unwrap();
        let ota_manifest_path = metadata_dir.join("ota_manifest");
        std::fs::write(&ota_manifest_path, &in_bytes).unwrap();

        let pb = product_bundle::ProductBundle::V2(product_bundle::ProductBundleV2 {
            product_name: "test".into(),
            product_version: "test".into(),
            partitions: assembly_partitions_config::PartitionsConfig::default(),
            sdk_version: "test".into(),
            system_a: None,
            system_b: None,
            system_r: None,
            platform_tools_a: vec![],
            platform_tools_b: vec![],
            platform_tools_r: vec![],
            repositories: vec![product_bundle::Repository {
                name: "fuchsia.com".into(),
                metadata_path: metadata_dir,
                blobs_path: pb_dir.join("blobs"),
                delivery_blob_type: 1,
                root_private_key_path: None,
                targets_private_key_path: None,
                snapshot_private_key_path: None,
                timestamp_private_key_path: None,
                ota_manifest_signature_path: None,
                ota_manifest_path: Some(ota_manifest_path),
            }],
            update_package_hash: None,
            virtual_devices_path: None,
            release_info: None,
        });
        pb.write(&pb_dir).unwrap();

        let out_file = NamedTempFile::new().unwrap();
        let out_path = camino::Utf8PathBuf::try_from(out_file.path().to_path_buf()).unwrap();

        let cmd = GenerateCommand {
            target: pb_dir,
            output: out_path.clone(),
            blob_base_url: Some("blobs/2".to_string()),
            vbmeta: None,
            zbi: None,
            key: None,
            key_signature: None,
        };
        let tool = GenerateTool { cmd };
        let test_buffers = ffx_writer::TestBuffers::default();
        let writer = ffx_writer::SimpleWriter::new_test(&test_buffers);

        tool.main(writer).await.unwrap();

        let out_bytes = std::fs::read(&out_path).unwrap();
        let parsed = parse_and_verify(&out_bytes);

        let mut expected = manifest;
        expected.blob_base_url = "blobs/2".to_string();
        assert_eq!(parsed, expected);
    }

    #[fuchsia::test]
    async fn test_generate_with_key_signature() {
        let keypair = make_keypair();
        let manifest = make_ota_manifest();
        let in_bytes =
            update_package::signed_manifest::generate(manifest.clone(), &keypair, None).unwrap();

        let in_file = NamedTempFile::new().unwrap();
        std::fs::write(in_file.path(), &in_bytes).unwrap();
        let in_path = Utf8PathBuf::try_from(in_file.path().to_path_buf()).unwrap();

        let sig_file = NamedTempFile::new().unwrap();
        let fake_sig = vec![0x42u8; 64];
        std::fs::write(sig_file.path(), &fake_sig).unwrap();
        let sig_path = Utf8PathBuf::try_from(sig_file.path().to_path_buf()).unwrap();

        let out_file = NamedTempFile::new().unwrap();
        let out_path = Utf8PathBuf::try_from(out_file.path().to_path_buf()).unwrap();

        let cmd = GenerateCommand {
            target: in_path,
            output: out_path.clone(),
            blob_base_url: None,
            vbmeta: None,
            zbi: None,
            key: None,
            key_signature: Some(sig_path),
        };
        let tool = GenerateTool { cmd };
        let test_buffers = ffx_writer::TestBuffers::default();
        let writer = ffx_writer::SimpleWriter::new_test(&test_buffers);

        tool.main(writer).await.unwrap();

        let out_bytes = std::fs::read(&out_path).unwrap();
        let raw = update_package::signed_manifest::parse_raw(&out_bytes).unwrap();
        assert_eq!(raw.signatures.manifest_key_signature, fake_sig);
    }

    #[fuchsia::test]
    async fn test_generate_replace_vbmeta_and_zbi() {
        let keypair = make_keypair();
        let mut manifest = make_ota_manifest();
        let recovery_zbi = Image {
            slot: Slot::R,
            image_type: ImageType::Asset(AssetType::Zbi),
            blob: Blob {
                uncompressed_size: 50,
                fuchsia_merkle_root: "5".repeat(64).parse().unwrap(),
            },
        };
        manifest.images.push(recovery_zbi.clone());

        let in_bytes =
            update_package::signed_manifest::generate(manifest.clone(), &keypair, None).unwrap();

        let in_file = NamedTempFile::new().unwrap();
        std::fs::write(in_file.path(), &in_bytes).unwrap();
        let in_path = Utf8PathBuf::try_from(in_file.path().to_path_buf()).unwrap();

        let new_zbi_file = NamedTempFile::new().unwrap();
        std::fs::write(new_zbi_file.path(), b"new zbi content").unwrap();
        let new_zbi_path = Utf8PathBuf::try_from(new_zbi_file.path().to_path_buf()).unwrap();

        let new_vbmeta_file = NamedTempFile::new().unwrap();
        std::fs::write(new_vbmeta_file.path(), b"new vbmeta content").unwrap();
        let new_vbmeta_path = Utf8PathBuf::try_from(new_vbmeta_file.path().to_path_buf()).unwrap();

        let out_file = NamedTempFile::new().unwrap();
        let out_path = Utf8PathBuf::try_from(out_file.path().to_path_buf()).unwrap();

        let cmd = GenerateCommand {
            target: in_path,
            output: out_path.clone(),
            blob_base_url: None,
            vbmeta: Some(new_vbmeta_path.clone()),
            zbi: Some(new_zbi_path.clone()),
            key: None,
            key_signature: None,
        };
        let tool = GenerateTool { cmd };
        let test_buffers = ffx_writer::TestBuffers::default();
        let writer = ffx_writer::SimpleWriter::new_test(&test_buffers);

        tool.main(writer).await.unwrap();

        let out_bytes = std::fs::read(&out_path).unwrap();
        let parsed = parse_and_verify(&out_bytes);

        let expected_zbi =
            Image::from_path(&new_zbi_path, Slot::AB, ImageType::Asset(AssetType::Zbi)).unwrap();
        let zbi_img = parsed
            .images
            .iter()
            .find(|i| i.image_type == ImageType::Asset(AssetType::Zbi) && i.slot == Slot::AB)
            .unwrap();
        assert_eq!(zbi_img, &expected_zbi);

        // recovery zbi is not changed
        assert!(parsed.images.contains(&recovery_zbi));

        let expected_vbmeta =
            Image::from_path(&new_vbmeta_path, Slot::AB, ImageType::Asset(AssetType::Vbmeta))
                .unwrap();
        let vbmeta_img = parsed
            .images
            .iter()
            .find(|i| i.image_type == ImageType::Asset(AssetType::Vbmeta) && i.slot == Slot::AB)
            .unwrap();
        assert_eq!(vbmeta_img, &expected_vbmeta);
    }
}
