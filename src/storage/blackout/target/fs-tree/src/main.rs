// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use async_trait::async_trait;
use blackout_target::static_tree::{DirectoryEntry, EntryDistribution};
use blackout_target::{Test, TestServer, find_partition, set_up_partition};
use fidl::endpoints::{ClientEnd, Proxy as _};
use fidl_fuchsia_fs_startup::{CheckOptions, CreateOptions, MountOptions};
use fidl_fuchsia_fxfs::{CryptManagementMarker, CryptMarker, KeyPurpose};
use fidl_fuchsia_io as fio;
use fs_management::filesystem::{
    BlockConnector, Filesystem, ServingMultiVolumeFilesystem, ServingSingleVolumeFilesystem,
};
use fs_management::{Fxfs, Minfs};
use fuchsia_component::client::connect_to_protocol;
use rand::rngs::StdRng;
use rand::{RngExt as _, SeedableRng as _};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const DATA_FILESYSTEM_FORMAT: &'static str = std::env!("DATA_FILESYSTEM_FORMAT");

const DATA_KEY: [u8; 32] = [
    0xcf, 0x9e, 0x45, 0x2a, 0x22, 0xa5, 0x70, 0x31, 0x33, 0x3b, 0x4d, 0x6b, 0x6f, 0x78, 0x58, 0x29,
    0x04, 0x79, 0xc7, 0xd6, 0xa9, 0x4b, 0xce, 0x82, 0x04, 0x56, 0x5e, 0x82, 0xfc, 0xe7, 0x37, 0xa8,
];

const METADATA_KEY: [u8; 32] = [
    0x0f, 0x4d, 0xca, 0x6b, 0x35, 0x0e, 0x85, 0x6a, 0xb3, 0x8c, 0xdd, 0xe9, 0xda, 0x0e, 0xc8, 0x22,
    0x8e, 0xea, 0xd8, 0x05, 0xc4, 0xc9, 0x0b, 0xa8, 0xd8, 0x85, 0x87, 0x50, 0x75, 0x40, 0x1c, 0x4c,
];

#[derive(Copy, Clone)]
struct FsTree;

impl FsTree {
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

    async fn setup_fxfs(&self, block_connector: Box<dyn BlockConnector>) -> Result<()> {
        let mut fxfs = Filesystem::from_boxed_config(block_connector, Box::new(Fxfs::default()));
        fxfs.format().await?;
        let fs = fxfs.serve_multi_volume().await?;
        let crypt = Some(self.setup_crypt_service().await?);
        fs.create_volume(
            "default",
            CreateOptions::default(),
            MountOptions { crypt, ..MountOptions::default() },
        )
        .await?;
        Ok(())
    }

    async fn setup_minfs(&self, block_connector: Box<dyn BlockConnector>) -> Result<()> {
        let mut minfs = Filesystem::from_boxed_config(block_connector, Box::new(Minfs::default()));
        minfs.format().await.context("failed to format minfs")?;
        Ok(())
    }

    async fn serve_fxfs(&self, block_connector: Box<dyn BlockConnector>) -> Result<FsInstance> {
        let fxfs = Filesystem::from_boxed_config(block_connector, Box::new(Fxfs::default()));
        let fs = fxfs.serve_multi_volume().await?;
        let crypt = Some(self.setup_crypt_service().await?);
        let vol =
            fs.open_volume("default", MountOptions { crypt, ..MountOptions::default() }).await?;
        Ok(FsInstance::Fxfs(fs, vol))
    }

    async fn serve_minfs(&self, block_connector: Box<dyn BlockConnector>) -> Result<FsInstance> {
        let minfs = Filesystem::from_boxed_config(block_connector, Box::new(Minfs::default()));
        let fs = minfs.serve().await?;
        Ok(FsInstance::Minfs(fs))
    }

