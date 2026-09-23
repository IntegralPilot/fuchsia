// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::filesystems::FsManagementFilesystemInstance;
use async_trait::async_trait;
use fidl::endpoints::ClientEnd;
use fidl_fuchsia_fxfs::{CryptManagementMarker, CryptMarker, KeyPurpose};
use fuchsia_component::client::{connect_channel_to_protocol, connect_to_protocol_at_dir_root};

use std::path::Path;
use std::sync::{Arc, Once};
use storage_benchmarks::{
    BlockDeviceConfig, BlockDeviceFactory, CacheClearableFilesystem, Filesystem, FilesystemConfig,
};

/// Config object for starting Fxfs instances.
#[derive(Clone)]
pub struct Fxfs {
    volume_size: u64,
}

impl Fxfs {
    pub fn new(volume_size: u64) -> Self {
        Fxfs { volume_size }
    }
}

#[async_trait]
impl FilesystemConfig for Fxfs {
    type Filesystem = FxfsInstance;

    async fn start_filesystem(
        &self,
        block_device_factory: &dyn BlockDeviceFactory,
    ) -> FxfsInstance {
        let block_device = block_device_factory
            .create_block_device(&BlockDeviceConfig {
                requires_fvm: false,
                use_zxcrypt: false,
                volume_size: Some(self.volume_size),
            })
            .await;
        FxfsInstance {
            fxfs: FsManagementFilesystemInstance::new(
                fs_management::Fxfs::default,
                block_device,
                Some(Arc::new(get_crypt_client)),
                /*as_blob=*/ false,
            )
            .await,
        }
    }

    fn name(&self) -> String {
        "fxfs".to_owned()
    }
}

fn get_crypt_client() -> ClientEnd<CryptMarker> {
    static CRYPT_CLIENT_INITIALIZER: Once = Once::new();
    CRYPT_CLIENT_INITIALIZER.call_once(|| {
        let (client_end, server_end) = zx::Channel::create();
        connect_channel_to_protocol::<CryptManagementMarker>(server_end)
            .expect("Failed to connect to the crypt management service");
        let crypt_management_service =
            fidl_fuchsia_fxfs::CryptManagementSynchronousProxy::new(client_end);

        let mut key = [0; 32];
        zx::cprng_draw(&mut key);
        let wrapping_key_id_0 = [0; 16];
        match crypt_management_service
            .add_wrapping_key(&wrapping_key_id_0, &key, zx::MonotonicInstant::INFINITE)
            .expect("FIDL failed")
            .map_err(zx::Status::err_from_raw)
        {
            Ok(()) => {}
            Err(zx::Status::ALREADY_EXISTS) => {
                // In tests, the binary is run multiple times which gets around the `Once`. The fxfs
                // crypt component is not restarted for each test so it may already be initialized.
                return;
            }
            Err(e) => panic!("add_wrapping_key failed: {:?}", e),
        };
        zx::cprng_draw(&mut key);
        let mut wrapping_key_id_1 = [0; 16];
        wrapping_key_id_1[0] = 1;
        crypt_management_service
            .add_wrapping_key(&wrapping_key_id_1, &key, zx::MonotonicInstant::INFINITE)
            .expect("FIDL failed")
            .map_err(zx::Status::err_from_raw)
            .expect("add_wrapping_key failed");
        crypt_management_service
            .set_active_key(KeyPurpose::Data, &wrapping_key_id_0, zx::MonotonicInstant::INFINITE)
            .expect("FIDL failed")
            .map_err(zx::Status::err_from_raw)
            .expect("set_active_key failed");
        crypt_management_service
            .set_active_key(
                KeyPurpose::Metadata,
                &wrapping_key_id_1,
                zx::MonotonicInstant::INFINITE,
            )
            .expect("FIDL failed")
            .map_err(zx::Status::err_from_raw)
            .expect("set_active_key failed");
    });
    let (client_end, server_end) = fidl::endpoints::create_endpoints();
    connect_channel_to_protocol::<CryptMarker>(server_end.into())
        .expect("Failed to connect to crypt service");
    client_end
}

pub struct FxfsInstance {
    fxfs: FsManagementFilesystemInstance,
}

impl FxfsInstance {
    async fn flush_journal(&self) {
        connect_to_protocol_at_dir_root::<fidl_fuchsia_fxfs::DebugMarker>(
            self.fxfs.exposed_services_dir(),
        )
        .expect("Connecting to debug protocol")
        .compact()
        .await
        .expect("Sending journal flush message")
        .map_err(zx::Status::err_from_raw)
        .expect("Journal flush");
    }
}

#[async_trait]
impl Filesystem for FxfsInstance {
    async fn shutdown(self) {
        self.fxfs.shutdown().await
    }

    fn benchmark_dir(&self) -> &Path {
        self.fxfs.benchmark_dir()
    }
}

#[async_trait]
impl CacheClearableFilesystem for FxfsInstance {
    async fn clear_cache(&mut self) {
        // Flush the journal, since otherwise metadata stays in the mutable in-memory layer and
        // clearing cache doesn't remove it, even over remount. So to emulate accessing actually
        // cold data, we push it down into the on-disk layer by forcing a journal flush.
        self.flush_journal().await;
        self.fxfs.clear_cache().await
    }
}

#[cfg(test)]
mod tests {
    use super::Fxfs;
    use crate::filesystems::testing::check_filesystem;

    #[fuchsia::test]
    async fn start_fxfs() {
        check_filesystem(Fxfs { volume_size: 4 * 1024 * 1024 }).await;
    }
}
