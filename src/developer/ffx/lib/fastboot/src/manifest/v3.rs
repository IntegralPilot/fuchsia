// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error::FfxFastbootError;
use crate::file_resolver::FileResolver;
use crate::manifest::{Boot, Flash, Unlock};
use crate::util::Event;

type Result<T> = std::result::Result<T, FfxFastbootError>;
use assembly_partitions_config::UploadMethod;
use async_trait::async_trait;
use ffx_fastboot_interface::fastboot_interface::FastbootInterface;
use ffx_flash_manifest::ManifestParams;
use ffx_flash_manifest::v2::FlashManifest as FlashManifestV2;
use ffx_flash_manifest::v3::FlashManifest;
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
        let v2: FlashManifestV2 = self.into();
        v2.flash(messenger, file_resolver, fastboot_interface, cmd, ssh_key_upload_method).await
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
        let v2: FlashManifestV2 = self.into();
        v2.unlock(messenger, file_resolver, fastboot_interface).await
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
        let v2: FlashManifestV2 = self.into();
        v2.boot(messenger, file_resolver, slot, fastboot_interface, cmd).await
    }
}

////////////////////////////////////////////////////////////////////////////////
// tests
#[cfg(test)]
mod test {
    use super::*;
    type Result<T> = std::result::Result<T, anyhow::Error>;
    use crate::common::vars::{IS_USERSPACE_VAR, MAX_DOWNLOAD_SIZE_VAR, PRODUCT_VAR, REVISION_VAR};
    use crate::file_resolver::test::TestResolver;
    use ffx_fastboot_interface::test::setup;
    use serde_json::{from_str, json};
    use std::path::PathBuf;
    use tempfile::NamedTempFile;
    use tokio::sync::mpsc;

