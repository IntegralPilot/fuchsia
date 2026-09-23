// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Tests fxfs compatibility with inline encryption using a software-based crypto implementation.

#[cfg(test)]
mod tests {
    use crate::component::new_block_client;
    use crate::file::FxFile;
    use crate::fuchsia::RemoteCrypt;
    use crate::fuchsia::volume::{FxVolume, MemoryPressureConfig};
    use fidl_fuchsia_fxfs::{CryptMarker, KeyPurpose};
    use fidl_fuchsia_hardware_inlineencryption::DeviceMarker;
    use fidl_fuchsia_io as fio;
    use fuchsia_async::Task;
    use fxfs::filesystem::{FxFilesystemBuilder, OpenFxFilesystem};
    use fxfs::lock_keys;
    use fxfs::object_handle::{ObjectHandle, WriteObjectHandle};
    use fxfs::object_store::directory::Directory;
    use fxfs::object_store::transaction::{LockKey, Options};
    use fxfs::object_store::volume::root_volume;
    use fxfs::object_store::{HandleOptions, NewChildStoreOptions, ObjectStore, StoreOptions};
    use fxfs_crypto::Crypt;
    use refaults_vmo::PageRefaultCounter;
    use std::sync::{Arc, Weak};
    use storage_device::DeviceHolder;
    use storage_device::block_device::BlockDevice;
    use test_vmo_backed_block_server::VmoBackedServer;
    use vfs::node::Node;

    const TEST_UUID: [u8; 16] =
        [73, 142, 230, 48, 132, 165, 68, 97, 141, 247, 22, 242, 153, 171, 153, 38];

    const TEST_DEVICE_BLOCK_SIZE: u32 = 512;

    struct TestFixture {
        block_server: Arc<VmoBackedServer>,
        filesystem: OpenFxFilesystem,
    }

    impl TestFixture {
        async fn new(inline_crypto_enabled: bool, barriers_enabled: bool) -> Self {
            let block_server = Arc::new(
                VmoBackedServer::new(393216, TEST_DEVICE_BLOCK_SIZE, &[])
                    .expect("Failed to create VmoBackedServer"),
            );

            let filesystem = FxFilesystemBuilder::new()
                .format(true)
                .inline_crypto_enabled(inline_crypto_enabled)
                .barriers_enabled(barriers_enabled)
                .open(DeviceHolder::new(
                    BlockDevice::new(
                        new_block_client(block_server.connect())
                            .await
                            .expect("failed to create new block client"),
                        false,
                    )
                    .await
                    .expect("failed to create block device"),
                ))
                .await
                .expect("failed to open filesystem");

            Self { block_server, filesystem }
        }

        // Default is to have inline encryption and barriers enabled
        async fn new_default() -> Self {
            Self::new(true, true).await
        }

        async fn create_vol_with_starnix_crypt(
            &mut self,
            name: &str,
        ) -> (Arc<ObjectStore>, Arc<starnix_crypt::CryptService>) {
            let block_server_clone = self.block_server.clone();
            let (insecure_inline_crypto_proxy, server) =
                fidl::endpoints::create_sync_proxy::<DeviceMarker>();
            Task::spawn(async move {
                block_server_clone
                    .connect_insecure_inline_encryption_server(
                        server.into_channel().into(),
                        TEST_UUID,
                    )
                    .await;
            })
            .detach();

            let raw_data_key = vec![0u8; 32];
            let raw_metadata_key = vec![0u8; 32];
            let crypt_service = Arc::new(starnix_crypt::CryptService::new(
                &raw_metadata_key,
                &raw_data_key,
                Some(insecure_inline_crypto_proxy),
            ));
            crypt_service.set_uuid(TEST_UUID);
            let crypt_service_clone = crypt_service.clone();
            let (crypt_client, crypt_server) = fidl::endpoints::create_endpoints::<CryptMarker>();
            Task::spawn(async move {
                crypt_service_clone
                    .handle_connection(crypt_server.into_stream())
                    .await
                    .expect("CryptService failed");
            })
            .detach();

            let crypt = Arc::new(RemoteCrypt::new(crypt_client));
            let root_volume =
                root_volume(self.filesystem.clone()).await.expect("root_volume failed");
            let vol = root_volume
                .new_volume(
                    name,
                    NewChildStoreOptions {
                        options: StoreOptions {
                            crypt: Some(crypt.clone() as Arc<dyn Crypt>),
                            ..StoreOptions::default()
                        },
                        ..NewChildStoreOptions::default()
                    },
                )
                .await
                .expect("new_volume failed");

            (vol, crypt_service)
        }

