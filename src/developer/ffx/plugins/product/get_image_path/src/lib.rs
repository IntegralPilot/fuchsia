// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! FFX plugin for get a path of the image inside product bundle.
use assembled_system::Image;
use async_trait::async_trait;
use camino::{Utf8Path, Utf8PathBuf};
use ffx_config::EnvironmentContext;
use ffx_writer::VerifiedMachineWriter;
use fho::{Error, FfxMain, FfxTool, Result, bug, return_user_error, user_error};
use product_bundle::ProductBundle;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use utf8_path::path_relative_from;

mod args;
pub use args::{GetImagePathCommand, ImageType, Slot};

/// CommandStatus is returned to indicate exit status of
/// a command. The Ok variant returns the list of artifacts.
#[derive(Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CommandStatus {
    /// Successful execution with an optional informational string.
    Ok { path: String },
    /// Unexpected error with string.
    UnexpectedError { message: String },
    /// A known kind of error that can be reported usefully to the user
    UserError { message: String },
}

/// This plugin will get the path of image from the product bundle, based on the slot and image_type passed in.
#[derive(FfxTool)]
#[target(None)]
pub struct PbGetImagePathTool {
    #[command]
    pub cmd: GetImagePathCommand,
    env: EnvironmentContext,
}

#[async_trait(?Send)]
impl FfxMain for PbGetImagePathTool {
    type Writer = VerifiedMachineWriter<CommandStatus>;

    type Error = ::fho::Error;

    async fn main(mut self, mut writer: Self::Writer) -> fho::Result<()> {
        // Set the product bundle path from config if it was not passed in.
        if self.cmd.product_bundle.is_none() {
            if let Some(default_path) =
                self.env.get::<Option<String>, _>("product.path").map_err(|e| bug!(e))?
            {
                self.cmd.product_bundle = Some(default_path.into());
            } else {
                let message = String::from("no product bundle specified nor configured.");
                writer
                    .machine(&CommandStatus::UserError { message: message.clone() })
                    .map_err(Into::<Error>::into)?;
                return_user_error!(message);
            }
        }
        match self.pb_get_image_path().await {
            Ok(Some(image_path)) => writer
                .machine_or(
                    &CommandStatus::Ok { path: image_path.to_string() },
                    image_path.to_string(),
                )
                .map_err(Into::into),
            Ok(None) => {
                let e = user_error!("No image found based on the specified options");
                writer.machine(&CommandStatus::UserError { message: e.to_string() })?;
                Err(e)
            }
            Err(Error::User(e)) => {
                writer.machine(&CommandStatus::UserError { message: e.to_string() })?;
                Err(Error::User(e))
            }
            Err(err) => {
                writer.machine(&CommandStatus::UnexpectedError { message: err.to_string() })?;
                Err(err)
            }
        }
    }
}

impl PbGetImagePathTool {
    pub async fn pb_get_image_path(&self) -> Result<Option<Utf8PathBuf>> {
        // It is expected that self.cmd.product_bundle has been validated to be some() by this point.
        let product_bundle =
            ProductBundle::try_load_from(&self.cmd.product_bundle.as_ref().unwrap())
                .map_err(|e| user_error!("Failed to load product bundle: {}", e))?;
        self.extract_image_path(product_bundle)
    }

    fn compute_path(&self, artifact_path: &Utf8Path) -> Result<Utf8PathBuf> {
        if self.cmd.relative_path {
            path_relative_from(artifact_path, &self.cmd.product_bundle.as_ref().unwrap())
                .map_err(Into::into)
        } else {
            Ok(artifact_path.into())
        }
    }

