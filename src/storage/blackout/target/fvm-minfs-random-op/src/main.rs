// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use async_trait::async_trait;
use blackout_target::random_op::{Op, generate_load};
use blackout_target::{Test, TestServer, find_partition, set_up_partition};
use crypt_policy::Policy;
use fidl_fuchsia_fs_startup::{CheckOptions, CreateOptions, MountOptions};
use fs_management::filesystem::Filesystem;
use fs_management::{DATA_TYPE_GUID, Fvm};
use rand::{Rng, RngExt as _};
use std::sync::Arc;
use zxcrypt_crypt::with_crypt_service;

struct OpSampler;

impl blackout_target::random_op::OpSampler for OpSampler {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> Op {
        match rng.random_range(0..100) {
            0..40 => Op::Write,
            40..75 => Op::Truncate,
            75..95 => Op::Reopen,
            95..100 => Op::Replace,
            _ => unreachable!(),
        }
    }
}

#[derive(Clone)]
struct FvmMinfsTest;

#[async_trait]
impl Test for FvmMinfsTest {
    async fn setup(
        self: Arc<Self>,
        device_label: String,
        _device_path: Option<String>,
        _seed: u64,
    ) -> Result<()> {
        log::info!(device_label:%; "setting up");
        let block_connector =
            set_up_partition(device_label).await.context("Failed to set up partition")?;
        let mut fvm = Filesystem::from_boxed_config(block_connector, Box::new(Fvm::default()));
        fvm.format().await.context("Failed to format FVM")?;
        let fs = fvm.serve_multi_volume().await.context("Failed to serve FVM")?;
        let minfs_vol = with_crypt_service(Policy::Null, |crypt| {
            fs.create_volume(
                "minfs",
                CreateOptions { type_guid: Some(DATA_TYPE_GUID), ..CreateOptions::default() },
                MountOptions {
                    crypt: Some(crypt),
                    uri: Some("#meta/minfs.cm".to_string()),
                    ..MountOptions::default()
                },
            )
        })
        .await
        .context("Failed to create minfs volume")?;
        minfs_vol.shutdown().await.context("Failed to shutdown minfs")?;
        fs.shutdown().await.context("Failed to shutdown")?;
        log::info!("setup complete");
        Ok(())
    }

    async fn test(
        self: Arc<Self>,
        device_label: String,
        _device_path: Option<String>,
        seed: u64,
    ) -> Result<()> {
        log::info!(device_label:%; "running load gen");
        let block_connector = find_partition(device_label).await.context("find partition")?;

        let fvm = Filesystem::from_boxed_config(block_connector, Box::new(Fvm::default()));
        let fs = fvm.serve_multi_volume().await?;
        let minfs = with_crypt_service(Policy::Null, |crypt| {
            fs.open_volume(
                "minfs",
                MountOptions {
                    crypt: Some(crypt),
                    uri: Some("#meta/minfs.cm".to_string()),
                    ..MountOptions::default()
                },
            )
        })
        .await?;
        let root = minfs.root();
        generate_load(seed, &OpSampler, root).await
    }

    async fn verify(
        self: Arc<Self>,
        device_label: String,
        _device_path: Option<String>,
        _seed: u64,
    ) -> Result<()> {
        log::info!(device_label:%; "verifying disk consistency");
        let block_connector = find_partition(device_label).await?;

        let fvm = Filesystem::from_boxed_config(block_connector, Box::new(Fvm::default()));
        let fs = fvm.serve_multi_volume().await?;
        with_crypt_service(Policy::Null, |crypt| {
            fs.check_volume(
                "minfs",
                CheckOptions {
                    crypt: Some(crypt),
                    uri: Some("#meta/minfs.cm".to_string()),
                    ..Default::default()
                },
            )
        })
        .await
        .context("Check failed")?;

        log::info!("verification complete");
        Ok(())
    }
}

#[fuchsia::main(logging_tags = ["blackout", "fvm-minfs"])]
async fn main() -> Result<()> {
    let server = TestServer::new(FvmMinfsTest)?;
    server.serve().await;

    Ok(())
}
