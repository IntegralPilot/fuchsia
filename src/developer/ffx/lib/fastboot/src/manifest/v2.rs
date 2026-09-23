// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::common::crypto::unlock_device;
use crate::common::{
    Boot, Flash, Unlock, finish, flash_bootloader, flash_product, is_locked, lock_device,
    verify_hardware,
};
use crate::error::FfxFastbootError;
use crate::file_resolver::FileResolver;
use crate::unlock::unlock;
use crate::util::Event;

type Result<T> = std::result::Result<T, FfxFastbootError>;
use assembly_partitions_config::UploadMethod;
use async_trait::async_trait;
use ffx_fastboot_interface::fastboot_interface::FastbootInterface;
use ffx_flash_manifest::ManifestParams;
use ffx_flash_manifest::v2::FlashManifest;
use tokio::sync::mpsc::Sender;

#[async_trait]
impl Flash for FlashManifest {
    async fn flash<F, T>(
        &self,
        messenger: &Sender<Event>,
        file_resolver: &mut F,
        fastboot_interface: &mut T,
        cmd: ManifestParams,
        ssh_key_upload_method: Option<&UploadMethod>,
    ) -> Result<()>
    where
        F: FileResolver + Sync + Send,
        T: FastbootInterface,
    {
        if !cmd.skip_verify {
            verify_hardware(&self.hw_revision, &self.product_matches, fastboot_interface).await?;
        }
        let product = match self.v1.0.iter().find(|product| product.name == cmd.product) {
            Some(res) => res,
            None => return Err(FfxFastbootError::MissingProduct(cmd.product.clone())),
        };
        if product.requires_unlock && is_locked(fastboot_interface).await? {
            if self.credentials.len() == 0 {
                return Err(FfxFastbootError::MissingCredentials);
            } else {
                unlock_device(messenger, file_resolver, &self.credentials, fastboot_interface)
                    .await?;
            }
        }
        messenger
            .send(Event::FlashProduct {
                product_name: product.name.clone(),
                partition_count: product.bootloader_partitions.len() + product.partitions.len(),
            })
            .await?;
        let set_active_performed =
            flash_bootloader(&messenger, file_resolver, product, fastboot_interface, &cmd).await?;
        if product.requires_unlock && !is_locked(fastboot_interface).await? {
            lock_device(fastboot_interface).await?;
        }
        flash_product(
            &messenger,
            file_resolver,
            product,
            fastboot_interface,
            &cmd,
            ssh_key_upload_method,
        )
        .await?;
        finish(fastboot_interface, set_active_performed).await
    }
}

#[async_trait]
impl Unlock for FlashManifest {
    async fn unlock<F, T>(
        &self,
        messenger: &Sender<Event>,
        file_resolver: &mut F,
        fastboot_interface: &mut T,
    ) -> Result<()>
    where
        F: FileResolver + Sync + Send,
        T: FastbootInterface,
    {
        unlock(messenger.clone(), file_resolver, &self.credentials, fastboot_interface).await
    }
}

#[async_trait]
impl Boot for FlashManifest {
    async fn boot<F, T>(
        &self,
        messenger: Sender<Event>,
        file_resolver: &mut F,
        slot: String,
        fastboot_interface: &mut T,
        cmd: ManifestParams,
    ) -> Result<()>
    where
        F: FileResolver + Sync + Send,
        T: FastbootInterface,
    {
        self.v1.boot(messenger, file_resolver, slot, fastboot_interface, cmd).await
    }
}

////////////////////////////////////////////////////////////////////////////////
// tests

#[cfg(test)]
mod test {
    use super::*;
    type Result<T> = std::result::Result<T, anyhow::Error>;
    use crate::common::vars::{IS_USERSPACE_VAR, LOCKED_VAR, MAX_DOWNLOAD_SIZE_VAR, REVISION_VAR};
    use crate::file_resolver::test::TestResolver;
    use ffx_fastboot_interface::test::setup;
    use ffx_flash_manifest::{BootParams, Command};
    use serde_json::{from_str, json};
    use std::path::PathBuf;
    use tempfile::NamedTempFile;
    use tokio::sync::mpsc;

