// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::*;
use anyhow::Context;
use block_client::RemoteBlockClient;
use f2fs_reader::F2fsReader;
use fidl::endpoints::Proxy;
use fidl_fuchsia_fxfs::CryptMarker;
use fidl_fuchsia_hardware_inlineencryption::DeviceMarker;
use fidl_fuchsia_storage_block::{BlockMarker, BlockProxy};
use fuchsia_async::{LocalExecutor, LocalExecutorBuilder};

use fxfs::filesystem::{FxFilesystemBuilder, OpenFxFilesystem};
use fxfs::object_store::journal::super_block::SuperBlockInstance;
use fxfs::object_store::volume::root_volume as fxfs_root_volume;
use fxfs::object_store::{Directory, HandleOptions, ObjectStore};
use fxfs_platform::fuchsia::RemoteCrypt;
use starnix_crypt::CryptService;
use storage_device::block_device::BlockDevice;
use storage_device::ranged_device::RangedDevice;

use std::io::Read;
use std::sync::Arc;
use storage_device::{Device, DeviceHolder};
use test_vmo_backed_block_server::VmoBackedServer;

fn create_device_with_image_at_offset(path: &str, offset: u64) -> Arc<VmoBackedServer> {
    let path = std::path::PathBuf::from(path);
    let mut reader = zstd::Decoder::new(std::fs::File::open(&path).expect("open image"))
        .expect("decompress image");

    // Start with a VMO size that covers the offset. We'll grow it as needed.
    // We can't easily know the uncompressed size upfront without reading it all,
    // so we'll resize dynamically.
    let mut current_size = std::cmp::max(
        offset + 1024 * 1024, // Start with at least 1MB past offset
        SuperBlockInstance::B.first_extent().end + FXFS_BLOCK_SIZE,
    );
    let vmo = zx::Vmo::create_with_opts(zx::VmoOptions::RESIZABLE, current_size)
        .expect("failed to create vmo");

    let mut buffer = vec![0u8; 1024 * 1024]; // 1MB buffer
    let mut current_offset = offset;

    loop {
        let n = reader.read(&mut buffer).expect("failed to read image");
        if n == 0 {
            break;
        }
        let chunk = &buffer[..n];

        // If a chunk is all zeros, skip it. Vmo zero pages don't consume any RAM.
        if !chunk.iter().all(|&x| x == 0) {
            // Ensure VMO is large enough.
            if current_offset + n as u64 > current_size {
                current_size = std::cmp::max(current_size * 2, current_offset + n as u64);
                vmo.set_size(current_size).expect("failed to resize vmo");
            }
            vmo.write(chunk, current_offset).expect("failed to write image to vmo");
        }
        current_offset += n as u64;
    }

    // Ensure the VMO is at least the size required for Fxfs superblocks.
    let min_size = SuperBlockInstance::B.first_extent().end + FXFS_BLOCK_SIZE;
    if current_offset < min_size {
        if current_size < min_size {
            vmo.set_size(min_size).expect("failed to resize vmo");
        }
        // If we didn't write up to min_size, the rest is zeros (implicit), which is what we want.
    } else {
        // We might have over-allocated during the loop. Shrink to fit the actual data end
        // if it's larger than min_size, or just leave it.
        // Let's resize to exact fit or min_size.
        let final_size = std::cmp::max(current_offset, min_size);
        if current_size != final_size {
            vmo.set_size(final_size).expect("failed to resize vmo");
        }
    }

    Arc::new(
        VmoBackedServer::from_vmo(F2FS_BLOCK_SIZE as u32, vmo)
            .expect("Failed to create VmoBackedServer"),
    )
}

