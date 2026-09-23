// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! FFX plugin to extract blobs from a product bundle.

use anyhow::{Result, anyhow};
use assembly_partitions_config::Slot;
use ffx_config::EnvironmentContext;
use fho::{FfxMain, FfxTool, Result as FhoResult, return_user_error};
use product_bundle::ProductBundle;
use std::io::Write;

mod args;
pub use args::ExtractBlobsCommand;

#[derive(FfxTool)]
pub struct PbExtractBlobsTool {
    #[command]
    pub cmd: ExtractBlobsCommand,
    env: EnvironmentContext,
}

#[async_trait::async_trait(?Send)]
impl FfxMain for PbExtractBlobsTool {
    type Writer = ffx_writer::SimpleWriter;
    type Error = ::fho::Error;

    async fn main(mut self, mut writer: Self::Writer) -> FhoResult<()> {
        // Set the product bundle path from config if it was not passed in.
        if self.cmd.product_bundle.is_none() {
            if let Some(default_path) =
                self.env.get::<Option<String>, _>("product.path").map_err(|e| anyhow!(e))?
            {
                self.cmd.product_bundle = Some(default_path.into());
            } else {
                return_user_error!("No product bundle specified nor configured.");
            }
        }

        let pb_path = self.cmd.product_bundle.as_ref().unwrap();
        let product_bundle = ProductBundle::try_load_from(pb_path)
            .map_err(|e| anyhow!("Failed to load product bundle from {:?}: {}", pb_path, e))?;

        let slot = match self.cmd.slot.to_uppercase().as_str() {
            "A" => Slot::A,
            "B" => Slot::B,
            "R" => Slot::R,
            _ => return_user_error!("Invalid slot: {}. Must be A, B, or R", self.cmd.slot),
        };

        let out_dir = &self.cmd.out_dir;

        writeln!(writer, "Extracting blobs to {:?}", out_dir).map_err(|e| anyhow!(e))?;
        product_bundle
            .extract_blobs(slot, out_dir, self.cmd.delivery_blob_type)
            .map_err(|e| anyhow!("Failed to extract blobs: {}", e))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assembly_partitions_config::PartitionsConfig;
    use camino::Utf8Path;
    use product_bundle::{ProductBundleV2, Repository};
    use std::fs::File;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn test_copy_existing_blobs() {
        let tmp = tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();
        let pb_dir = dir.join("pb");
        let blobs_dir = pb_dir.join("blobs").join("1");
        std::fs::create_dir_all(&blobs_dir).unwrap();

        // Create a placeholder blob
        let blob_hash = "050907f009ff634f9aa57bff541fb9e9c2c62b587c23578e77637cda3bd69458";
        let mut blob_file = File::create(blobs_dir.join(blob_hash)).unwrap();
        let delivery_data =
            delivery_blob::generate(delivery_blob::DeliveryBlobType::Type1, b"fake blob contents");
        blob_file.write_all(&delivery_data).unwrap();

        let pb = ProductBundle::V2(ProductBundleV2 {
            product_name: "test".into(),
            product_version: "test".into(),
            partitions: PartitionsConfig::default(),
            sdk_version: "test".into(),
            system_a: None,
            system_b: None,
            system_r: None,
            platform_tools_a: vec![],
            platform_tools_b: vec![],
            platform_tools_r: vec![],
            repositories: vec![Repository {
                name: "fuchsia.com".into(),
                metadata_path: pb_dir.join("repository"),
                blobs_path: pb_dir.join("blobs"),
                delivery_blob_type: 1,
                root_private_key_path: None,
                targets_private_key_path: None,
                snapshot_private_key_path: None,
                timestamp_private_key_path: None,
                ota_manifest_signature_path: None,
                ota_manifest_path: None,
            }],
            update_package_hash: None,
            virtual_devices_path: None,
            release_info: None,
        });

        let out_dir = dir.join("out");
        pb.extract_blobs(Slot::A, &out_dir, None).unwrap();

        assert!(out_dir.join(blob_hash).exists());
        let content = std::fs::read_to_string(out_dir.join(blob_hash)).unwrap();
        assert_eq!(content, "fake blob contents");
    }

    #[fuchsia::test]
    async fn test_extract_blobs_subcommand_fxfs() {
        use std::os::unix::fs::PermissionsExt;
        let env = ffx_config::test_init().expect("test env");
        let tmp = tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();
        let pb_path = dir.join("pb");
        std::fs::create_dir_all(&pb_path).unwrap();

        // Create a placeholder fxfs_pbtool
        let tool_path = pb_path.join("fxfs_pbtool");
        {
            let mut file = File::create(&tool_path).unwrap();
            file.write_all(b"#!/bin/sh
out_dir=\"\"
while [ \"$#\" -gt 0 ]; do
    case \"$1\" in
        --out) out_dir=\"$2\"; shift 2;;
        *) shift 1;;
    esac
done
mkdir -p \"$out_dir\"
echo \"extracted blob contents\" > \"$out_dir/050907f009ff634f9aa57bff541fb9e9c2c62b587c23578e77637cda3bd69458\"
exit 0
").unwrap();
            let mut perms = std::fs::metadata(&tool_path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&tool_path, perms).unwrap();
        }

        let pb = ProductBundle::V2(ProductBundleV2 {
            product_name: "test".into(),
            product_version: "test".into(),
            partitions: PartitionsConfig::default(),
            sdk_version: "test".into(),
            system_a: Some(vec![assembled_system::Image::Fxfs(pb_path.join("fxfs.blk"))]),
            system_b: None,
            system_r: None,
            platform_tools_a: vec![tool_path],
            platform_tools_b: vec![],
            platform_tools_r: vec![],
            repositories: vec![],
            update_package_hash: None,
            virtual_devices_path: None,
            release_info: None,
        });
        pb.write(&pb_path).unwrap();

        let out_dir = dir.join("out");
        let tool = PbExtractBlobsTool {
            cmd: ExtractBlobsCommand {
                product_bundle: Some(pb_path),
                out_dir: out_dir.clone(),
                slot: "A".to_string(),
                delivery_blob_type: None,
            },
            env: env.context.clone(),
        };

        let test_buffers = ffx_writer::TestBuffers::default();
        let writer = ffx_writer::SimpleWriter::new_test(&test_buffers);
        tool.main(writer).await.unwrap();

        let blob_file =
            out_dir.join("050907f009ff634f9aa57bff541fb9e9c2c62b587c23578e77637cda3bd69458");
        assert!(blob_file.exists());
        let content = std::fs::read_to_string(blob_file).unwrap();
        assert_eq!(content, "extracted blob contents\n");
    }
}
