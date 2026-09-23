// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::boot::boot;
use crate::common::{
    Boot, Flash, Partition as PartitionTrait, Product as ProductTrait, Unlock, flash_and_reboot,
    is_locked,
};
use crate::error::FfxFastbootError;
use crate::file_resolver::FileResolver;
use crate::util::Event;

type Result<T> = std::result::Result<T, FfxFastbootError>;
use assembly_partitions_config::UploadMethod;
use async_trait::async_trait;
use ffx_fastboot_interface::fastboot_interface::FastbootInterface;
use ffx_flash_manifest::v1::{FlashManifest, Partition, Product};
use ffx_flash_manifest::{ManifestParams, OemFile};
use futures::try_join;
use tokio::sync::mpsc::{self, Sender};

impl ProductTrait<Partition> for Product {
    fn name(&self) -> &String {
        &self.name
    }

    fn bootloader_partitions(&self) -> &Vec<Partition> {
        &self.bootloader_partitions
    }

    fn partitions(&self) -> &Vec<Partition> {
        &self.partitions
    }

    fn oem_files(&self) -> &Vec<OemFile> {
        &self.oem_files
    }
}

impl PartitionTrait for Partition {
    fn name(&self) -> &str {
        self.name()
    }

    fn file(&self) -> &str {
        self.file()
    }

    fn variable(&self) -> Option<&str> {
        self.variable()
    }

    fn variable_value(&self) -> Option<&str> {
        self.variable_value()
    }
}

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
        let product = match self.0.iter().find(|product| product.name == cmd.product) {
            Some(res) => res,
            None => return Err(FfxFastbootError::MissingProduct(cmd.product.clone())),
        };
        if product.requires_unlock && is_locked(fastboot_interface).await? {
            return Err(FfxFastbootError::UnlockRequired);
        }
        flash_and_reboot(
            messenger,
            file_resolver,
            product,
            fastboot_interface,
            cmd,
            ssh_key_upload_method,
        )
        .await
    }
}

#[async_trait]
impl Unlock for FlashManifest {}

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
        let product = match self.0.iter().find(|product| product.name == cmd.product) {
            Some(res) => res,
            None => return Err(FfxFastbootError::MissingProduct(cmd.product.clone())),
        };
        let partitions: Vec<&Partition> = product
            .partitions
            .iter()
            .filter(|p| p.name().ends_with(&format!("_{}", slot)))
            .collect();
        let zbi =
            partitions.iter().find(|p| p.name().contains("zircon")).map(|p| p.file().to_string());
        let boot_img = partitions
            .iter()
            .find(|p| p.name() == format!("boot_{}", slot))
            .map(|p| p.file().to_string());
        let vbmeta =
            partitions.iter().find(|p| p.name().contains("vbmeta")).map(|p| p.file().to_string());
        match zbi.or(boot_img) {
            Some(z) => {
                let (up_client, mut up_server) = mpsc::channel(100);
                try_join!(boot(up_client, file_resolver, z, vbmeta, fastboot_interface), async {
                    loop {
                        match up_server.recv().await {
                            Some(u) => messenger.send(Event::Upload(u)).await?,
                            None => {
                                return Ok(());
                            }
                        }
                    }
                })?;
                Ok(())
            }
            None => Err(FfxFastbootError::MatchingPartitionsNotFound {
                slot: slot.clone(),
                missing: "zbi".to_string(),
            }),
        }
    }
}

////////////////////////////////////////////////////////////////////////////////
// tests

#[cfg(test)]
mod test {
    use super::*;
    type Result<T> = std::result::Result<T, anyhow::Error>;
    use crate::common::vars::{IS_USERSPACE_VAR, LOCKED_VAR, MAX_DOWNLOAD_SIZE_VAR};
    use crate::file_resolver::test::TestResolver;
    use ffx_fastboot_interface::test::setup;
    use serde_json::{from_str, json};
    use std::path::PathBuf;
    use tempfile::NamedTempFile;
    use tokio::sync::mpsc;