        async fn create_vol_with_fxfs_crypt(&self, name: &str) -> Arc<ObjectStore> {
            let crypt_service = Arc::new(fxfs_crypt::CryptService::new());
            crypt_service
                .add_wrapping_key(0, fxfs_insecure_crypto::DATA_KEY.to_vec())
                .expect("add_wrapping_key failed");
            crypt_service
                .add_wrapping_key(1, fxfs_insecure_crypto::METADATA_KEY.to_vec())
                .expect("add_wrapping_key failed");
            crypt_service.set_active_key(KeyPurpose::Data, 0).expect("set_active_key failed");
            crypt_service.set_active_key(KeyPurpose::Metadata, 1).expect("set_active_key failed");

            let (crypt_client, crypt_server) = fidl::endpoints::create_endpoints::<CryptMarker>();
            Task::spawn(async move {
                crypt_service
                    .handle_request(fxfs_crypt::Services::Crypt(crypt_server.into_stream()))
                    .await
                    .expect("CryptService failed");
            })
            .detach();

            let crypt = Arc::new(RemoteCrypt::new(crypt_client));
            let root_volume =
                root_volume(self.filesystem.clone()).await.expect("root_volume failed");
            root_volume
                .new_volume(
                    name,
                    NewChildStoreOptions {
                        options: StoreOptions {
                            crypt: Some(crypt.clone() as Arc<dyn Crypt>),
                            ..StoreOptions::default()
                        },
                        ..NewChildStoreOptions::default()
                    },
                )
                .await
                .expect("new_volume failed")
        }

        fn take_filesystem(self) -> OpenFxFilesystem {
            self.filesystem
        }

        async fn close(self) {
            self.filesystem.close().await.expect("close failed");
        }
    }