async fn test_fxfs_migration_at_offset(offset: u64) {
    let block_server = create_device_with_image_at_offset("/pkg/testdata/f2fs.img.zst", offset);
    let (client, server_end) = fidl::endpoints::create_proxy::<BlockMarker>();
    let block_server_clone = block_server.clone();
    std::thread::spawn(move || {
        let mut executor = LocalExecutorBuilder::new().build();
        executor.run_singlethreaded(async move {
            if let Err(e) = block_server_clone.serve(server_end.into_stream()).await {
                log::error!("Server error: {:?}", e);
            }
        });
    });
    let device = Arc::new(
        BlockDevice::new(
            RemoteBlockClient::new(BlockProxy::new(client.into_channel().unwrap()))
                .await
                .expect("Unable to create block client"),
            false,
        )
        .await
        .unwrap(),
    );

    // Open f2fs to get UUID.
    let block_size = device.block_size() as u64;
    let start_block = offset / block_size;
    let num_blocks = device.block_count() - start_block;

    let original_superblock = {
        let ranged_device = Arc::new(
            RangedDevice::new(
                device.clone(),
                start_block * block_size..(start_block + num_blocks) * block_size,
            )
            .expect("create ranged device"),
        );
        let f2fs = F2fsReader::open_device(ranged_device).await.expect("f2fs open ok");
        (*f2fs.superblock()).clone()
    };

    let block_server_clone = block_server.clone();
    let (insecure_inline_crypto_proxy, server) =
        fidl::endpoints::create_sync_proxy::<DeviceMarker>();
    std::thread::spawn(move || {
        LocalExecutor::default().run_singlethreaded(async {
            block_server_clone
                .connect_insecure_inline_encryption_server(server, original_superblock.uuid)
                .await
        })
    });

    let crypt_service =
        Arc::new(CryptService::new(&[0; 32], &[1; 32], Some(insecure_inline_crypto_proxy)));

    let (crypt_client_end, crypt_proxy) = fidl::endpoints::create_endpoints::<CryptMarker>();

    let crypt_service_clone = Arc::clone(&crypt_service);
    fuchsia_async::Task::spawn(async move {
        if let Err(err) = crypt_service_clone.handle_connection(crypt_proxy.into_stream()).await {
            log::error!(err:?; "Crypt service failure");
        }
    })
    .detach();

    let crypt = Arc::new(RemoteCrypt::new(crypt_client_end)) as Arc<dyn Crypt>;
    let device = Arc::try_unwrap(device).map_err(|_| ()).expect("only one ref to device");

    Box::pin(migrate_device(offset, DeviceHolder::new(device), crypt.clone()))
        .await
        .expect("migrate_device");

    let (client, server_end) = fidl::endpoints::create_proxy::<BlockMarker>();
    let block_server_clone = block_server.clone();
    std::thread::spawn(move || {
        let mut executor = LocalExecutorBuilder::new().build();
        executor.run_singlethreaded(async move {
            if let Err(e) = block_server_clone.serve(server_end.into_stream()).await {
                log::error!("Server error: {:?}", e);
            }
        });
    });
    let device = DeviceHolder::new(
        BlockDevice::new(
            RemoteBlockClient::new(BlockProxy::new(client.into_channel().unwrap()))
                .await
                .expect("Unable to create block client"),
            false,
        )
        .await
        .unwrap(),
    );

    let fxfs = FxFilesystemBuilder::new()
        .read_only(true)
        .barriers_enabled(true)
        .open(device)
        .await
        .expect("open failed");

    let root_vol = fxfs_root_volume(fxfs.clone()).await.expect("Opening root volume");
    let vol = root_vol
        .volume("userdata", StoreOptions { crypt: Some(crypt.clone()), ..StoreOptions::default() })
        .await
        .expect("Opening volume");
    fxfs::fsck::fsck_volume(&fxfs, vol.store_object_id(), Some(crypt)).await.expect("fsck volume");
    let root_directory =
        Directory::open(&vol, vol.root_directory_object_id()).await.expect("open failed");
    let ino = original_superblock.root_ino;

    // Note that we can't check file contents in this test as we haven't given fxfs encryption keys.
    let check_file_contents = false;
    Box::pin(verify(offset, fxfs.device(), &fxfs, ino, root_directory, check_file_contents))
        .await
        .expect("verify");

    fxfs.close().await.expect("close ok");
}

// Migrates an f2fs device to fxfs and verifies directory tree matches.
// Note this test can't verify file contents as we haven't given encryption keys.
#[fuchsia::test]
async fn test_fxfs_migration_no_keys() {
    Box::pin(test_fxfs_migration_at_offset(0)).await;
}

