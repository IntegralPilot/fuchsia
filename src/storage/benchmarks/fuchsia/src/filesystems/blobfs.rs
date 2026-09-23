// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::filesystems::{BlobFilesystem, FsManagementFilesystemInstance};
use async_trait::async_trait;
use fidl_fuchsia_fxfs::{BlobCreatorMarker, BlobCreatorProxy, BlobReaderMarker, BlobReaderProxy};
use fidl_fuchsia_io as fio;
use fuchsia_component::client::connect_to_protocol_at_dir_root;
use std::path::Path;
use storage_benchmarks::{
    BlockDeviceConfig, BlockDeviceFactory, CacheClearableFilesystem, Filesystem, FilesystemConfig,
};

/// Config object for starting Blobfs instances.
#[derive(Clone)]
pub struct Blobfs;

#[async_trait]
impl FilesystemConfig for Blobfs {
    type Filesystem = BlobfsInstance;

    async fn start_filesystem(
        &self,
        block_device_factory: &dyn BlockDeviceFactory,
    ) -> BlobfsInstance {
        let block_device = block_device_factory
            .create_block_device(&BlockDeviceConfig {
                requires_fvm: true,
                use_zxcrypt: false,
                volume_size: None,
            })
            .await;
        let blobfs = FsManagementFilesystemInstance::new(
            fs_management::Blobfs::default,
            block_device,
            None,
            /*as_blob=*/ false,
        )
        .await;
        let blob_creator =
            connect_to_protocol_at_dir_root::<BlobCreatorMarker>(blobfs.exposed_dir())
                .expect("failed to connect to the BlobCreator protocol");
        let blob_reader = connect_to_protocol_at_dir_root::<BlobReaderMarker>(blobfs.exposed_dir())
            .expect("failed to connect to the BlobReader protocol");
        BlobfsInstance { blob_creator, blob_reader, blobfs }
    }

    fn name(&self) -> String {
        "blobfs".to_owned()
    }
}

pub struct BlobfsInstance {
    blob_creator: BlobCreatorProxy,
    blob_reader: BlobReaderProxy,
    blobfs: FsManagementFilesystemInstance,
}

#[async_trait]
impl Filesystem for BlobfsInstance {
    async fn shutdown(self) {
        self.blobfs.shutdown().await
    }

    fn benchmark_dir(&self) -> &Path {
        self.blobfs.benchmark_dir()
    }
}

#[async_trait]
impl CacheClearableFilesystem for BlobfsInstance {
    async fn clear_cache(&mut self) {
        let () = self.blobfs.clear_cache().await;
        self.blob_creator =
            connect_to_protocol_at_dir_root::<BlobCreatorMarker>(self.blobfs.exposed_dir())
                .expect("failed to connect to the BlobCreator protocol");
        self.blob_reader =
            connect_to_protocol_at_dir_root::<BlobReaderMarker>(self.blobfs.exposed_dir())
                .expect("failed to connect to the BlobReader protocol");
    }
}

#[async_trait]
impl BlobFilesystem for BlobfsInstance {
    fn blob_creator(&self) -> &BlobCreatorProxy {
        &self.blob_creator
    }

    fn blob_reader(&self) -> &BlobReaderProxy {
        &self.blob_reader
    }

    fn exposed_dir(&self) -> &fio::DirectoryProxy {
        self.blobfs.exposed_dir()
    }
}

#[cfg(test)]
mod tests {
    use super::Blobfs;
    use crate::filesystems::testing::check_blob_filesystem;

    #[fuchsia::test]
    async fn start_blobfs() {
        check_blob_filesystem(Blobfs).await;
    }
}
