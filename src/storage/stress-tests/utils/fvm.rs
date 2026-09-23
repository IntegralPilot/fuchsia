// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::endpoints::DiscoverableProtocolMarker;
use fidl_fuchsia_io as fio;

pub use block_matcher::Guid;
use block_matcher::create_random_guid;
use fs_management::filesystem::{
    DirBasedBlockConnector, Filesystem, ServingMultiVolumeFilesystem, ServingVolume,
};
use fs_management::{self, Fvm};
use ramdevice_client::{RamdiskClient, RamdiskClientBuilder};
use zx::{Rights, Vmo};

async fn create_ramdisk(vmo: &Vmo, ramdisk_block_size: u64) -> RamdiskClient {
    let duplicated_handle = vmo.as_handle_ref().duplicate_handle(Rights::SAME_RIGHTS).unwrap();
    let duplicated_vmo = Vmo::from(duplicated_handle);

    RamdiskClientBuilder::new_with_vmo(duplicated_vmo, Some(ramdisk_block_size))
        .build()
        .await
        .unwrap()
}

/// A wrapper around a running FVM component instance backed by a ramdisk.
pub struct FvmInstance {
    ramdisk: RamdiskClient,
    fvm_instance: ServingMultiVolumeFilesystem,
}

/// A wrapper around an opened volume in an FVM instance.
/// This keeps the volume alive and provides access to its block connector.
pub struct FvmVolume {
    serving_volume: ServingVolume,
    guid: Guid,
}

impl FvmVolume {
    /// Creates a new `FvmVolume` wrapping the given `ServingVolume` and `Guid`.
    pub fn new(serving_volume: ServingVolume, guid: Guid) -> Self {
        Self { serving_volume, guid }
    }

    pub fn guid(&self) -> &Guid {
        &self.guid
    }

    pub fn block_connector(&self) -> DirBasedBlockConnector {
        let block_dir = fuchsia_fs::directory::clone(self.serving_volume.exposed_dir()).unwrap();
        DirBasedBlockConnector::new(
            block_dir,
            format!("svc/{}", fidl_fuchsia_storage_block::BlockMarker::PROTOCOL_NAME),
        )
    }
}

impl FvmInstance {
    /// Creates a new FVM instance.  If `fvm_slice_size` is specified, the device is formatted with
    /// the specified slice size.  If not specified, `vmo` should contain an existing FVM format.
    pub async fn new(vmo: &Vmo, ramdisk_block_size: u64, fvm_slice_size: Option<u64>) -> Self {
        let ramdisk = create_ramdisk(&vmo, ramdisk_block_size).await;

        let mut fs = Filesystem::from_boxed_config(
            ramdisk.connector().unwrap(),
            Box::new(Fvm { slice_size: fvm_slice_size.unwrap_or(0), ..Fvm::dynamic_child() }),
        );

        if fvm_slice_size.is_some() {
            fs.format().await.unwrap();
        }

        Self { ramdisk, fvm_instance: fs.serve_multi_volume().await.unwrap() }
    }

    pub async fn new_volume(
        &mut self,
        name: &str,
        type_guid: &Guid,
        initial_volume_size: Option<u64>,
    ) -> FvmVolume {
        let instance_guid = create_random_guid();

        let create_options = fidl_fuchsia_fs_startup::CreateOptions {
            initial_size: initial_volume_size,
            guid: Some(instance_guid.clone()),
            type_guid: Some(type_guid.clone()),
            ..fidl_fuchsia_fs_startup::CreateOptions::default()
        };

        let mount_options = fidl_fuchsia_fs_startup::MountOptions::default();

        let serving_volume =
            self.fvm_instance.create_volume(name, create_options, mount_options).await.unwrap();

        FvmVolume { serving_volume, guid: instance_guid }
    }

    pub async fn open_volume(&self, name: &str) -> FvmVolume {
        let guid = self.fvm_instance.get_volume_info(name).await.unwrap().guid;
        let serving_volume = self
            .fvm_instance
            .open_volume(name, fidl_fuchsia_fs_startup::MountOptions::default())
            .await
            .unwrap();
        FvmVolume::new(serving_volume, guid.unwrap())
    }

    pub fn ramdisk_get_dir(&self) -> Option<&fio::DirectoryProxy> {
        Some(self.ramdisk.outgoing())
    }

    pub async fn free_space(&self) -> u64 {
        let info = self.fvm_instance.get_info().await.unwrap();
        (info.slice_count - info.assigned_slice_count) * info.slice_size
    }
}
