// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::filesystems::{BlobFilesystem, FsManagementFilesystemInstance};
use async_trait::async_trait;
use fidl_fuchsia_fxfs::{BlobCreatorMarker, BlobCreatorProxy, BlobReaderMarker, BlobReaderProxy};
use fidl_fuchsia_io as fio;
use fuchsia_component::client::{connect_to_protocol_at_dir_root, connect_to_protocol_at_dir_svc};
use std::path::Path;
use storage_benchmarks::{
    BlockDeviceConfig, BlockDeviceFactory, CacheClearableFilesystem, Filesystem, FilesystemConfig,
};

/// Config object for starting Fxblob instances.
#[derive(Clone)]
pub struct Fxblob;

#[async_trait]
impl FilesystemConfig for Fxblob {
    type Filesystem = FxblobInstance;

    async fn start_filesystem(
        &self,
        block_device_factory: &dyn BlockDeviceFactory,
    ) -> FxblobInstance {
        let block_device = block_device_factory
            .create_block_device(&BlockDeviceConfig {
                requires_fvm: false,
                use_zxcrypt: false,
                volume_size: Some(104 * 1024 * 1024),
            })
            .await;
        let fxblob = FsManagementFilesystemInstance::new(
            || fs_management::Fxfs { allow_type3_blobs: true, ..Default::default() },
            block_device,
            None,
            /*as_blob=*/ true,
        )
        .await;
        let blob_creator =
            connect_to_protocol_at_dir_svc::<BlobCreatorMarker>(fxblob.exposed_dir())
                .expect("failed to connect to the BlobCreator protocol");
        let blob_reader = connect_to_protocol_at_dir_svc::<BlobReaderMarker>(fxblob.exposed_dir())
            .expect("failed to connect to the BlobReader protocol");
        FxblobInstance { blob_creator, blob_reader, fxblob }
    }

    fn name(&self) -> String {
        "fxblob".to_owned()
    }
}

pub struct FxblobInstance {
    blob_creator: BlobCreatorProxy,
    blob_reader: BlobReaderProxy,
    fxblob: FsManagementFilesystemInstance,
}

impl FxblobInstance {
    pub fn connect_to_debug(&self) -> fidl_fuchsia_fxfs::DebugProxy {
        connect_to_protocol_at_dir_root::<fidl_fuchsia_fxfs::DebugMarker>(
            self.fxblob.exposed_services_dir(),
        )
        .expect("failed to connect to Debug protocol")
    }
}

#[async_trait]
impl Filesystem for FxblobInstance {
    async fn shutdown(self) {
        self.fxblob.shutdown().await
    }

    fn benchmark_dir(&self) -> &Path {
        self.fxblob.benchmark_dir()
    }
}

#[async_trait]
impl CacheClearableFilesystem for FxblobInstance {
    async fn clear_cache(&mut self) {
        let () = self.fxblob.clear_cache().await;
        self.blob_creator =
            connect_to_protocol_at_dir_svc::<BlobCreatorMarker>(self.fxblob.exposed_dir())
                .expect("failed to connect to the BlobCreator protocol");
        self.blob_reader =
            connect_to_protocol_at_dir_svc::<BlobReaderMarker>(self.fxblob.exposed_dir())
                .expect("failed to connect to the BlobReader protocol");
    }
}

#[async_trait]
impl BlobFilesystem for FxblobInstance {
    fn blob_creator(&self) -> &BlobCreatorProxy {
        &self.blob_creator
    }

    fn blob_reader(&self) -> &BlobReaderProxy {
        &self.blob_reader
    }

    fn exposed_dir(&self) -> &fio::DirectoryProxy {
        self.fxblob.exposed_dir()
    }
}

#[cfg(test)]
mod tests {
    use super::Fxblob;
    use crate::filesystems::testing::check_blob_filesystem;

    #[fuchsia::test]
    async fn start_fxblob_new() {
        check_blob_filesystem(Fxblob).await;
    }
}