#[fuchsia::test]
async fn test_fxfs_migration_with_offset() {
    // Place f2fs image between Fxfs superblocks.
    let offset = SuperBlockInstance::A.first_extent().end + FXFS_BLOCK_SIZE;
    Box::pin(test_fxfs_migration_at_offset(offset)).await;
}

async fn recurse_resolve_f2fs(f2fs: &F2fsReader, ino: u32, path: &str) -> u32 {
    if let Some((head, rest)) = path.split_once("/") {
        for entry in f2fs.readdir(ino).await.expect("readdir") {
            if entry.filename == head {
                return Box::pin(recurse_resolve_f2fs(f2fs, entry.ino, rest)).await;
            }
        }
    } else {
        for entry in f2fs.readdir(ino).await.expect("readdir") {
            if entry.filename == path {
                return entry.ino;
            }
        }
    }
    panic!("Path not found: {path:?}");
}

// Read a single file encrypted with fscrypt's INO_LBLK32 mode.
#[fuchsia::test]
async fn test_fxfs_read_lblk32_ino_file() {
    let offset = 0;
    let block_server = create_device_with_image_at_offset("/pkg/testdata/f2fs.img.zst", offset);
    let (client, server_end) = fidl::endpoints::create_proxy::<BlockMarker>();
    let block_server_clone = block_server.clone();
    std::thread::spawn(move || {
        let mut executor = LocalExecutorBuilder::new().build();
        executor.run_singlethreaded(async move {
            if let Err(e) = block_server_clone.serve(server_end.into_stream()).await {
                log::error!("Server error: {:?}", e);
            }
        });
    });
    let device = Arc::new(
        BlockDevice::new(
            RemoteBlockClient::new(BlockProxy::new(client.into_channel().unwrap()))
                .await
                .expect("Unable to create block client"),
            false,
        )
        .await
        .unwrap(),
    );

    // Read data from F2FS before migration.
    let (superblock, expected_data, ino) = {
        let block_size = device.block_size() as u64;
        let start_block = offset / block_size;
        let num_blocks = device.block_count() - start_block;
        let ranged_device = Arc::new(
            RangedDevice::new(
                device.clone(),
                start_block * block_size..(start_block + num_blocks) * block_size,
            )
            .expect("create ranged device"),
        );
        let mut f2fs = F2fsReader::open_device(ranged_device).await.expect("f2fs open ok");
        f2fs.add_key(&[0; 64]);

        let ino = recurse_resolve_f2fs(&f2fs, f2fs.root_ino(), "fscrypt/a/b/inlined").await;
        let inode = f2fs.read_inode(ino).await.expect("read file");
        let f2fs_data = f2fs.read_data(&inode, 0).await.expect("read data");
        (f2fs.superblock().clone(), f2fs_data, ino)
    };

    let device = Arc::try_unwrap(device).map_err(|_| ()).expect("only one ref to device");

    let block_server_clone = block_server.clone();
    let (insecure_inline_crypto_proxy, server) =
        fidl::endpoints::create_sync_proxy::<DeviceMarker>();
    std::thread::spawn(move || {
        LocalExecutor::default().run_singlethreaded(async {
            block_server_clone
                .connect_insecure_inline_encryption_server(server, superblock.uuid)
                .await
        })
    });

    let crypt_service =
        Arc::new(CryptService::new(&[1; 32], &[2; 32], Some(insecure_inline_crypto_proxy)));
    crypt_service.set_uuid(superblock.uuid);
    crypt_service.add_wrapping_key(&[0; 64], 0).expect("add wrapping key failed");

    let (crypt_client_end, crypt_proxy) = fidl::endpoints::create_endpoints::<CryptMarker>();

    let crypt_service_clone = Arc::clone(&crypt_service);
    fuchsia_async::Task::spawn(async move {
        if let Err(err) = crypt_service_clone.handle_connection(crypt_proxy.into_stream()).await {
            log::error!(err:?; "Crypt service failure");
        }
    })
    .detach();

    let crypt = Arc::new(RemoteCrypt::new(crypt_client_end)) as Arc<dyn Crypt>;

    Box::pin(migrate_device(offset, DeviceHolder::new(device), crypt.clone()))
        .await
        .expect("migrate_device");

    let (client, server_end) = fidl::endpoints::create_proxy::<BlockMarker>();
    let block_server_clone = block_server.clone();
    std::thread::spawn(move || {
        let mut executor = LocalExecutorBuilder::new().build();
        executor.run_singlethreaded(async move {
            if let Err(e) = block_server_clone.serve(server_end.into_stream()).await {
                log::error!("Server error: {:?}", e);
            }
        });
    });
    let device = DeviceHolder::new(
        BlockDevice::new(
            RemoteBlockClient::new(BlockProxy::new(client.into_channel().unwrap()))
                .await
                .expect("Unable to create block client"),
            false,
        )
        .await
        .unwrap(),
    );

    let fxfs = FxFilesystemBuilder::new()
        .read_only(true)
        .barriers_enabled(true)
        .open(device)
        .await
        .expect("open failed");

    let root_vol = fxfs_root_volume(fxfs.clone()).await.expect("Opening root volume");
    let vol = root_vol
        .volume("userdata", StoreOptions { crypt: Some(crypt.clone()), ..StoreOptions::default() })
        .await
        .expect("Opening volume");
    fxfs::fsck::fsck_volume(&fxfs, vol.store_object_id(), Some(crypt.clone()))
        .await
        .expect("fsck volume");
    let root_directory =
        Directory::open(&vol, vol.root_directory_object_id()).await.expect("open failed");

    // This is the data originally written into the file via our generation script.
    const EXPECTED_CONTENTS: &[u8] = b"test45678abcdef_12345678";
    // Confirm f2fs returns this data.
    assert_eq!(
        &expected_data.as_ref().unwrap().as_slice()[..EXPECTED_CONTENTS.len()],
        EXPECTED_CONTENTS
    );

    // Confirm fxfs also returns this data.
    let fxfs_object = ObjectStore::open_object(&vol, ino as u64, HandleOptions::default(), None)
        .await
        .expect("open object");
    if let Some(expected_data) = expected_data {
        let mut buf = fxfs_object.allocate_buffer(4096).await;
        assert_eq!(fxfs_object.read(0, buf.as_mut()).await.expect("read"), EXPECTED_CONTENTS.len());
        assert_eq!(
            &buf.to_vec()[..EXPECTED_CONTENTS.len()],
            &expected_data[..EXPECTED_CONTENTS.len()]
        );
    }
    Box::pin(verify(
        offset,
        fxfs.device(),
        &fxfs,
        superblock.root_ino,
        root_directory,
        /*check_file_contents=*/ true,
    ))
    .await
    .expect("verify");

    fxfs.close().await.expect("close ok");
}