    #[fuchsia::test]
    async fn test_write_and_read_inline_encrypted_files() {
        let mut fixture = TestFixture::new_default().await;

        // Use Starnix CryptService which supports creating and storing keys in the format expected
        // for inline encryption.
        let (starnix_vol, crypt_service) = fixture.create_vol_with_starnix_crypt("starnix").await;

        let starnix_vol_root_dir =
            Directory::open(&starnix_vol, starnix_vol.root_directory_object_id())
                .await
                .expect("open failed");

        // Create a directory that has a wrapping key identifier.
        let mut transaction = fixture
            .filesystem
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(
                    starnix_vol.store_object_id(),
                    starnix_vol.root_directory_object_id()
                )],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let dir = starnix_vol_root_dir
            .create_child_dir(&mut transaction, "dir")
            .await
            .expect("create_child_dir failed");
        transaction.commit().await.expect("transaction commit failed");
        // `crypt_service` uses the (sync) crypt proxy to add wrapping keys. Wrap this with
        // `fuchsia_async::unblock` so that the test thread will not block on this operation.
        let key_identifier = fuchsia_async::unblock(move || {
            crypt_service.add_wrapping_key(&[0xab; 32], 0).expect("add_wrapping_key failed")
        })
        .await;
        let transaction = fixture
            .filesystem
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(starnix_vol.store_object_id(), dir.object_id())],
                Options::default(),
            )
            .await
            .expect("new transaction failed");
        dir.update_attributes(
            transaction,
            Some(&fio::MutableNodeAttributes {
                wrapping_key_id: Some(key_identifier),
                ..Default::default()
            }),
            0,
            None,
        )
        .await
        .expect("update attributes failed");

        // Files created within this directory will have wrapped key of type `FscryptInoLblk32File`.
        let mut transaction = fixture
            .filesystem
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(starnix_vol.store_object_id(), dir.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let handle = dir
            .create_child_file(&mut transaction, "file")
            .await
            .expect("create_child_file failed");
        transaction.commit().await.expect("commit failed");

        // Write and read using inline encryption
        let mut buf = handle.allocate_buffer(2 * TEST_DEVICE_BLOCK_SIZE as usize).await;
        buf.fill(0xaa);
        handle.write_or_append(Some(0), buf.as_ref()).await.expect("write failed");

        let mut buf = handle.allocate_buffer(2 * TEST_DEVICE_BLOCK_SIZE as usize).await;
        handle.read(0, buf.as_mut()).await.expect("read failed");
        assert_eq!(buf.to_vec(), vec![0xaa; 2 * TEST_DEVICE_BLOCK_SIZE as usize]);

        // If the keyslots are removed, reading from and writing to the file should now fail.
        // Cheat: Only one keyslot was programmed in this test, and VmoBackedServer programs keys to
        // the next available keyslot. The keyslot that we want to evict is 0.
        fixture.block_server.evict_key_slot(0).expect("evict_key_slot failed");

        let mut buf = handle.allocate_buffer(2 * TEST_DEVICE_BLOCK_SIZE as usize).await;
        handle.read(0, buf.as_mut()).await.expect_err("read passed unexpectedly");

        // Write with an aligned buffer to avoid reading from vmo when creating a new aligned
        // buffer. We already know that reading from vmo will fail, we want to check that writing
        // fails as well.
        let mut buf = handle.allocate_buffer(handle.block_size().get() as usize).await;
        buf.fill(0xcc);
        handle.write_or_append(Some(0), buf.as_ref()).await.expect_err("write passed unexpectedly");

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_multiple_volume_different_crypt_services() {
        let mut fixture = TestFixture::new_default().await;

        let (starnix_vol, starnix_crypt_service) =
            fixture.create_vol_with_starnix_crypt("starnix").await;
        // Note that fxfs crypt service does not support creating key of type `FscryptInoLblk32File`
        let vol_with_fxfs_crypt = fixture.create_vol_with_fxfs_crypt("vol").await;

        // (1) Test write and read from file in volume with Starnix crypt service
        let starnix_vol_root_dir =
            Directory::open(&starnix_vol, starnix_vol.root_directory_object_id())
                .await
                .expect("open failed");
        let mut transaction = fixture
            .filesystem
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(
                    starnix_vol.store_object_id(),
                    starnix_vol.root_directory_object_id()
                )],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let dir = starnix_vol_root_dir
            .create_child_dir(&mut transaction, "dir")
            .await
            .expect("create_child_dir failed");
        transaction.commit().await.expect("transaction commit failed");
        let key_identifier = fuchsia_async::unblock(move || {
            starnix_crypt_service.add_wrapping_key(&[0xab; 32], 0).expect("add_wrapping_key failed")
        })
        .await;
        let transaction = fixture
            .filesystem
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(starnix_vol.store_object_id(), dir.object_id())],
                Options::default(),
            )
            .await
            .expect("new transaction failed");
        dir.update_attributes(
            transaction,
            Some(&fio::MutableNodeAttributes {
                wrapping_key_id: Some(key_identifier),
                ..Default::default()
            }),
            0,
            None,
        )
        .await
        .expect("update attributes failed");
        let mut transaction = fixture
            .filesystem
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(starnix_vol.store_object_id(), dir.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let handle = dir
            .create_child_file(&mut transaction, "file")
            .await
            .expect("create_child_file failed");
        transaction.commit().await.expect("commit failed");

        // Write and read using inline encryption
        let mut buf = handle.allocate_buffer(2 * TEST_DEVICE_BLOCK_SIZE as usize).await;
        buf.fill(0xaa);
        handle.write_or_append(Some(0), buf.as_ref()).await.expect("write failed");

        let mut buf = handle.allocate_buffer(2 * TEST_DEVICE_BLOCK_SIZE as usize).await;
        handle.read(0, buf.as_mut()).await.expect("read failed");
        assert_eq!(buf.to_vec(), vec![0xaa; 2 * TEST_DEVICE_BLOCK_SIZE as usize]);

        // (2) Test write and read from file in volume with fxfs crypt service
        let vol_with_fxfs_crypt_root_dir =
            Directory::open(&vol_with_fxfs_crypt, vol_with_fxfs_crypt.root_directory_object_id())
                .await
                .expect("open failed");
        let mut transaction = fixture
            .filesystem
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(
                    vol_with_fxfs_crypt.store_object_id(),
                    vol_with_fxfs_crypt.root_directory_object_id()
                )],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let dir = vol_with_fxfs_crypt_root_dir
            .create_child_dir(&mut transaction, "dir1")
            .await
            .expect("create_child_dir failed");
        transaction.commit().await.expect("transaction commit failed");
        let mut transaction = fixture
            .filesystem
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(vol_with_fxfs_crypt.store_object_id(), dir.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let handle = dir
            .create_child_file(&mut transaction, "file1")
            .await
            .expect("create_child_file failed");
        transaction.commit().await.expect("commit failed");

        // Write and read non-inline encrypted files
        let mut buf = handle.allocate_buffer(2 * TEST_DEVICE_BLOCK_SIZE as usize).await;
        buf.fill(0xbb);
        handle.write_or_append(Some(0), buf.as_ref()).await.expect("write failed");

        let mut buf = handle.allocate_buffer(2 * TEST_DEVICE_BLOCK_SIZE as usize).await;
        handle.read(0, buf.as_mut()).await.expect("read failed");
        assert_eq!(buf.to_vec(), vec![0xbb; 2 * TEST_DEVICE_BLOCK_SIZE as usize]);

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_get_attribute_of_inline_encrypted_files() {
        let mut fixture = TestFixture::new_default().await;
        {
            // Use Starnix CryptService which supports creating and storing keys in the format
            // expected for inline encryption.
            let (starnix_vol, crypt_service) =
                fixture.create_vol_with_starnix_crypt("starnix").await;

            let starnix_vol_root_dir =
                Directory::open(&starnix_vol, starnix_vol.root_directory_object_id())
                    .await
                    .expect("open failed");

            // Create a directory that has a wrapping key identifier.
            let mut transaction = fixture
                .filesystem
                .root_store()
                .new_transaction(
                    lock_keys![LockKey::object(
                        starnix_vol.store_object_id(),
                        starnix_vol.root_directory_object_id()
                    )],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");
            let dir = starnix_vol_root_dir
                .create_child_dir(&mut transaction, "dir")
                .await
                .expect("create_child_dir failed");
            transaction.commit().await.expect("transaction commit failed");
            // `crypt_service` uses the (sync) crypt proxy to add wrapping keys. Wrap this with
            // `fuchsia_async::unblock` so that the test thread will not block on this operation.
            let key_identifier = fuchsia_async::unblock(move || {
                crypt_service.add_wrapping_key(&[0xab; 32], 0).expect("add_wrapping_key failed")
            })
            .await;
            let transaction = fixture
                .filesystem
                .root_store()
                .new_transaction(
                    lock_keys![LockKey::object(starnix_vol.store_object_id(), dir.object_id())],
                    Options::default(),
                )
                .await
                .expect("new transaction failed");
            dir.update_attributes(
                transaction,
                Some(&fio::MutableNodeAttributes {
                    wrapping_key_id: Some(key_identifier),
                    ..Default::default()
                }),
                0,
                None,
            )
            .await
            .expect("update attributes failed");

            let mut transaction = fixture
                .filesystem
                .root_store()
                .new_transaction(
                    lock_keys![LockKey::object(starnix_vol.store_object_id(), dir.object_id())],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");
            let file_object = dir
                .create_child_file(&mut transaction, "file")
                .await
                .expect("create_child_file failed");
            transaction.commit().await.expect("commit failed");

            let vol = Arc::new(
                FxVolume::new(
                    Weak::new(),
                    starnix_vol.clone(),
                    starnix_vol.store_object_id(),
                    "starnix-vol".to_owned(),
                    Arc::new(PageRefaultCounter::new().expect("PageRefaultCounter::new failed")),
                    MemoryPressureConfig::default(),
                )
                .expect("FxVolume::new failed"),
            );
            let file = FxFile::new(
                ObjectStore::open_object(
                    &vol,
                    file_object.object_id(),
                    HandleOptions::default(),
                    None,
                )
                .await
                .expect("open_object failed"),
            );
            let attributes = file
                .get_attributes(fio::NodeAttributesQuery::WRAPPING_KEY_ID)
                .await
                .expect("get_attributes failed");
            assert_eq!(attributes.mutable_attributes.wrapping_key_id, Some(key_identifier));
        }
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_write_to_inline_encrypted_file_fails_without_barriers() {
        // Create inline encrypted file and write to it when barriers are enabled.
        let mut fixture = TestFixture::new_default().await;

        let (starnix_vol, crypt_service) = fixture.create_vol_with_starnix_crypt("starnix").await;

        let starnix_vol_root_dir =
            Directory::open(&starnix_vol, starnix_vol.root_directory_object_id())
                .await
                .expect("open failed");
        let mut transaction = fixture
            .filesystem
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(
                    starnix_vol.store_object_id(),
                    starnix_vol.root_directory_object_id()
                )],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let dir = starnix_vol_root_dir
            .create_child_dir(&mut transaction, "dir")
            .await
            .expect("create_child_dir failed");
        transaction.commit().await.expect("transaction commit failed");
        let crypt_service_clone = crypt_service.clone();
        let key_identifier = fuchsia_async::unblock(move || {
            crypt_service_clone.add_wrapping_key(&[0xab; 32], 0).expect("add_wrapping_key failed")
        })
        .await;
        let transaction = fixture
            .filesystem
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(starnix_vol.store_object_id(), dir.object_id())],
                Options::default(),
            )
            .await
            .expect("new transaction failed");
        dir.update_attributes(
            transaction,
            Some(&fio::MutableNodeAttributes {
                wrapping_key_id: Some(key_identifier),
                ..Default::default()
            }),
            0,
            None,
        )
        .await
        .expect("update attributes failed");

        // Create an inline encrypted file.
        let mut transaction = fixture
            .filesystem
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(starnix_vol.store_object_id(), dir.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let handle = dir
            .create_child_file(&mut transaction, "file")
            .await
            .expect("create_child_file failed");
        transaction.commit().await.expect("commit failed");

        let mut buf = handle.allocate_buffer(2 * TEST_DEVICE_BLOCK_SIZE as usize).await;
        buf.fill(0xaa);
        handle.write_or_append(Some(0), buf.as_ref()).await.expect("write failed");

        let mut buf = handle.allocate_buffer(2 * TEST_DEVICE_BLOCK_SIZE as usize).await;
        handle.read(0, buf.as_mut()).await.expect("read failed");
        assert_eq!(buf.to_vec(), vec![0xaa; 2 * TEST_DEVICE_BLOCK_SIZE as usize]);

        // Reopen filesystem and unlock existing inline encrypted volume.
        let block_server = fixture.block_server.clone();
        let fs = fixture.take_filesystem();
        fs.close().await.expect("close failed");
        // Opening filesystem with `inline_crypto_enabled` but barriers not enabled should fail.
        let device = DeviceHolder::new(
            BlockDevice::new(
                new_block_client(block_server.clone().connect())
                    .await
                    .expect("failed to create new block client"),
                false,
            )
            .await
            .expect("failed to create block device"),
        );
        assert!(
            FxFilesystemBuilder::new()
                .inline_crypto_enabled(true)
                .barriers_enabled(false)
                .open(device)
                .await
                .is_err(),
            "Barriers must be enabled if intending to using inline crypto"
        );
        // Even though this is not recommended, users could set `inline_crypto_enabled` to false
        // with barriers disabled, and attempt to read/write to the inline encrypted file. We expect
        // this to fail.
        let device = DeviceHolder::new(
            BlockDevice::new(
                new_block_client(block_server.connect())
                    .await
                    .expect("failed to create new block client"),
                false,
            )
            .await
            .expect("failed to create block device"),
        );
        let fs = FxFilesystemBuilder::new()
            .inline_crypto_enabled(false)
            .barriers_enabled(false)
            .open(device)
            .await
            .expect("open failed");
        let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
        let (crypt_client, crypt_server) = fidl::endpoints::create_endpoints::<CryptMarker>();
        Task::spawn(async move {
            crypt_service
                .handle_connection(crypt_server.into_stream())
                .await
                .expect("CryptService failed");
        })
        .detach();
        let crypt = Arc::new(RemoteCrypt::new(crypt_client));
        let store = root_volume
            .volume(
                "starnix",
                StoreOptions {
                    crypt: Some(crypt.clone() as Arc<dyn Crypt>),
                    ..StoreOptions::default()
                },
            )
            .await
            .expect("opening volume failed");

        let vol = Arc::new(
            FxVolume::new(
                Weak::new(),
                store.clone(),
                store.store_object_id(),
                "starnix-vol".to_owned(),
                Arc::new(PageRefaultCounter::new().expect("PageRefaultCounter::new failed")),
                MemoryPressureConfig::default(),
            )
            .expect("FxVolume::new failed"),
        );
        let root_dir = Directory::open(&store, store.root_directory_object_id())
            .await
            .expect("open root dir failed");
        let (dir_id, _, _) =
            root_dir.lookup("dir").await.expect("lookup dir failed").expect("dir not found");
        let dir = Directory::open(&store, dir_id).await.expect("open dir failed");
        let (file_id, _, _) =
            dir.lookup("file").await.expect("lookup file failed").expect("file not found");
        let file = ObjectStore::open_object(&vol, file_id, HandleOptions::default(), None)
            .await
            .expect("open_object failed");

        let mut buf = file.allocate_buffer(2 * TEST_DEVICE_BLOCK_SIZE as usize).await;
        buf.fill(0xbb);
        file.write_or_append(Some(0), buf.as_ref())
            .await
            .expect_err("write passed unexpectedly without barriers");

        let mut buf = file.allocate_buffer(2 * TEST_DEVICE_BLOCK_SIZE as usize).await;
        file.read(0, buf.as_mut()).await.expect_err("read passed unexpectedly without barriers");

        fs.close().await.expect("close failed");
    }
}