    #[fuchsia::test]
    async fn test_minimal_manifest_succeeds() -> Result<()> {
        let tmp_file = NamedTempFile::new().expect("tmp access failed");
        let tmp_file_name = tmp_file.path().to_string_lossy().to_string();

        // Setup image files for flashing
        let tmp_img_files = [(); 4].map(|_| NamedTempFile::new().expect("tmp access failed"));
        let tmp_img_file_paths = tmp_img_files
            .iter()
            .map(|tmp| tmp.path().to_str().expect("non-unicode tmp path"))
            .collect::<Vec<&str>>();

        let manifest = json!({
            "hw_revision": "rev_test",
            "products": [
                {
                    "name": "zedboot",
                    "partitions": [
                        {"name": "test1", "path":tmp_img_file_paths[0]  },
                        {"name": "test2", "path":tmp_img_file_paths[1]  },
                        {"name": "test3", "path":tmp_img_file_paths[2]  },
                        {
                            "name": "test4",
                            "path":tmp_img_file_paths[3],
                            "condition": {
                                "variable": "var",
                                "value": "val"
                            }
                        }
                    ]
                }
            ]
        });

        let v: FlashManifest = from_str(&manifest.to_string())?;
        let (state, mut proxy) = setup();
        {
            let mut state = state.lock().unwrap();
            state.set_var(REVISION_VAR.to_string(), "rev_test-b4".to_string());
            state.set_var(IS_USERSPACE_VAR.to_string(), "no".to_string());
            state.set_var("var".to_string(), "val".to_string());
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
    async fn test_full_manifest_succeeds() -> Result<()> {
        let tmp_file = NamedTempFile::new().expect("tmp access failed");
        let tmp_file_name = tmp_file.path().to_string_lossy().to_string();

        // Setup image files for flashing
        let tmp_img_files = [(); 4].map(|_| NamedTempFile::new().expect("tmp access failed"));
        let tmp_img_file_paths = tmp_img_files
            .iter()
            .map(|tmp| tmp.path().to_str().expect("non-unicode tmp path"))
            .collect::<Vec<&str>>();

        let manifest = json!({
            "hw_revision": "rev_test",
            "products": [
                {
                    "name": "zedboot",
                    "bootloader_partitions": [
                        {"name": "test1", "path": tmp_img_file_paths[0] },
                        {"name": "test2", "path": tmp_img_file_paths[1] },
                        {"name": "test3", "path": tmp_img_file_paths[2] },
                        {
                            "name": "test4",
                            "path":  tmp_img_file_paths[3] ,
                            "condition": {
                                "variable": "var",
                                "value": "val"
                            }
                        }
                    ],
                    "partitions": [
                        {"name": "test1", "path": tmp_img_file_paths[0] },
                        {"name": "test2", "path": tmp_img_file_paths[1] },
                        {"name": "test3", "path": tmp_img_file_paths[2] },
                        {"name": "test4", "path": tmp_img_file_paths[3] }
                    ],
                    "oem_files": [
                        {"command": "test1", "path": tmp_img_file_paths[0] },
                        {"command": "test2", "path": tmp_img_file_paths[1] },
                        {"command": "test3", "path": tmp_img_file_paths[2] },
                        {"command": "test4", "path": tmp_img_file_paths[3] }
                    ]
                }
            ]
        });

        let v: FlashManifest = from_str(&manifest.to_string())?;
        let (state, mut proxy) = setup();
        {
            let mut state = state.lock().unwrap();
            state.set_var("var".to_string(), "val".to_string());
            state.set_var(IS_USERSPACE_VAR.to_string(), "no".to_string());
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
    async fn test_product_match_succeeds() -> Result<()> {
        let tmp_file = NamedTempFile::new().expect("tmp access failed");
        let tmp_file_name = tmp_file.path().to_string_lossy().to_string();

        let tmp_img_files = [(); 1].map(|_| NamedTempFile::new().expect("tmp access failed"));
        let tmp_img_file_paths = tmp_img_files
            .iter()
            .map(|tmp| tmp.path().to_str().expect("non-unicode tmp path"))
            .collect::<Vec<&str>>();

        let manifest = json!({
            "hw_revision": "mismatch",
            "product_matches": ["product_test"],
            "products": [
                {
                    "name": "zedboot",
                    "partitions": [
                        {"name": "test1", "path": tmp_img_file_paths[0] }
                    ]
                }
            ]
        });

        let v: FlashManifest = from_str(&manifest.to_string())?;
        let (state, mut proxy) = setup();
        {
            let mut state = state.lock().unwrap();
            state.set_var(REVISION_VAR.to_string(), "wrong_rev".to_string());
            state.set_var(PRODUCT_VAR.to_string(), "product_test".to_string());
            state.set_var(IS_USERSPACE_VAR.to_string(), "yes".to_string());
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
    async fn test_product_match_fails() -> Result<()> {
        let tmp_file = NamedTempFile::new().expect("tmp access failed");
        let tmp_file_name = tmp_file.path().to_string_lossy().to_string();

        let tmp_img_files = [(); 1].map(|_| NamedTempFile::new().expect("tmp access failed"));
        let tmp_img_file_paths = tmp_img_files
            .iter()
            .map(|tmp| tmp.path().to_str().expect("non-unicode tmp path"))
            .collect::<Vec<&str>>();

        let manifest = json!({
            "hw_revision": "mismatch",
            "product_matches": ["product_test"],
            "products": [
                {
                    "name": "zedboot",
                    "partitions": [
                        {"name": "test1", "path": tmp_img_file_paths[0] }
                    ]
                }
            ]
        });

        let v: FlashManifest = from_str(&manifest.to_string())?;
        let (state, mut proxy) = setup();
        {
            let mut state = state.lock().unwrap();
            state.set_var(REVISION_VAR.to_string(), "wrong_rev".to_string());
            state.set_var(PRODUCT_VAR.to_string(), "wrong_product".to_string());
            state.set_var(IS_USERSPACE_VAR.to_string(), "yes".to_string());
        }
        let (client, _server) = mpsc::channel(100);
        let res = v
            .flash(
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
            .await;
        assert!(res.is_err());
        Ok(())
    }
}