#[fuchsia::test]
async fn test_fxfs_verify_encrypted_data() {
    let offset = 0;
    let block_server = create_device_with_image_at_offset("/pkg/testdata/f2fs.img.zst", offset);
    let (client, server_end) = fidl::endpoints::create_proxy::<BlockMarker>();
    let block_server_clone = block_server.clone();
    std::thread::spawn(move || {
        let mut executor = LocalExecutorBuilder::new().build();
        executor.run_singlethreaded(async move {
            if let Err(e) = block_server_clone.serve(server_end.into_stream()).await {
                log::error!("Server error: {:?}", e);
            }
        });
    });
    let device = Arc::new(
        BlockDevice::new(
            RemoteBlockClient::new(BlockProxy::new(client.into_channel().unwrap()))
                .await
                .expect("Unable to create block client"),
            false,
        )
        .await
        .unwrap(),
    );

    // Get UUID from F2FS.
    let (uuid, superblock) = {
        let block_size = device.block_size() as u64;
        let start_block = offset / block_size;
        let num_blocks = device.block_count() - start_block;
        let ranged_device = Arc::new(
            RangedDevice::new(
                device.clone(),
                start_block * block_size..(start_block + num_blocks) * block_size,
            )
            .expect("create ranged device"),
        );
        let f2fs = F2fsReader::open_device(ranged_device).await.expect("f2fs open ok");
        (f2fs.superblock().uuid, (*f2fs.superblock()).clone())
    };

    let device = Arc::try_unwrap(device).map_err(|_| ()).expect("only one ref to device");

    let block_server_clone = block_server.clone();
    let (insecure_inline_crypto_proxy, server) =
        fidl::endpoints::create_sync_proxy::<DeviceMarker>();
    std::thread::spawn(move || {
        LocalExecutor::default().run_singlethreaded(async {
            block_server_clone.connect_insecure_inline_encryption_server(server, uuid).await
        })
    });

    let crypt_service =
        Arc::new(CryptService::new(&[1; 32], &[2; 32], Some(insecure_inline_crypto_proxy)));
    crypt_service.set_uuid(uuid);
    crypt_service.add_wrapping_key(&[0; 64], 0).expect("add wrapping key failed");

    let (crypt_client_end, crypt_proxy) = fidl::endpoints::create_endpoints::<CryptMarker>();

    let crypt_service_clone = Arc::clone(&crypt_service);
    fuchsia_async::Task::spawn(async move {
        if let Err(err) = crypt_service_clone.handle_connection(crypt_proxy.into_stream()).await {
            log::error!(err:?; "Crypt service failure");
        }
    })
    .detach();

    let crypt = Arc::new(RemoteCrypt::new(crypt_client_end)) as Arc<dyn Crypt>;

    Box::pin(migrate_device(offset, DeviceHolder::new(device), crypt.clone()))
        .await
        .expect("migrate_device");

    let (client, server_end) = fidl::endpoints::create_proxy::<BlockMarker>();
    let block_server_clone = block_server.clone();
    std::thread::spawn(move || {
        let mut executor = LocalExecutorBuilder::new().build();
        executor.run_singlethreaded(async move {
            if let Err(e) = block_server_clone.serve(server_end.into_stream()).await {
                log::error!("Server error: {:?}", e);
            }
        });
    });
    let device = DeviceHolder::new(
        BlockDevice::new(
            RemoteBlockClient::new(BlockProxy::new(client.into_channel().unwrap()))
                .await
                .expect("Unable to create block client"),
            false,
        )
        .await
        .unwrap(),
    );

    let fxfs = FxFilesystemBuilder::new()
        .read_only(true)
        .barriers_enabled(true)
        .open(device)
        .await
        .expect("open failed");

    assert_eq!(&uuid, fxfs.super_block_header().guid.0.as_bytes());

    let root_vol = fxfs_root_volume(fxfs.clone()).await.expect("Opening root volume");
    let vol = root_vol
        .volume("userdata", StoreOptions { crypt: Some(crypt.clone()), ..StoreOptions::default() })
        .await
        .expect("Opening volume");
    fxfs::fsck::fsck_volume(&fxfs, vol.store_object_id(), Some(crypt.clone()))
        .await
        .expect("fsck volume");
    let root_directory =
        Directory::open(&vol, vol.root_directory_object_id()).await.expect("open failed");

    Box::pin(verify(
        offset,
        fxfs.device(),
        &fxfs,
        superblock.root_ino,
        root_directory,
        /*check_file_contents=*/ true,
    ))
    .await
    .expect("verify");
    fxfs.close().await.expect("close ok");
}
// Verifies that the directory tree in fxfs matches f2fs.
// If check_file_contents is true, opens f2fs and verifies file contents.
async fn verify(
    offset: u64,
    device: Arc<dyn Device>,
    fxfs: &OpenFxFilesystem,
    ino: u32,
    root_directory: Directory<ObjectStore>,
    check_file_contents: bool,
) -> Result<(), Error> {
    let block_size = device.block_size() as u64;
    let start_block = offset / block_size;
    let num_blocks = device.block_count() - start_block;
    let ranged_device = Arc::new(
        RangedDevice::new(
            device.clone(),
            start_block * block_size..(start_block + num_blocks) * block_size,
        )
        .context("RangedDevice::new")?,
    );
    let mut f2fs =
        F2fsReader::open_device(ranged_device).await.context("Failed to open f2fs image")?;
    f2fs.add_key(&[0; 64]);

    crate::verify(&f2fs, fxfs, ino, root_directory, check_file_contents).await
}