    const MISMATCH_MANIFEST: &'static str = r#"{
        "hw_revision": "mismatch",
        "products": [
            {
                "name": "zedboot",
                "bootloader_partitions": [],
                "partitions": [
                    ["test1", "path1"],
                    ["test2", "path2"],
                    ["test3", "path3"],
                    ["test4", "path4"],
                    ["test5", "path5"]
                ],
                "oem_files": []
            }
        ]
    }"#;

    const NO_CREDS_MANIFEST: &'static str = r#"{
        "hw_revision": "zedboot",
        "products": [
            {
                "name": "zedboot",
                "requires_unlock": true,
                "bootloader_partitions": [],
                "partitions": [
                    ["test1", "path1"],
                    ["test2", "path2"],
                    ["test3", "path3"],
                    ["test4", "path4"],
                    ["test5", "path5"]
                ],
                "oem_files": []
            }
        ]
    }"#;

    #[fuchsia::test]
    async fn test_matching_revision_should_work() -> Result<()> {
        let tmp_file = NamedTempFile::new().expect("tmp access failed");
        let tmp_file_name = tmp_file.path().to_string_lossy().to_string();

        // Setup image files for flashing
        let tmp_img_files = [(); 5].map(|_| NamedTempFile::new().expect("tmp access failed"));
        let tmp_img_file_paths = tmp_img_files
            .iter()
            .map(|tmp| tmp.path().to_str().expect("non-unicode tmp path"))
            .collect::<Vec<&str>>();

        let manifest = json!({
            "hw_revision": "rev_test",
            "products": [
                {
                    "name": "zedboot",
                    "bootloader_partitions": [],
                    "partitions": [
                        ["test1",tmp_img_file_paths[0] ],
                        ["test2",tmp_img_file_paths[1] ],
                        ["test3",tmp_img_file_paths[2] ],
                        ["test4",tmp_img_file_paths[3] ],
                        ["test5",tmp_img_file_paths[4] ]
                    ],
                    "oem_files": []
                }
            ]
        });
        let v: FlashManifest = from_str(&manifest.to_string())?;

        let (state, mut proxy) = setup();
        {
            let mut state = state.lock().unwrap();
            state.set_var(IS_USERSPACE_VAR.to_string(), "yes".to_string());
            state.set_var(REVISION_VAR.to_string(), "rev_test-b4".to_string());
            state.set_var(MAX_DOWNLOAD_SIZE_VAR.to_string(), "8192".to_string());
        }
        let (client, _server) = mpsc::channel(100);
        v.flash(
            &client,
            &mut TestResolver::new(),
            &mut proxy,
            ManifestParams {
                manifest: Some(PathBuf::from(tmp_file_name)),
                product: "zedboot".to_string(),
                ..Default::default()
            },
            None,
        )
        .await?;
        Ok(())
    }

    #[fuchsia::test]
    async fn test_mismatching_revision_should_err() -> Result<()> {
        let v: FlashManifest = from_str(MISMATCH_MANIFEST)?;
        let tmp_file = NamedTempFile::new().expect("tmp access failed");
        let tmp_file_name = tmp_file.path().to_string_lossy().to_string();
        let (state, mut proxy) = setup();
        {
            let mut state = state.lock().unwrap();
            state.set_var(IS_USERSPACE_VAR.to_string(), "yes".to_string());
            state.set_var(REVISION_VAR.to_string(), "test".to_string());
        }
        let (client, _server) = mpsc::channel(100);
        assert!(
            v.flash(
                &client,
                &mut TestResolver::new(),
                &mut proxy,
                ManifestParams {
                    manifest: Some(PathBuf::from(tmp_file_name)),
                    product: "zedboot".to_string(),
                    ..Default::default()
                },
                None,
            )
            .await
            .is_err()
        );
        Ok(())
    }

    #[fuchsia::test]
    async fn test_no_creds_and_requires_unlock_should_err() -> Result<()> {
        let v: FlashManifest = from_str(NO_CREDS_MANIFEST)?;
        let tmp_file = NamedTempFile::new().expect("tmp access failed");
        let tmp_file_name = tmp_file.path().to_string_lossy().to_string();
        let (state, mut proxy) = setup();
        {
            let mut state = state.lock().unwrap();
            state.set_var(IS_USERSPACE_VAR.to_string(), "no".to_string());
            state.set_var(REVISION_VAR.to_string(), "zedboot".to_string());
            state.set_var(LOCKED_VAR.to_string(), "yes".to_string());
        }
        let (client, _server) = mpsc::channel(100);
        assert!(
            v.flash(
                &client,
                &mut TestResolver::new(),
                &mut proxy,
                ManifestParams {
                    manifest: Some(PathBuf::from(tmp_file_name)),
                    product: "zedboot".to_string(),
                    ..Default::default()
                },
                None,
            )
            .await
            .is_err()
        );
        Ok(())
    }

    #[fuchsia::test]
    async fn test_minimal_manifest_succeeds() -> Result<()> {
        let tmp_file = NamedTempFile::new().expect("tmp access failed");
        let tmp_file_name = tmp_file.path().to_string_lossy().to_string();

        let tmp_img_files = [(); 4].map(|_| NamedTempFile::new().expect("tmp access failed"));
        let tmp_img_file_paths = tmp_img_files
            .iter()
            .map(|tmp| tmp.path().to_str().expect("non-unicode tmp path"))
            .collect::<Vec<&str>>();

        let manifest = json!({
            "hw_revision": "zedboot",
            "products": [
                {
                    "name": "zedboot",
                    "requires_unlock": false,
                    "bootloader_partitions": [],
                    "partitions": [
                        ["zircon_a",tmp_img_file_paths[0] ],
                        ["zircon_b",tmp_img_file_paths[1] ],
                        ["vbmeta_a",tmp_img_file_paths[2] ],
                        ["vbmeta_b",tmp_img_file_paths[3] ]
                    ],
                    "oem_files": []
                }
            ]
        });

        let v: FlashManifest = from_str(&manifest.to_string())?;
        let (state, mut proxy) = setup();
        {
            let mut state = state.lock().unwrap();
            state.set_var(IS_USERSPACE_VAR.to_string(), "yes".to_string());
            state.set_var(REVISION_VAR.to_string(), "zedboot".to_string());
            state.set_var(MAX_DOWNLOAD_SIZE_VAR.to_string(), "8192".to_string());
        }
        let (client, _server) = mpsc::channel(100);
        v.flash(
            &client,
            &mut TestResolver::new(),
            &mut proxy,
            ManifestParams {
                manifest: Some(PathBuf::from(tmp_file_name)),
                product: "zedboot".to_string(),
                op: Command::Boot(BootParams { zbi: None, vbmeta: None, slot: "a".to_string() }),
                ..Default::default()
            },
            None,
        )
        .await?;

        let state = state.lock().unwrap();
        assert_eq!(state.bootloader_reboots, 1);
        assert_eq!(state.set_actives, vec!["a".to_string()]);
        assert_eq!(state.continue_boots, 1);
        Ok(())
    }

    #[fuchsia::test]
    async fn test_flash_with_bootloader_partitions_sets_active_only_once() -> Result<()> {
        let tmp_file = NamedTempFile::new().expect("tmp access failed");
        let tmp_file_name = tmp_file.path().to_string_lossy().to_string();

        let bootloader_file = NamedTempFile::new().expect("tmp access failed");
        let bootloader_path = bootloader_file.path().to_str().expect("non-unicode tmp path");
        let partition_file = NamedTempFile::new().expect("tmp access failed");
        let partition_path = partition_file.path().to_str().expect("non-unicode tmp path");

        let manifest = json!({
            "hw_revision": "zedboot",
            "products": [
                {
                    "name": "zedboot",
                    "requires_unlock": false,
                    "bootloader_partitions": [
                        ["bootloader", bootloader_path]
                    ],
                    "partitions": [
                        ["zircon", partition_path]
                    ],
                    "oem_files": []
                }
            ]
        });

        let v: FlashManifest = from_str(&manifest.to_string())?;
        let (state, mut proxy) = setup();
        {
            let mut state = state.lock().unwrap();
            state.set_var(IS_USERSPACE_VAR.to_string(), "no".to_string());
            state.set_var(REVISION_VAR.to_string(), "zedboot".to_string());
            state.set_var(MAX_DOWNLOAD_SIZE_VAR.to_string(), "8192".to_string());
        }

        let (client, _server) = mpsc::channel(100);
        v.flash(
            &client,
            &mut TestResolver::new(),
            &mut proxy,
            ManifestParams {
                manifest: Some(PathBuf::from(tmp_file_name)),
                product: "zedboot".to_string(),
                ..Default::default()
            },
            None,
        )
        .await?;

        let state = state.lock().unwrap();
        assert_eq!(state.bootloader_reboots, 1);
        assert_eq!(state.set_actives, vec!["a".to_string()]);
        assert_eq!(state.continue_boots, 1);
        Ok(())
    }
}