    async fn verify_fxfs(&self, block_connector: Box<dyn BlockConnector>) -> Result<()> {
        let mut fxfs = Filesystem::from_boxed_config(block_connector, Box::new(Fxfs::default()));
        fxfs.fsck().await.context("overall fsck")?;

        // Also check the volume we created.
        let fs = fxfs.serve_multi_volume().await.context("failed to serve")?;
        let crypt = Some(self.setup_crypt_service().await?);
        fs.check_volume("default", CheckOptions { crypt, ..Default::default() })
            .await
            .context("default volume check")?;
        Ok(())
    }

    async fn verify_minfs(&self, block_connector: Box<dyn BlockConnector>) -> Result<()> {
        let mut minfs = Filesystem::from_boxed_config(block_connector, Box::new(Minfs::default()));
        minfs.fsck().await?;
        Ok(())
    }
}

enum FsInstance {
    #[allow(dead_code)]
    Fxfs(ServingMultiVolumeFilesystem, fs_management::filesystem::ServingVolume),
    Minfs(ServingSingleVolumeFilesystem),
}

impl FsInstance {
    fn root(&self) -> &fio::DirectoryProxy {
        match &self {
            FsInstance::Fxfs(_, vol) => vol.root(),
            FsInstance::Minfs(fs) => fs.root(),
        }
    }
}

#[async_trait]
impl Test for FsTree {
    async fn setup(
        self: Arc<Self>,
        device_label: String,
        _device_path: Option<String>,
        _seed: u64,
    ) -> Result<()> {
        log::info!(device_label:%; "setting up");
        let block_connector = set_up_partition(device_label).await?;

        match DATA_FILESYSTEM_FORMAT {
            "fxfs" => self.setup_fxfs(block_connector).await?,
            "minfs" => self.setup_minfs(block_connector).await?,
            _ => panic!("Unsupported filesystem"),
        }

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
        let block_connector =
            find_partition(device_label).await.context("test failed to find partition")?;
        let fs = match DATA_FILESYSTEM_FORMAT {
            "fxfs" => self.serve_fxfs(block_connector).await?,
            "minfs" => self.serve_minfs(block_connector).await?,
            _ => panic!("Unsupported filesystem"),
        };

        log::info!("generating load");
        let mut rng = StdRng::seed_from_u64(seed);
        loop {
            log::debug!("generating tree");
            let dist = EntryDistribution::new(6);
            let tree: DirectoryEntry = rng.sample(&dist);
            log::debug!("generated tree: {:?}", tree);
            let tree_name = tree.get_name();
            log::debug!("writing tree");
            tree.write_tree_at(fs.root()).await.context("failed to write directory tree")?;
            // now try renaming the tree root
            let tree_name2 = format!("{}-renamed", tree_name);
            log::debug!("moving tree");
            fuchsia_fs::directory::rename(fs.root(), &tree_name, &tree_name2)
                .await
                .context("failed to rename directory tree")?;
            // then try deleting the entire thing.
            log::debug!("deleting tree");
            fuchsia_fs::directory::remove_dir_recursive(fs.root(), &tree_name2)
                .await
                .context("failed to delete directory tree")?;
        }
    }

    async fn verify(
        self: Arc<Self>,
        device_label: String,
        _device_path: Option<String>,
        _seed: u64,
    ) -> Result<()> {
        log::info!(device_label:%; "verifying disk consistency");
        let block_connector =
            find_partition(device_label).await.context("verify failed to find partition")?;

        match DATA_FILESYSTEM_FORMAT {
            "fxfs" => self.verify_fxfs(block_connector).await?,
            "minfs" => self.verify_minfs(block_connector).await?,
            _ => panic!("Unsupported filesystem"),
        }

        log::info!("verification complete");
        Ok(())
    }
}

#[fuchsia::main(logging_tags = ["blackout", "fs-tree"])]
async fn main() -> Result<()> {
    let server = TestServer::new(FsTree)?;
    server.serve().await;

    Ok(())
}