    fn extract_image_path(&self, product_bundle: ProductBundle) -> Result<Option<Utf8PathBuf>> {
        let product_bundle = match product_bundle {
            ProductBundle::V2(pb) => pb,
        };

        if self.cmd.bootloader.is_some() && self.cmd.slot.is_some() {
            return_user_error!("Cannot pass in both --bootloader and --slot parameters");
        }

        if let Some(bootloader) = &self.cmd.bootloader {
            let bootloader_type = bootloader
                .strip_prefix("firmware")
                .ok_or(user_error!("Bootloader name must begin with 'firmware'"))?
                .trim_start_matches("_");
            return product_bundle
                .partitions
                .bootloader_partitions
                .iter()
                .find_map(|b| {
                    if b.partition_type == bootloader_type {
                        Some(self.compute_path(&b.image))
                    } else {
                        None
                    }
                })
                .transpose();
        }

        let system = match self.cmd.slot {
            Some(Slot::A) => product_bundle.system_a,
            Some(Slot::B) => product_bundle.system_b,
            Some(Slot::R) => product_bundle.system_r,
            _ => return_user_error!("No valid slot passed in {:#?}", self.cmd.slot),
        }
        .ok_or(user_error!("No image found in slot"))?;

        system
            .iter()
            .find_map(|i| match (i, self.cmd.image_type) {
                (Image::ZBI { path, .. }, Some(ImageType::Zbi))
                | (Image::VBMeta(path), Some(ImageType::VBMeta))
                | (Image::FVM(path), Some(ImageType::Fvm))
                | (Image::FxfsSparse { path, .. }, Some(ImageType::FxfsFastboot))
                | (Image::QemuKernel(path), Some(ImageType::QemuKernel))
                | (Image::Dtbo(path), Some(ImageType::Dtbo)) => Some(self.compute_path(path)),
                _ => None,
            })
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assembled_system::BlobfsContents;
    use assembly_container::AssemblyContainer;
    use assembly_partitions_config::PartitionsConfig;

    use ffx_writer::{Format, TestBuffers};
    use product_bundle::ProductBundleV2;
    use std::fs;
    use std::fs::File;
    use std::io::Write;
    use tempfile::tempdir;

    #[fuchsia::test]
    async fn test_get_image_path() {
        let env = ffx_config::test_init().expect("test env");
        let tmp = tempdir().unwrap();
        let tempdir = Utf8Path::from_path(tmp.path()).unwrap().canonicalize_utf8().unwrap();

        let json = r#"
            {
                bootloader_partitions: [
                    {
                        type: "bl2",
                        name: "bl2",
                        image: "bootloader_bl2",
                    },
                    {
                        type: "",
                        name: "bootloader",
                        image: "bootloader",
                    }
                ],
                partitions: [
                    {
                        type: "ZBI",
                        name: "zircon_a",
                        slot: "A",
                    },
                    {
                        type: "VBMeta",
                        name: "vbmeta_b",
                        slot: "B",
                    },
                    {
                        type: "FVM",
                        name: "fvm",
                    },
                ],
                hardware_revision: "hw",
                unlock_credentials: [
                    "credential.zip",
                ],
            }
        "#;
        let partitions_config_path = tempdir.join("partitions_config.json");
        File::create(partitions_config_path.as_path()).unwrap().write_all(json.as_bytes()).unwrap();

        let create_temp_file = |name: &str| {
            let path = tempdir.join(name);
            let mut file = File::create(path).unwrap();
            write!(file, "{}", name).unwrap();
        };

        create_temp_file("bootloader_bl2");
        create_temp_file("bootloader");
        create_temp_file("credential.zip");

        let config = PartitionsConfig::from_dir(&tempdir).unwrap();

        let pb = ProductBundle::V2(ProductBundleV2 {
            product_name: "".to_string(),
            product_version: "".to_string(),
            partitions: config,
            sdk_version: "".to_string(),
            system_a: Some(vec![
                Image::ZBI { path: Utf8PathBuf::from("zbi/path"), signed: false },
                Image::FVM(Utf8PathBuf::from(tempdir.join("system_a/fvm.blk"))),
                Image::QemuKernel(Utf8PathBuf::from("qemu/path")),
                Image::FxfsSparse {
                    path: Utf8PathBuf::from("system_a/fxfs.sparse.blk"),
                    contents: BlobfsContents::default(),
                },
                Image::Dtbo(Utf8PathBuf::from("system_a/sorrel-dtbo.img")),
            ]),
            system_b: None,
            system_r: None,
            platform_tools_a: vec![],
            platform_tools_b: vec![],
            platform_tools_r: vec![],
            repositories: vec![],
            update_package_hash: None,
            virtual_devices_path: None,
            release_info: None,
        });

        // Each test case is in its own scope
        {
            let tool = PbGetImagePathTool {
                cmd: GetImagePathCommand {
                    product_bundle: None,
                    slot: Some(Slot::A),
                    image_type: Some(ImageType::Zbi),
                    relative_path: false,
                    bootloader: None,
                },
                env: env.context.clone(),
            };
            let path = tool.extract_image_path(pb.clone()).unwrap().unwrap();
            let expected_path = Utf8PathBuf::from("zbi/path");
            assert_eq!(expected_path, path);
        }
        {
            let tool = PbGetImagePathTool {
                cmd: GetImagePathCommand {
                    product_bundle: None,
                    slot: Some(Slot::A),
                    image_type: Some(ImageType::QemuKernel),
                    relative_path: false,
                    bootloader: None,
                },
                env: env.context.clone(),
            };
            let path = tool.extract_image_path(pb.clone()).unwrap().unwrap();
            let expected_path = Utf8PathBuf::from("qemu/path");
            assert_eq!(expected_path, path);
        }
        {
            let tool = PbGetImagePathTool {
                cmd: GetImagePathCommand {
                    product_bundle: None,
                    slot: Some(Slot::A),
                    image_type: Some(ImageType::Dtbo),
                    relative_path: false,
                    bootloader: None,
                },
                env: env.context.clone(),
            };
            let path = tool.extract_image_path(pb.clone()).unwrap().unwrap();
            let expected_path = Utf8PathBuf::from("system_a/sorrel-dtbo.img");
            assert_eq!(expected_path, path);
        }
        {
            let tool = PbGetImagePathTool {
                cmd: GetImagePathCommand {
                    product_bundle: Some(tempdir.clone()),
                    slot: Some(Slot::A),
                    image_type: Some(ImageType::Fvm),
                    relative_path: true,
                    bootloader: None,
                },
                env: env.context.clone(),
            };
            let path = tool.extract_image_path(pb.clone()).unwrap().unwrap();
            let expected_path = Utf8PathBuf::from("system_a/fvm.blk");
            assert_eq!(expected_path, path);
        }
        {
            let tool = PbGetImagePathTool {
                cmd: GetImagePathCommand {
                    product_bundle: Some(tempdir.clone()),
                    slot: Some(Slot::A),
                    image_type: Some(ImageType::FxfsFastboot),
                    relative_path: false,
                    bootloader: None,
                },
                env: env.context.clone(),
            };
            let path = tool.extract_image_path(pb.clone()).unwrap().unwrap();
            let expected_path = Utf8PathBuf::from("system_a/fxfs.sparse.blk");
            assert_eq!(expected_path, path);
        }
        {
            let tool = PbGetImagePathTool {
                cmd: GetImagePathCommand {
                    product_bundle: Some(tempdir.clone()),
                    slot: None,
                    image_type: None,
                    relative_path: true,
                    bootloader: Some(String::from("firmware_bl2")),
                },
                env: env.context.clone(),
            };
            let path = tool.extract_image_path(pb.clone()).unwrap().unwrap();
            let expected_path = Utf8PathBuf::from("bootloader_bl2");
            assert_eq!(expected_path, path);
        }
        {
            let tool = PbGetImagePathTool {
                cmd: GetImagePathCommand {
                    product_bundle: Some(tempdir.clone()),
                    slot: None,
                    image_type: None,
                    relative_path: true,
                    bootloader: Some(String::from("firmware")),
                },
                env: env.context.clone(),
            };
            let path = tool.extract_image_path(pb.clone()).unwrap().unwrap();
            let expected_path = Utf8PathBuf::from("bootloader");
            assert_eq!(expected_path, path);
        }
    }

    #[fuchsia::test]
    async fn test_get_image_path_not_found() {
        let env = ffx_config::test_init().expect("test env");
        let pb = ProductBundle::V2(ProductBundleV2 {
            product_name: "".to_string(),
            product_version: "".to_string(),
            partitions: PartitionsConfig::default(),
            sdk_version: "".to_string(),
            system_a: Some(vec![
                Image::ZBI { path: Utf8PathBuf::from("zbi/path"), signed: false },
                Image::FVM(Utf8PathBuf::from("fvm/path")),
                Image::QemuKernel(Utf8PathBuf::from("qemu/path")),
            ]),
            system_b: None,
            system_r: None,
            platform_tools_a: vec![],
            platform_tools_b: vec![],
            platform_tools_r: vec![],
            repositories: vec![],
            update_package_hash: None,
            virtual_devices_path: None,
            release_info: None,
        });
        let tool = PbGetImagePathTool {
            cmd: GetImagePathCommand {
                product_bundle: None,
                slot: Some(Slot::A),
                image_type: Some(ImageType::VBMeta),
                relative_path: false,
                bootloader: None,
            },
            env: env.context.clone(),
        };
        let path = tool.extract_image_path(pb).unwrap();
        assert_eq!(None, path);
    }

    #[fuchsia::test]
    async fn test_get_image_path_not_found_machine() {
        let env = ffx_config::test_init().expect("test env");
        let pb_path = env.isolate_root.path().join("test_bundle");
        fs::create_dir_all(&pb_path).expect("create test bundle dir");

        let pb = ProductBundle::V2(ProductBundleV2 {
            product_name: "".to_string(),
            product_version: "".to_string(),
            partitions: PartitionsConfig::default(),
            sdk_version: "".to_string(),
            system_a: Some(vec![
                Image::ZBI {
                    path: Utf8PathBuf::from_path_buf(pb_path.join("zbi/path")).expect("utf8 path"),
                    signed: false,
                },
                Image::FVM(
                    Utf8PathBuf::from_path_buf(pb_path.join("fvm/path")).expect("utf8 path"),
                ),
                Image::QemuKernel(
                    Utf8PathBuf::from_path_buf(pb_path.join("qemu/path")).expect("utf8 path"),
                ),
            ]),
            system_b: None,
            system_r: None,
            platform_tools_a: vec![],
            platform_tools_b: vec![],
            platform_tools_r: vec![],
            repositories: vec![],
            update_package_hash: None,
            virtual_devices_path: None,
            release_info: None,
        });
        pb.write(Utf8Path::from_path(&pb_path).expect("temp dir to utf8 path"))
            .expect("temp test product bundle");

        let tool = PbGetImagePathTool {
            cmd: GetImagePathCommand {
                product_bundle: Some(Utf8PathBuf::from_path_buf(pb_path).expect("utf8 path")),
                slot: Some(Slot::A),
                image_type: Some(ImageType::VBMeta),
                relative_path: false,
                bootloader: None,
            },
            env: env.context.clone(),
        };
        let test_buffers = TestBuffers::default();
        let writer: <PbGetImagePathTool as FfxMain>::Writer =
            <PbGetImagePathTool as FfxMain>::Writer::new_test(
                Some(Format::JsonPretty),
                &test_buffers,
            );

        let result = tool.main(writer).await;

        assert!(result.is_err(), "Expect result to be an error");
        let raw = test_buffers.into_stdout_str();

        let got: CommandStatus = serde_json::from_str(&raw).expect("parse output");
        let want = CommandStatus::UserError {
            message: "No image found based on the specified options".to_string(),
        };
        assert_eq!(got, want);
        let data: serde_json::Value = serde_json::from_str(&raw).expect("serde value");
        match <PbGetImagePathTool as FfxMain>::Writer::verify_schema(&data) {
            Ok(_) => (),
            Err(e) => {
                assert_eq!("", format!("Error verifying schema: {e:?}\n{data:?}"));
            }
        };
    }

    #[fuchsia::test]
    async fn test_get_image_path_machine() {
        let env = ffx_config::test_init().expect("test env");
        let pb_path = env.isolate_root.path().join("test_bundle");
        fs::create_dir_all(&pb_path).expect("create test bundle dir");

        let pb = ProductBundle::V2(ProductBundleV2 {
            product_name: "".to_string(),
            product_version: "".to_string(),
            partitions: PartitionsConfig::default(),
            sdk_version: "".to_string(),
            system_a: Some(vec![
                Image::ZBI {
                    path: Utf8PathBuf::from_path_buf(pb_path.join("zbi/path")).expect("utf8 path"),
                    signed: false,
                },
                Image::FVM(
                    Utf8PathBuf::from_path_buf(pb_path.join("fvm/path")).expect("utf8 path"),
                ),
                Image::QemuKernel(
                    Utf8PathBuf::from_path_buf(pb_path.join("qemu/path")).expect("utf8 path"),
                ),
            ]),
            system_b: None,
            system_r: None,
            platform_tools_a: vec![],
            platform_tools_b: vec![],
            platform_tools_r: vec![],
            repositories: vec![],
            update_package_hash: None,
            virtual_devices_path: None,
            release_info: None,
        });
        pb.write(Utf8Path::from_path(&pb_path).expect("temp dir to utf8 path"))
            .expect("temp test product bundle");

        let tool = PbGetImagePathTool {
            cmd: GetImagePathCommand {
                product_bundle: Some(Utf8Path::from_path(&pb_path).expect("utf8 path").into()),
                slot: Some(Slot::A),
                image_type: Some(ImageType::Zbi),
                relative_path: false,
                bootloader: None,
            },
            env: env.context.clone(),
        };
        let test_buffers = TestBuffers::default();
        let writer: <PbGetImagePathTool as FfxMain>::Writer =
            <PbGetImagePathTool as FfxMain>::Writer::new_test(
                Some(Format::JsonPretty),
                &test_buffers,
            );

        let result = tool.main(writer).await;

        assert!(result.is_ok(), "Expect result to be ok");
        let raw = test_buffers.into_stdout_str();

        let got: CommandStatus = serde_json::from_str(&raw).expect("parse output");
        let want =
            CommandStatus::Ok { path: pb_path.join("zbi/path").to_string_lossy().to_string() };
        assert_eq!(got, want);
        let data: serde_json::Value = serde_json::from_str(&raw).expect("serde value");
        match <PbGetImagePathTool as FfxMain>::Writer::verify_schema(&data) {
            Ok(_) => (),
            Err(e) => {
                assert_eq!("", format!("Error verifying schema: {e:?}\n{data:?}"));
            }
        }
    }

    #[fuchsia::test]
    async fn test_get_image_path_from_config() {
        let mut builder = ffx_config::test_env();
        let isolate_root = builder.isolate_root();
        let pb_path = isolate_root.join("test_bundle");
        fs::create_dir_all(&pb_path).expect("create test bundle dir");

        let env = builder
            .user_config("product.path", pb_path.to_string_lossy())
            .build()
            .expect("test env");

        let pb = ProductBundle::V2(ProductBundleV2 {
            product_name: "".to_string(),
            product_version: "".to_string(),
            partitions: PartitionsConfig::default(),
            sdk_version: "".to_string(),
            system_a: Some(vec![
                Image::ZBI {
                    path: Utf8PathBuf::from_path_buf(pb_path.join("zbi/path")).expect("utf8 path"),
                    signed: false,
                },
                Image::FVM(
                    Utf8PathBuf::from_path_buf(pb_path.join("fvm/path")).expect("utf8 path"),
                ),
                Image::QemuKernel(
                    Utf8PathBuf::from_path_buf(pb_path.join("qemu/path")).expect("utf8 path"),
                ),
            ]),
            system_b: None,
            system_r: None,
            platform_tools_a: vec![],
            platform_tools_b: vec![],
            platform_tools_r: vec![],
            repositories: vec![],
            update_package_hash: None,
            virtual_devices_path: None,
            release_info: None,
        });
        pb.write(Utf8Path::from_path(&pb_path).expect("temp dir to utf8 path"))
            .expect("temp test product bundle");

        let tool = PbGetImagePathTool {
            cmd: GetImagePathCommand {
                product_bundle: None,
                slot: Some(Slot::A),
                image_type: Some(ImageType::Zbi),
                relative_path: false,
                bootloader: None,
            },
            env: env.context.clone(),
        };
        let test_buffers = TestBuffers::default();
        let writer: <PbGetImagePathTool as FfxMain>::Writer =
            <PbGetImagePathTool as FfxMain>::Writer::new_test(
                Some(Format::JsonPretty),
                &test_buffers,
            );

        let result = tool.main(writer).await;

        assert!(result.is_ok(), "Expect result to be ok: {}", test_buffers.into_stderr_str());
        let raw = test_buffers.into_stdout_str();

        let got: CommandStatus = serde_json::from_str(&raw).expect("parse output");
        let want =
            CommandStatus::Ok { path: pb_path.join("zbi/path").to_string_lossy().to_string() };
        assert_eq!(got, want);
    }
}
