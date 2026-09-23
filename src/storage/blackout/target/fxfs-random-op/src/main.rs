// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use async_trait::async_trait;
use blackout_target::random_op::{Op, generate_load};
use blackout_target::{Test, TestServer, find_partition, set_up_partition};
use fidl::endpoints::{ClientEnd, Proxy as _};
use fidl_fuchsia_fs_startup::{CheckOptions, CreateOptions, MountOptions};
use fidl_fuchsia_fxfs::{CryptManagementMarker, CryptMarker, KeyPurpose};
use fs_management::Fxfs;
use fs_management::filesystem::Filesystem;
use fuchsia_component::client::connect_to_protocol;
use rand::{Rng, RngExt as _};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const BARRIERS_ENABLED: &'static str = std::env!("BARRIERS_ENABLED");

const DATA_KEY: [u8; 32] = [
    0xcf, 0x9e, 0x45, 0x2a, 0x22, 0xa5, 0x70, 0x31, 0x33, 0x3b, 0x4d, 0x6b, 0x6f, 0x78, 0x58, 0x29,
    0x04, 0x79, 0xc7, 0xd6, 0xa9, 0x4b, 0xce, 0x82, 0x04, 0x56, 0x5e, 0x82, 0xfc, 0xe7, 0x37, 0xa8,
];

const METADATA_KEY: [u8; 32] = [
    0x0f, 0x4d, 0xca, 0x6b, 0x35, 0x0e, 0x85, 0x6a, 0xb3, 0x8c, 0xdd, 0xe9, 0xda, 0x0e, 0xc8, 0x22,
    0x8e, 0xea, 0xd8, 0x05, 0xc4, 0xc9, 0x0b, 0xa8, 0xd8, 0x85, 0x87, 0x50, 0x75, 0x40, 0x1c, 0x4c,
];

struct OpSampler;

impl blackout_target::random_op::OpSampler for OpSampler {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> Op {
        match rng.random_range(0..100) {
            0..25 => Op::Allocate,
            25..50 => Op::Write,
            50..75 => Op::Truncate,
            75..95 => Op::Reopen,
            95..100 => Op::Replace,
            _ => unreachable!(),
        }
    }
}

#[derive(Copy, Clone)]
struct FxfsRandomOp {
    barriers_enabled: bool,
}

impl FxfsRandomOp {
    fn connect_to_crypt_service(&self) -> Result<ClientEnd<CryptMarker>> {
        Ok(connect_to_protocol::<CryptMarker>()?.into_client_end().unwrap())
    }

    async fn setup_crypt_service(&self) -> Result<ClientEnd<CryptMarker>> {
        static INITIALIZED: AtomicBool = AtomicBool::new(false);
        if INITIALIZED.load(Ordering::SeqCst) {
            return self.connect_to_crypt_service();
        }
        let crypt_management = connect_to_protocol::<CryptManagementMarker>()?;
        let wrapping_key_id_0 = [0; 16];
        let mut wrapping_key_id_1 = [0; 16];
        wrapping_key_id_1[0] = 1;
        crypt_management
            .add_wrapping_key(&wrapping_key_id_0, &DATA_KEY)
            .await?
            .map_err(zx::Status::err_from_raw)?;
        crypt_management
            .add_wrapping_key(&wrapping_key_id_1, &METADATA_KEY)
            .await?
            .map_err(zx::Status::err_from_raw)?;
        crypt_management
            .set_active_key(KeyPurpose::Data, &wrapping_key_id_0)
            .await?
            .map_err(zx::Status::err_from_raw)?;
        crypt_management
            .set_active_key(KeyPurpose::Metadata, &wrapping_key_id_1)
            .await?
            .map_err(zx::Status::err_from_raw)?;
        INITIALIZED.store(true, Ordering::SeqCst);
        self.connect_to_crypt_service()
    }
}

#[async_trait]
impl Test for FxfsRandomOp {
    async fn setup(
        self: Arc<Self>,
        device_label: String,
        _device_path: Option<String>,
        _seed: u64,
    ) -> Result<()> {
        log::info!(device_label:%; "setting up");
        let block_connector = set_up_partition(device_label).await?;

        let mut fxfs = Filesystem::from_boxed_config(
            block_connector,
            Box::new(Fxfs { barriers_enabled: self.barriers_enabled, ..Default::default() }),
        );
        fxfs.format().await?;
        let fs = fxfs.serve_multi_volume().await?;
        let crypt = Some(self.setup_crypt_service().await?);
        fs.create_volume(
            "default",
            CreateOptions::default(),
            MountOptions { crypt, ..MountOptions::default() },
        )
        .await?;

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

        let fxfs = Filesystem::from_boxed_config(
            block_connector,
            Box::new(Fxfs { barriers_enabled: self.barriers_enabled, ..Default::default() }),
        );
        let fs = fxfs.serve_multi_volume().await.context("serve multi volume")?;
        let crypt = Some(self.setup_crypt_service().await.context("set up crypt service")?);
        let volume = fs
            .open_volume("default", MountOptions { crypt, ..MountOptions::default() })
            .await
            .context("open volume")?;
        let root = volume.root();
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

        let mut fxfs = Filesystem::from_boxed_config(
            block_connector,
            Box::new(Fxfs { barriers_enabled: self.barriers_enabled, ..Default::default() }),
        );
        fxfs.fsck().await.context("overall fsck")?;

        let fs = fxfs.serve_multi_volume().await.context("failed to serve")?;
        let crypt = Some(self.setup_crypt_service().await.context("set up crypt service")?);
        fs.check_volume("default", CheckOptions { crypt, ..Default::default() })
            .await
            .context("default volume check")?;

        log::info!("verification complete");
        Ok(())
    }
}

#[fuchsia::main(logging_tags = ["blackout", "fxfs-random-op"])]
async fn main() -> Result<()> {
    let barriers_enabled = BARRIERS_ENABLED == "true";
    let server = TestServer::new(FxfsRandomOp { barriers_enabled })?;
    server.serve().await;

    Ok(())
}