    const MANIFEST: &'static str = r#"[
        {
            "name": "zedboot",
            "bootloader_partitions": [
                ["test1", "path1"],
                ["test2", "path2"]
            ],
            "partitions": [
                ["test1", "path1"],
                ["test2", "path2"],
                ["test3", "path3"],
                ["test4", "path4"],
                ["test5", "path5"]
            ],
            "oem_files": [
                ["test1", "path1"],
                ["test2", "path2"]
            ]
        },
        {
            "name": "fuchsia",
            "bootloader_partitions": [],
            "partitions": [
                ["test10", "path10"],
                ["test20", "path20"],
                ["test30", "path30"]
            ],
            "oem_files": []
        }
    ]"#;

    const LOCKED_MANIFEST: &'static str = r#"[
        {
            "name": "zedboot",
            "bootloader_partitions": [
                ["btest1", "bpath1", "var1", "value1"]
            ],
            "partitions": [],
            "oem_files": [],
            "requires_unlock": true
        }
    ]"#;

    #[fuchsia::test]
    async fn test_deserializing_should_work() -> Result<()> {
        let v: FlashManifest = from_str(MANIFEST)?;
        let zedboot: &Product = &v.0[0];
        assert_eq!("zedboot", zedboot.name);
        assert_eq!(2, zedboot.bootloader_partitions.len());
        let bootloader_expected = [["test1", "path1"], ["test2", "path2"]];
        for x in 0..bootloader_expected.len() {
            assert_eq!(zedboot.bootloader_partitions[x].name(), bootloader_expected[x][0]);
            assert_eq!(zedboot.bootloader_partitions[x].file(), bootloader_expected[x][1]);
        }
        assert_eq!(5, zedboot.partitions.len());
        let expected = [
            ["test1", "path1"],
            ["test2", "path2"],
            ["test3", "path3"],
            ["test4", "path4"],
            ["test5", "path5"],
        ];
        for x in 0..expected.len() {
            assert_eq!(zedboot.partitions[x].name(), expected[x][0]);
            assert_eq!(zedboot.partitions[x].file(), expected[x][1]);
        }
        assert_eq!(2, zedboot.oem_files.len());
        let oem_files_expected = [["test1", "path1"], ["test2", "path2"]];
        for x in 0..oem_files_expected.len() {
            assert_eq!(zedboot.oem_files[x].command(), oem_files_expected[x][0]);
            assert_eq!(zedboot.oem_files[x].file(), oem_files_expected[x][1]);
        }
        let product: &Product = &v.0[1];
        assert_eq!("fuchsia", product.name);
        assert_eq!(0, product.bootloader_partitions.len());
        assert_eq!(3, product.partitions.len());
        let expected2 = [["test10", "path10"], ["test20", "path20"], ["test30", "path30"]];
        for x in 0..expected2.len() {
            assert_eq!(product.partitions[x].name(), expected2[x][0]);
            assert_eq!(product.partitions[x].file(), expected2[x][1]);
        }
        assert_eq!(0, product.oem_files.len());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_should_fail_if_product_missing() -> Result<()> {
        let tmp_file = NamedTempFile::new().expect("tmp access failed");
        let tmp_file_name = tmp_file.path().to_string_lossy().to_string();
        let v: FlashManifest = from_str(MANIFEST)?;
        let (_, mut proxy) = setup();
        let (client, _server) = mpsc::channel(100);
        assert!(
            v.flash(
                &client,
                &mut TestResolver::new(),
                &mut proxy,
                ManifestParams {
                    manifest: Some(PathBuf::from(tmp_file_name)),
                    product: "Unknown".to_string(),
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
    async fn test_should_succeed_if_product_found() -> Result<()> {
        let tmp_file = NamedTempFile::new().expect("tmp access failed");
        let tmp_file_name = tmp_file.path().to_string_lossy().to_string();

        // Setup image files for flashing
        let tmp_img_files = [(); 3].map(|_| NamedTempFile::new().expect("tmp access failed"));
        let tmp_img_file_paths = tmp_img_files
            .iter()
            .map(|tmp| tmp.path().to_str().expect("non-unicode tmp path"))
            .collect::<Vec<&str>>();

        let manifest = json!([
            {
                "name": "zedboot",
                "bootloader_partitions": [
                    ["test1", "path1"],
                    ["test2", "path2"]
                ],
                "partitions": [
                    ["test1", "path1"],
                    ["test2", "path2"],
                    ["test3", "path3"],
                    ["test4", "path4"],
                    ["test5", "path5"]
                ],
                "oem_files": [
                    ["test1", "path1"],
                    ["test2", "path2"]
                ]
            },
            {
                "name": "fuchsia",
                "bootloader_partitions": [],
                "partitions": [
                    ["test10",tmp_img_file_paths[0]],
                    ["test20",tmp_img_file_paths[1]],
                    ["test30",tmp_img_file_paths[2]]
                ],
                "oem_files": []
            }
        ]);

        let v: FlashManifest = from_str(&manifest.to_string())?;

        let (state, mut proxy) = setup();
        {
            let mut state = state.lock().unwrap();
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
                product: "fuchsia".to_string(),
                ..Default::default()
            },
            None,
        )
        .await?;
        Ok(())
    }

    #[fuchsia::test]
    async fn test_oem_file_should_be_staged_from_command() -> Result<()> {
        let test_oem_cmd = "test-oem-cmd";
        let tmp_file = NamedTempFile::new().expect("tmp access failed");
        let tmp_file_name = tmp_file.path().to_string_lossy().to_string();
        let test_staged_file = format!("{},{}", test_oem_cmd, tmp_file_name).parse::<OemFile>()?;
        let manifest_file = NamedTempFile::new().expect("tmp access failed");
        let manifest_file_name = manifest_file.path().to_string_lossy().to_string();

        // Set up the temporary files to read
        let tmp_img_files = [(); 5].map(|_| NamedTempFile::new().expect("tmp access failed"));
        let tmp_img_file_paths = tmp_img_files
            .iter()
            .map(|tmp| tmp.path().to_str().expect("non-unicode tmp path"))
            .collect::<Vec<&str>>();

        let manifest = json!([
                {
                    "name": "fuchsia",
                    "bootloader_partitions": [],
                    "partitions": [
                        ["test1", tmp_img_file_paths[0]],
                        ["test2", tmp_img_file_paths[1]],
                        ["test3", tmp_img_file_paths[2]],
                        ["test4", tmp_img_file_paths[3]],
                        ["test5", tmp_img_file_paths[4]]
                    ],
                    "oem_files": []
                }
        ]);

        let v: FlashManifest = from_str(&manifest.to_string())?;
        let (state, mut proxy) = setup();
        {
            let mut state = state.lock().unwrap();
            state.set_var(IS_USERSPACE_VAR.to_string(), "yes".to_string());
            state.set_var(MAX_DOWNLOAD_SIZE_VAR.to_string(), "8192".to_string());
        }
        let (client, _server) = mpsc::channel(100);
        v.flash(
            &client,
            &mut TestResolver::new(),
            &mut proxy,
            ManifestParams {
                manifest: Some(PathBuf::from(manifest_file_name)),
                product: "fuchsia".to_string(),
                oem_stage: vec![test_staged_file],
                ..Default::default()
            },
            None,
        )
        .await?;
        let state = state.lock().unwrap();
        assert_eq!(1, state.staged_files.len());
        assert_eq!(1, state.oem_commands.len());
        assert_eq!(format!("oem {}", test_oem_cmd), state.oem_commands[0]);
        Ok(())
    }

    #[fuchsia::test]
    async fn test_should_upload_conditional_partitions_that_match() -> Result<()> {
        let tmp_file = NamedTempFile::new().expect("tmp access failed");
        let tmp_file_name = tmp_file.path().to_string_lossy().to_string();

        // Setup images to flash
        let temp_image_file_2 = NamedTempFile::new().expect("tmp access failed");
        let tmp_image_file_2_name = temp_image_file_2.path().to_string_lossy().to_string();
        let manifest = json!([
                {
                    "name": "zedboot",
                    "bootloader_partitions": [
                        ["btest1", "bpath1", "var1", "value1"],
                        ["btest2", tmp_image_file_2_name, "var2", "value2"],
                        ["btest3", "bpath3", "var3", "value3"]
                    ],
                    "partitions": [],
                    "oem_files": []
                }
        ]);
        let v: FlashManifest = from_str(&manifest.to_string())?;

        let (state, mut proxy) = setup();
        {
            let mut state = state.lock().unwrap();
            state.set_var(IS_USERSPACE_VAR.to_string(), "no".to_string());
            state.set_var("var1".to_string(), "not_value1".to_string());
            state.set_var("var2".to_string(), "value2".to_string());
            state.set_var("var3".to_string(), "not_value3".to_string());
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
    async fn test_should_succeed_and_not_reboot_bootloader() -> Result<()> {
        let tmp_file = NamedTempFile::new().expect("tmp access failed");
        let tmp_file_name = tmp_file.path().to_string_lossy().to_string();

        let tmp_img_files = [(); 3].map(|_| NamedTempFile::new().expect("tmp access failed"));
        let tmp_img_file_paths = tmp_img_files
            .iter()
            .map(|tmp| tmp.path().to_str().expect("non-unicode tmp path"))
            .collect::<Vec<&str>>();

        let manifest = json!([
                {
                    "name": "zedboot",
                    "bootloader_partitions": [
                        ["test1", "path1"],
                        ["test2", "path2"]
                    ],
                    "partitions": [
                        ["test1", "path1"],
                        ["test2", "path2"],
                        ["test3", "path3"],
                        ["test4", "path4"],
                        ["test5", "path5"]
                    ],
                    "oem_files": [
                        ["test1", "path1"],
                        ["test2", "path2"]
                    ]
                },
                {
                    "name": "fuchsia",
                    "bootloader_partitions": [],
                    "partitions": [
                        ["test10", tmp_img_file_paths[0]],
                        ["test20", tmp_img_file_paths[1]],
                        ["test30", tmp_img_file_paths[2]]
                    ],
                    "oem_files": []
                }
            ]
        );

        let v: FlashManifest = from_str(&manifest.to_string())?;
        let (state, mut proxy) = setup();
        {
            let mut state = state.lock().unwrap();
            state.set_var(IS_USERSPACE_VAR.to_string(), "no".to_string());
            state.set_var(MAX_DOWNLOAD_SIZE_VAR.to_string(), "8192".to_string());
        }

        let (client, _server) = mpsc::channel(100);
        v.flash(
            &client,
            &mut TestResolver::new(),
            &mut proxy,
            ManifestParams {
                manifest: Some(PathBuf::from(tmp_file_name)),
                product: "fuchsia".to_string(),
                no_bootloader_reboot: true,
                ..Default::default()
            },
            None,
        )
        .await?;
        let state = state.lock().unwrap();
        assert_eq!(0, state.bootloader_reboots);
        Ok(())
    }

    #[fuchsia::test]
    async fn test_should_not_flash_if_target_is_locked_and_product_requires_unlock() -> Result<()> {
        let v: FlashManifest = from_str(LOCKED_MANIFEST)?;
        let tmp_file = NamedTempFile::new().expect("tmp access failed");
        let tmp_file_name = tmp_file.path().to_string_lossy().to_string();
        let (state, mut proxy) = setup();
        {
            let mut state = state.lock().unwrap();
            state.set_var(LOCKED_VAR.to_string(), "vx-locked".to_string());
            state.set_var(LOCKED_VAR.to_string(), "yes".to_string());
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
        assert_eq!(true, res.is_err());
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

        let manifest = json!([
            {
                "name": "zedboot",
                "bootloader_partitions": [
                    ["bootloader", bootloader_path]
                ],
                "partitions": [
                    ["zircon", partition_path]
                ],
                "oem_files": []
            }
        ]);

        let v: FlashManifest = from_str(&manifest.to_string())?;
        let (state, mut proxy) = setup();
        {
            let mut state = state.lock().unwrap();
            state.set_var(IS_USERSPACE_VAR.to_string(), "no".to_string());
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
