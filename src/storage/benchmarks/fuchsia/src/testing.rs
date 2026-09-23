// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use block_matcher::create_random_guid;
use fidl::endpoints::DiscoverableProtocolMarker as _;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_storage_block::BlockMarker;
use fs_management::Fvm;
use fs_management::filesystem::{
    BlockConnector, DirBasedBlockConnector, ServingMultiVolumeFilesystem,
};
use fuchsia_async as fasync;
use fuchsia_component::client::connect_to_protocol_at_dir_root;
use ramdevice_client::RamdiskClient;
use storage_benchmarks::block_device::BlockDevice;
use storage_benchmarks::{BlockDeviceConfig, BlockDeviceFactory};

use crate::block_devices::create_fvm_volume;

pub const RAMDISK_FVM_SLICE_SIZE: u64 = 1024 * 1024;

/// Creates block devices on ramdisks.
pub struct RamdiskFactory {
    block_size: u64,
    block_count: u64,
}

impl RamdiskFactory {
    #[allow(dead_code)]
    pub fn new(block_size: u64, block_count: u64) -> Self {
        Self { block_size, block_count }
    }
}

#[async_trait]
impl BlockDeviceFactory for RamdiskFactory {
    async fn create_block_device(&self, config: &BlockDeviceConfig) -> Box<dyn BlockDevice> {
        Box::new(Ramdisk::new(self.block_size, self.block_count, config).await)
    }
}

/// A ramdisk backed block device.
pub struct Ramdisk {
    _ramdisk: RamdiskClient,
    _fvm: ServingMultiVolumeFilesystem,
    volume_dir: fio::DirectoryProxy,
    _crypt_task: Option<fasync::Task<()>>,
}

impl Ramdisk {
    async fn new(block_size: u64, block_count: u64, config: &BlockDeviceConfig) -> Self {
        let ramdisk = RamdiskClient::builder(block_size, block_count)
            .build()
            .await
            .expect("Failed to create RamdiskClient");

        let mut fs = fs_management::filesystem::Filesystem::from_boxed_config(
            ramdisk.connector().unwrap(),
            Box::new(Fvm { slice_size: RAMDISK_FVM_SLICE_SIZE, ..Fvm::default() }),
        );
        fs.format().await.expect("Failed to format FVM");
        let fvm = fs.serve_multi_volume().await.expect("Failed to serve FVM");
        let volumes = connect_to_protocol_at_dir_root::<fidl_fuchsia_fs_startup::VolumesMarker>(
            fvm.exposed_dir(),
        )
        .unwrap();
        let (volume_dir, crypt_task) =
            create_fvm_volume(&volumes, create_random_guid(), config).await;

        Self { _ramdisk: ramdisk, _fvm: fvm, volume_dir, _crypt_task: crypt_task }
    }
}

impl BlockDevice for Ramdisk {
    fn connector(&self) -> Box<dyn BlockConnector> {
        let volume_dir = fuchsia_fs::directory::clone(&self.volume_dir).unwrap();
        Box::new(DirBasedBlockConnector::new(
            volume_dir,
            format!("svc/{}", BlockMarker::PROTOCOL_NAME),
        ))
    }
}
