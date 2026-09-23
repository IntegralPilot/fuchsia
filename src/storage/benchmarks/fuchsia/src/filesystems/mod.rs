// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use blob_writer::BlobWriter;
use delivery_blob::{CompressionMode, DeliveryBlobType, Type1Blob, Type3Blob};
use fidl::endpoints::ClientEnd;
use fidl_fuchsia_fs_startup::{CreateOptions, MountOptions};
use fidl_fuchsia_fxfs::{BlobCreatorProxy, BlobReaderProxy, CryptMarker};
use fidl_fuchsia_io as fio;
use fs_management::FSConfig;
use fs_management::filesystem::{
    Filesystem as FsManagementFilesystem, ServingMultiVolumeFilesystem,
    ServingSingleVolumeFilesystem, ServingVolume,
};
use fuchsia_merkle::Hash;
use std::path::Path;
use std::sync::Arc;
use storage_benchmarks::block_device::BlockDevice;
use storage_benchmarks::{CacheClearableFilesystem, Filesystem};

mod blobfs;
mod f2fs;
mod fxblob;
pub mod fxfs;
mod memfs;
mod minfs;
mod pkgdir;
#[cfg(test)]
mod testing;

pub use blobfs::Blobfs;
pub use f2fs::F2fs;
pub use fxblob::{Fxblob, FxblobInstance};
pub use fxfs::Fxfs;
pub use memfs::Memfs;
pub use minfs::Minfs;
pub use pkgdir::{PkgDirInstance, PkgDirTest};

const MOUNT_PATH: &str = "/benchmark";

/// Struct for holding the name of a blob and its contents in the delivery blob format.
pub struct DeliveryBlob {
    pub data: Vec<u8>,
    pub name: Hash,
}

impl DeliveryBlob {
    pub fn new(data: Vec<u8>, mode: CompressionMode) -> Self {
        Self::new_with_type(DeliveryBlobType::Type1, data, mode)
    }

    pub fn new_with_type(
        delivery_type: DeliveryBlobType,
        data: Vec<u8>,
        mode: CompressionMode,
    ) -> Self {
        let name = fuchsia_merkle::root_from_slice(&data);
        let data = match delivery_type {
            DeliveryBlobType::Type1 => Type1Blob::generate(&data, mode),
            DeliveryBlobType::Type3 => Type3Blob::generate(&data, mode),
            _ => panic!("Unsupported delivery blob type"),
        };
        Self { data, name }
    }
}

/// A trait for filesystems that support reading and writing blobs.
#[async_trait]
pub trait BlobFilesystem: CacheClearableFilesystem {
    /// Writes a blob to the filesystem.
    async fn write_blob(&self, blob: &DeliveryBlob) {
        let writer_client_end = self
            .blob_creator()
            .create(&blob.name.into(), false)
            .await
            .expect("transport error on BlobCreator.Create")
            .expect("failed to create blob");
        let writer = writer_client_end.into_proxy();
        let mut blob_writer = BlobWriter::create(writer, blob.data.len() as u64)
            .await
            .expect("failed to create BlobWriter");
        blob_writer.write(&blob.data).await.unwrap();
    }

    /// Gets a VMO backing a blob.
    async fn get_vmo(&self, name: &Hash) -> zx::Vmo {
        self.blob_reader()
            .get_vmo(&*name)
            .await
            .expect("transport error on BlobReader.GetVmo")
            .expect("failed to get vmo")
    }

    async fn remove_blob(&self, name: &Hash) {
        let root = fuchsia_fs::directory::open_directory(
            self.exposed_dir(),
            "root",
            fio::PERM_READABLE | fio::PERM_WRITABLE,
        )
        .await
        .expect("failed to open blob directory");
        root.unlink(&name.to_string(), &fio::UnlinkOptions::default())
            .await
            .expect("transport error on Directory.Unlink")
            .expect("failed to unlink blob");
        root.sync()
            .await
            .expect("transport error on Directory.Sync")
            .expect("failed to sync blob directory");
    }

    /// Direct access to the BlobCreator protocol.
    fn blob_creator(&self) -> &BlobCreatorProxy;

    /// Direct access to the BlobReader protocol.
    fn blob_reader(&self) -> &BlobReaderProxy;

    /// Returns the exposed dir of the blob volume.
    fn exposed_dir(&self) -> &fio::DirectoryProxy;
}

enum FsType {
    SingleVolume(ServingSingleVolumeFilesystem),
    MultiVolume(ServingMultiVolumeFilesystem, ServingVolume),
}

pub type CryptClientFn = Arc<dyn Fn() -> ClientEnd<CryptMarker> + Send + Sync>;

pub struct FsManagementFilesystemInstance {
    config_creator: Box<dyn Fn() -> Box<dyn FSConfig> + Send + Sync>,
    crypt_client_fn: Option<CryptClientFn>,
    serving_filesystem: Option<FsType>,
    as_blob: bool,
    // Keep the underlying block device alive for as long as we are using the filesystem.
    block_device: Box<dyn BlockDevice>,
}

impl FsManagementFilesystemInstance {
    pub async fn new<FSC: FSConfig>(
        config_creator: impl (Fn() -> FSC) + Send + Sync + 'static,
        block_device: Box<dyn BlockDevice>,
        crypt_client_fn: Option<CryptClientFn>,
        as_blob: bool,
    ) -> Self {
        let config_creator = Box::new(move || Box::new(config_creator()) as Box<dyn FSConfig>);
        let mut fs =
            FsManagementFilesystem::from_boxed_config(block_device.connector(), config_creator());
        fs.format().await.expect("Failed to format the filesystem");
        let serving_filesystem = if fs.config().is_multi_volume() {
            let serving_filesystem =
                fs.serve_multi_volume().await.expect("Failed to start the filesystem");
            let mut vol = serving_filesystem
                .create_volume(
                    "default",
                    CreateOptions::default(),
                    MountOptions {
                        crypt: crypt_client_fn.as_ref().map(|f| f()),
                        as_blob: Some(as_blob),
                        ..MountOptions::default()
                    },
                )
                .await
                .expect("Failed to create volume");
            vol.bind_to_path(MOUNT_PATH).expect("Failed to bind the volume");
            FsType::MultiVolume(serving_filesystem, vol)
        } else {
            let mut serving_filesystem = fs.serve().await.expect("Failed to start the filesystem");
            serving_filesystem.bind_to_path(MOUNT_PATH).expect("Failed to bind the filesystem");
            FsType::SingleVolume(serving_filesystem)
        };
        Self {
            config_creator,
            crypt_client_fn,
            serving_filesystem: Some(serving_filesystem),
            as_blob,
            block_device,
        }
    }

    fn exposed_dir(&self) -> &fio::DirectoryProxy {
        let fs = self.serving_filesystem.as_ref().unwrap();
        match fs {
            FsType::SingleVolume(serving_filesystem) => serving_filesystem.exposed_dir(),
            FsType::MultiVolume(_, serving_volume) => serving_volume.exposed_dir(),
        }
    }

    // Instead of returning the directory that is the data root for this filesystem, return the root
    // that would be used for accessing top level service protocols.
    pub(crate) fn exposed_services_dir(&self) -> &fio::DirectoryProxy {
        let fs = self.serving_filesystem.as_ref().unwrap();
        match fs {
            FsType::SingleVolume(serving_filesystem) => serving_filesystem.exposed_dir(),
            FsType::MultiVolume(serving_filesystem, _) => serving_filesystem.exposed_dir(),
        }
    }

    fn fs(&self) -> FsManagementFilesystem {
        FsManagementFilesystem::from_boxed_config(
            self.block_device.connector(),
            (self.config_creator)(),
        )
    }
}

#[async_trait]
impl Filesystem for FsManagementFilesystemInstance {
    async fn shutdown(mut self) {
        if let Some(fs) = self.serving_filesystem.take() {
            match fs {
                FsType::SingleVolume(fs) => fs.shutdown().await.expect("Failed to stop filesystem"),
                FsType::MultiVolume(fs, vol) => {
                    vol.shutdown().await.expect("Failed to stop volume");
                    fs.shutdown().await.expect("Failed to stop filesystem")
                }
            }
        }
    }

    fn benchmark_dir(&self) -> &Path {
        Path::new(MOUNT_PATH)
    }
}

#[async_trait]
impl CacheClearableFilesystem for FsManagementFilesystemInstance {
    async fn clear_cache(&mut self) {
        // Remount the filesystem to guarantee that all cached data from reads and write is cleared.
        let serving_filesystem = self.serving_filesystem.take().unwrap();
        let serving_filesystem = match serving_filesystem {
            FsType::SingleVolume(serving_filesystem) => {
                serving_filesystem.shutdown().await.expect("Failed to stop the filesystem");
                let mut serving_filesystem =
                    self.fs().serve().await.expect("Failed to start the filesystem");
                serving_filesystem.bind_to_path(MOUNT_PATH).expect("Failed to bind the filesystem");
                FsType::SingleVolume(serving_filesystem)
            }
            FsType::MultiVolume(serving_filesystem, volume) => {
                volume.shutdown().await.expect("Failed to stop the volume");
                serving_filesystem.shutdown().await.expect("Failed to stop the filesystem");
                let serving_filesystem =
                    self.fs().serve_multi_volume().await.expect("Failed to start the filesystem");
                let mut vol = serving_filesystem
                    .open_volume(
                        "default",
                        MountOptions {
                            crypt: self.crypt_client_fn.as_ref().map(|f| f()),
                            as_blob: Some(self.as_blob),
                            ..MountOptions::default()
                        },
                    )
                    .await
                    .expect("Failed to create volume");
                vol.bind_to_path(MOUNT_PATH).expect("Failed to bind the volume");
                FsType::MultiVolume(serving_filesystem, vol)
            }
        };
        self.serving_filesystem = Some(serving_filesystem);
    }
}
