// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![recursion_limit = "256"]

//! Tests for fuchsia.fshost/Recovery methods that manipulate volumes in the system container.
//! These methods are intended for use in recovery environments to enable fxblob-based products
//! to apply a system update or flash a new system image.

pub mod config;
use block_client::RemoteBlockClient;
use config::{blob_fs_type, data_fs_spec, data_fs_type, new_builder, volumes_spec};
use device_watcher::recursive_wait;
use fidl::endpoints::{ServiceMarker as _, create_proxy};
use fidl_fuchsia_fshost as ffshost;
use fidl_fuchsia_fxfs as ffxfs;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_storage_block as fblock;
use fshost_test_fixture::disk_builder::{
    Disk, expected_fxblob_volumes, list_all_fxfs_volumes, test_blob_hash,
};
use fshost_test_fixture::{TestFixture, write_blob};
use fuchsia_async as fasync;
use fuchsia_component::client::{connect_to_protocol_at_dir_root, connect_to_protocol_at_dir_svc};
use fxfs_make_blob_image::{CompressionAlgorithm, FxBlobBuilder};
use std::sync::Arc;
use storage_device::DeviceHolder;
use storage_device::block_device::BlockDevice;
use test_vmo_backed_block_server::VmoBackedServer;

const TEST_BLOBS: [&[u8]; 3] =
    ["Goodbye, stranger!".as_bytes(), "Hello, world!".as_bytes(), &['a' as u8; 16_384]];
const BLOCK_SIZE: u32 = 4096;
const NUM_BLOCKS: u64 = 512;
const DEVICE_SIZE: u64 = BLOCK_SIZE as u64 * NUM_BLOCKS;
const TRANSFER_BUFFER_SIZE: usize = 131_072;

/// Extension trait to help access the result of fuchsia.fshost/Recovery.GetBlobImageHandle when the
/// system container is expected to have been mounted successfully.
trait RecoveryGetBlobImageHandleResponseExt {
    fn as_mounted_system_container(self) -> (fio::FileProxy, zx::EventPair);
}

impl RecoveryGetBlobImageHandleResponseExt for ffshost::RecoveryGetBlobImageHandleResponse {
    fn as_mounted_system_container(self) -> (fio::FileProxy, zx::EventPair) {
        let system_container = match self {
            ffshost::RecoveryGetBlobImageHandleResponse::MountedSystemContainer(container) => {
                container
            }
            ffshost::RecoveryGetBlobImageHandleResponse::Unformatted(_) => {
                panic!("System container is unformatted!")
            }
        };
        let ffshost::MountedSystemContainer { image_file, mount_token } = system_container;
        (image_file.into_proxy(), mount_token)
    }
}

/// Initializes a disk to have an fxfs partition with a blob and data volume. The blob volume will
/// contain an initial blob.
async fn build_fxblob() -> Disk {
    let mut builder = new_builder();
    builder.with_disk().format_volumes(volumes_spec()).format_data(data_fs_spec()).with_gpt();
    let fixture = builder.build().await;
    fixture.check_fs_type("blob", blob_fs_type()).await;
    fixture.check_fs_type("data", data_fs_type()).await;
    fixture.check_test_blob().await;
    fixture.tear_down().await.unwrap()
}

/// Creates a new test fixture in what we consider recovery mode using `system_container` to
/// represent the underlying mutable storage.
async fn start_recovery_fixture(system_container: Disk) -> TestFixture {
    let mut builder = new_builder().with_disk_from(system_container);
    builder.fshost().set_config_value("ramdisk_image", true);
    builder.with_zbi_ramdisk().format_volumes(volumes_spec());
    let fixture = builder.build().await;
    // Wait for the zbi ramdisk filesystems
    fixture.check_fs_type("blob", blob_fs_type()).await;
    fixture.check_fs_type("data", data_fs_type()).await;
    // Also wait for any driver binding on the "on-disk" devices
    recursive_wait(
        &fixture.dir(
            fidl_fuchsia_storage_partitions::PartitionServiceMarker::SERVICE_NAME,
            fio::PERM_READABLE,
        ),
        "part-000",
    )
    .await
    .unwrap();
    fixture
}

#[fuchsia::test]
async fn mount_system_blob_volume() {
    let system_container = build_fxblob().await;
    let fixture = start_recovery_fixture(system_container).await;
    let recovery: ffshost::RecoveryProxy =
        fixture.realm.root.connect_to_protocol_at_exposed_dir().unwrap();

    // Invoke MountSystemBlobVolume
    let (blob_exposed_dir, blob_exposed_dir_server) = create_proxy::<fio::DirectoryMarker>();
    recovery
        .mount_system_blob_volume(blob_exposed_dir_server)
        .await
        .unwrap()
        .expect("MountSystemBlobVolume unexpectedly failed");
    let blob_svc =
        fuchsia_fs::directory::open_directory(&blob_exposed_dir, "svc", fio::PERM_READABLE)
            .await
            .unwrap();

    // We should still be able to read our test blob back.
    let blob_reader =
        connect_to_protocol_at_dir_root::<ffxfs::BlobReaderMarker>(&blob_svc).unwrap();
    blob_reader
        .get_vmo(test_blob_hash().as_bytes().try_into().unwrap())
        .await
        .unwrap()
        .expect("missing test blob");

    // Make sure we can create & write a blob with the BlobCreator.
    let data = "Hello, world!".as_bytes();
    let blob_creator =
        connect_to_protocol_at_dir_root::<ffxfs::BlobCreatorMarker>(&blob_svc).unwrap();
    let hash = write_blob(blob_creator, data).await;
    let hash = hash.as_bytes().try_into().unwrap();

    // Ensure we can read the blob back.
    let vmo = blob_reader.get_vmo(hash).await.unwrap().unwrap();
    let mut buff = vec![];
    buff.resize(data.len(), 0);
    vmo.read(&mut buff, 0).unwrap();
    assert_eq!(data, buff.as_slice());
    fixture.tear_down().await;
}

#[fuchsia::test]
async fn mount_system_blob_volume_with_format() {
    let system_container = build_fxblob().await;
    let fixture = start_recovery_fixture(system_container).await;
    let recovery: ffshost::RecoveryProxy =
        fixture.realm.root.connect_to_protocol_at_exposed_dir().unwrap();

    // Format the existing blob volume and then mount the newly created volume.
    recovery.format_system_blob_volume().await.unwrap().expect("FormatSystemBlobVolume failed");

    let (blob_exposed_dir, blob_exposed_dir_server) = create_proxy::<fio::DirectoryMarker>();
    recovery
        .mount_system_blob_volume(blob_exposed_dir_server)
        .await
        .unwrap()
        .expect("MountSystemBlobVolume failed");
    let blob_root = fuchsia_fs::directory::open_directory(
        &blob_exposed_dir,
        "root",
        fio::PERM_READABLE | fio::Flags::PERM_INHERIT_EXECUTE,
    )
    .await
    .unwrap();
    let blob_svc =
        fuchsia_fs::directory::open_directory(&blob_exposed_dir, "svc", fio::PERM_READABLE)
            .await
            .unwrap();

    // Verify there are no blobs since we should have formatted the blob volume.
    assert!(fuchsia_fs::directory::readdir(&blob_root).await.unwrap().is_empty());

    // Make sure we can create & write a blob with the BlobCreator.
    let data = "Hello, world!".as_bytes();
    let blob_creator =
        connect_to_protocol_at_dir_root::<ffxfs::BlobCreatorMarker>(&blob_svc).unwrap();
    let hash = write_blob(blob_creator, data).await;
    let hash = hash.as_bytes().try_into().unwrap();

    // Ensure we can read the blob back.
    let blob_reader =
        connect_to_protocol_at_dir_root::<ffxfs::BlobReaderMarker>(&blob_svc).unwrap();
    let vmo = blob_reader.get_vmo(hash).await.unwrap().unwrap();
    let mut buff = vec![];
    buff.resize(data.len(), 0);
    vmo.read(&mut buff, 0).unwrap();
    assert_eq!(data, buff.as_slice());

    fixture.tear_down().await;
}

#[fuchsia::test]
async fn mount_system_blob_volume_handles_corrupt_partition_table() {
    // Initialize the system GPT but don't actually format or initialize any volumes.
    let system_container = {
        let mut builder = new_builder();
        builder.with_disk().with_gpt();
        let fixture = builder.build().await;
        fixture.tear_down().await.unwrap()
    };
    let fixture = start_recovery_fixture(system_container).await;
    let recovery: ffshost::RecoveryProxy =
        fixture.realm.root.connect_to_protocol_at_exposed_dir().unwrap();

    // Both FormatSystemBlobVolume and MountSystemBlobVolume require a valid system container
    // partition to be present in the GPT and the system container must have a valid fxfs partition.
    let _ = recovery
        .format_system_blob_volume()
        .await
        .unwrap()
        .expect_err("FormatSystemBlobVolume succeeded on a corrupt filesystem!");

    let (_blob_exposed_dir, blob_exposed_dir_server) = create_proxy::<fio::DirectoryMarker>();
    let _ = recovery
        .mount_system_blob_volume(blob_exposed_dir_server)
        .await
        .unwrap()
        .expect_err("MountSystemBlobVolume succeeded on a corrupt filesystem!");
    fixture.tear_down().await;
}

#[fuchsia::test]
async fn mount_system_blob_volume_lifecycle() {
    let system_container = build_fxblob().await;
    let fixture = start_recovery_fixture(system_container).await;
    let recovery: ffshost::RecoveryProxy =
        fixture.realm.root.connect_to_protocol_at_exposed_dir().unwrap();

    // Invoke MountSystemBlobVolume
    let (blob_exposed_dir, blob_exposed_dir_server) = create_proxy::<fio::DirectoryMarker>();
    recovery
        .mount_system_blob_volume(blob_exposed_dir_server)
        .await
        .unwrap()
        .expect("MountSystemBlobVolume unexpectedly failed");
    let blob_svc =
        fuchsia_fs::directory::open_directory(&blob_exposed_dir, "svc", fio::PERM_READABLE)
            .await
            .unwrap();

    // Make sure the BlobReader is working and being served.
    let hash = test_blob_hash();
    let hash = hash.as_bytes().try_into().unwrap();
    let blob_reader =
        connect_to_protocol_at_dir_root::<ffxfs::BlobReaderMarker>(&blob_svc).unwrap();
    blob_reader.get_vmo(hash).await.unwrap().expect("missing blob");

    // Ensure we shut the filesystem down when we close (or drop) the last connection to the exposed
    // directory. The BlobReader service should start failing soon afterwards.
    blob_exposed_dir.close().await.unwrap().expect("close failed");
    loop {
        if let Err(e) = blob_reader.get_vmo(hash).await {
            assert!(e.is_closed());
            break;
        }
        fasync::Timer::new(std::time::Duration::from_millis(100)).await;
    }

    fixture.tear_down().await;
}

/// Ensure multiple requests to mount the system blob volume are handled gracefully. While a caller
/// still has an open handle to the system container, new mount requests should remain pending.
#[fuchsia::test]
async fn mount_system_blob_volume_handles_concurrency() {
    let system_container = build_fxblob().await;
    let fixture = start_recovery_fixture(system_container).await;

    async fn write_blob_and_get_hash(fixture: &TestFixture, data: &str) -> fuchsia_hash::Hash {
        let recovery: ffshost::RecoveryProxy =
            fixture.realm.root.connect_to_protocol_at_exposed_dir().unwrap();
        let (blob_exposed_dir, blob_exposed_dir_server) = create_proxy::<fio::DirectoryMarker>();
        recovery
            .mount_system_blob_volume(blob_exposed_dir_server)
            .await
            .unwrap()
            .expect("MountSystemBlobVolume unexpectedly failed");
        let blob_svc =
            fuchsia_fs::directory::open_directory(&blob_exposed_dir, "svc", fio::PERM_READABLE)
                .await
                .unwrap();
        let blob_creator =
            connect_to_protocol_at_dir_root::<ffxfs::BlobCreatorMarker>(&blob_svc).unwrap();
        write_blob(blob_creator, data.as_bytes()).await
    }

    // Although the protocol only allows a single instance to access the system container, we should
    // be able to gracefully wait for each future to complete. The recovery method should only
    // return once a previous iteration has completed and the filesystem was shut down cleanly.
    let blob_data = [
        "Goodbye stranger",
        "It's been nice...",
        "And I think my spaceship knows what I must do",
        "ground control to Major Tom",
    ];
    let futures: Vec<_> =
        blob_data.into_iter().map(|data| write_blob_and_get_hash(&fixture, data)).collect();
    let hashes = futures::future::join_all(futures).await;

    // Ensure we can read back all of the blobs we just wrote.
    let recovery: ffshost::RecoveryProxy =
        fixture.realm.root.connect_to_protocol_at_exposed_dir().unwrap();
    let (blob_exposed_dir, blob_exposed_dir_server) = create_proxy::<fio::DirectoryMarker>();
    recovery
        .mount_system_blob_volume(blob_exposed_dir_server)
        .await
        .unwrap()
        .expect("MountSystemBlobVolume unexpectedly failed");
    let blob_reader =
        connect_to_protocol_at_dir_svc::<ffxfs::BlobReaderMarker>(&blob_exposed_dir).unwrap();
    for hash in hashes {
        blob_reader
            .get_vmo(hash.as_bytes().try_into().unwrap())
            .await
            .unwrap()
            .expect("blob missing");
    }

    fixture.tear_down().await;
}

/// Helper function that creates a new fxfs system image containing a single blob volume with
/// some test blobs.
async fn create_fxblob_image() -> zx::Vmo {
    let fxblob_vmo = zx::Vmo::create(DEVICE_SIZE).unwrap();
    let image_size = {
        let block_server = Arc::new(
            VmoBackedServer::from_vmo(
                BLOCK_SIZE,
                fxblob_vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
            )
            .unwrap(),
        );
        let device = DeviceHolder::new(
            BlockDevice::new(
                RemoteBlockClient::new(block_server.connect::<fblock::BlockProxy>())
                    .await
                    .expect("Unable to create block client"),
                false,
            )
            .await
            .unwrap(),
        );
        let fxblob = FxBlobBuilder::new(device).await.unwrap();
        for data in TEST_BLOBS {
            let blob =
                fxblob.generate_blob(data.to_vec(), Some(CompressionAlgorithm::Zstd)).unwrap();
            fxblob.install_blob(&blob).await.unwrap();
        }
        fxblob.finalize().await.unwrap().1
    };
    // Truncate the VMO to the size of the image.
    fxblob_vmo.set_stream_size(image_size).unwrap();
    fxblob_vmo
}

async fn copy_image_to_file(image: &zx::Vmo, file: &fio::FileProxy) {
    file.resize(DEVICE_SIZE).await.expect("transport error").expect("resize failed");
    let target = file
        .get_backing_memory(fio::VmoFlags::WRITE | fio::VmoFlags::SHARED_BUFFER)
        .await
        .expect("transport error")
        .map_err(zx::Status::err_from_raw)
        .unwrap();

    let mut buffer = vec![0u8; TRANSFER_BUFFER_SIZE];
    let mut offset = 0;
    let image_size = image.get_stream_size().unwrap();
    while offset < image_size {
        let end = std::cmp::min(offset + TRANSFER_BUFFER_SIZE as u64, image_size);
        let amount = end - offset;
        image.read(&mut buffer[0..amount as usize], offset).unwrap();
        target.write(&buffer[0..amount as usize], offset).unwrap();
        offset += amount;
    }

    file.sync().await.expect("transport error").expect("sync failed");
}

/// Verifies that the set of [`TEST_BLOBS`] are present in the installed system image on `disk`,
/// that the `data` volume still exists and contains the test data file, and that there are no
/// unexpected volumes (e.g. that any uninstalled blob volumes are cleaned up).
async fn verify_installed_fxblob_system(disk: Disk) {
    let fixture = new_builder().with_disk_from(disk).build().await;
    fixture.check_fs_type("data", data_fs_type()).await;
    fixture.check_test_data_file().await;
    fixture.check_fs_type("blob", blob_fs_type()).await;

    let reader = connect_to_protocol_at_dir_root::<ffxfs::BlobReaderMarker>(
        fixture.realm.root.get_exposed_dir(),
    )
    .expect("failed to connect to the BlobReader");

    // The original blob should be missing.
    assert_eq!(
        reader
            .get_vmo(&test_blob_hash().into())
            .await
            .unwrap()
            .map_err(zx::Status::err_from_raw)
            .expect_err("old blob should be gone"),
        zx::Status::NOT_FOUND
    );

    // We should be able to find and read the blobs we installed.
    for data in TEST_BLOBS {
        let hash = fuchsia_merkle::root_from_slice(data);
        let vmo = reader
            .get_vmo(&hash.as_bytes().try_into().unwrap())
            .await
            .unwrap()
            .map_err(zx::Status::err_from_raw)
            .expect("GetVmo failed");
        let mut buf = vec![0; data.len()];
        vmo.read(&mut buf, 0).unwrap();
        assert_eq!(buf.as_slice(), data);
    }

    // We should only have the set of volumes we expect - any uninstalled blob volumes should have
    // been removed when we mounted the filesystem in fshost.
    let system_container = fixture.tear_down().await.unwrap();
    let volumes = list_all_fxfs_volumes(&system_container).await;
    assert!(volumes.len() == expected_fxblob_volumes().len());
}

/// Test writing and installing a new system blob volume.
#[fuchsia::test]
async fn write_and_install_blob_image() {
    let system_container = build_fxblob().await;
    let fixture = start_recovery_fixture(system_container).await;
    let image = create_fxblob_image().await;
    let recovery: ffshost::RecoveryProxy =
        fixture.realm.root.connect_to_protocol_at_exposed_dir().unwrap();
    let (image_file, mount_token) = recovery
        .get_blob_image_handle()
        .await
        .unwrap()
        .expect("get_blob_image_handle returned error")
        .as_mounted_system_container();
    copy_image_to_file(&image, &image_file).await;

    // Drop the mount token and try to install the image.
    std::mem::drop(mount_token);
    recovery
        .install_blob_image()
        .await
        .unwrap()
        .map_err(zx::Status::err_from_raw)
        .expect("volume installation failed");

    // Tear down the fixture, mount the system image as normal, and verify the blobs.
    let system_container = fixture.tear_down().await.unwrap();
    verify_installed_fxblob_system(system_container).await;
}

/// Ensure that if we fail to install a blob image, we can re-attempt the process without rebooting.
#[fuchsia::test]
async fn write_and_install_blob_image_can_reattempt() {
    let system_container = build_fxblob().await;
    let fixture = start_recovery_fixture(system_container).await;
    let image = create_fxblob_image().await;
    let recovery: ffshost::RecoveryProxy =
        fixture.realm.root.connect_to_protocol_at_exposed_dir().unwrap();

    // Write the image file as normal, but then truncate it to invalidate the image.
    {
        let (image_file, _mount_token) = recovery
            .get_blob_image_handle()
            .await
            .unwrap()
            .expect("get_blob_image_handle returned error")
            .as_mounted_system_container();
        copy_image_to_file(&image, &image_file).await;
        // Truncate the file so the image is invalid.
        image_file.resize(8192).await.expect("transport error").expect("resize failed");
        image_file.sync().await.expect("transport error").expect("sync failed");
    }

    // We should fail to install the invalid image.
    recovery
        .install_blob_image()
        .await
        .unwrap()
        .map_err(zx::Status::err_from_raw)
        .expect_err("installation should fail if the image is incomplete");

    // Now write the image file in full.
    {
        let (image_file, _mount_token) = recovery
            .get_blob_image_handle()
            .await
            .unwrap()
            .expect("get_blob_image_handle returned error")
            .as_mounted_system_container();
        copy_image_to_file(&image, &image_file).await;
    }

    recovery
        .install_blob_image()
        .await
        .unwrap()
        .map_err(zx::Status::err_from_raw)
        .expect("volume installation failed");

    // Tear down the fixture, mount the system image as normal, and verify the blobs.
    let system_container = fixture.tear_down().await.unwrap();
    verify_installed_fxblob_system(system_container).await;
}

/// Verifies that if we write a new blob volume image but do not install it, it gets installed
/// automatically on the next boot.
#[fuchsia::test]
async fn new_blob_volume_is_installed_on_normal_boot() {
    let system_container = build_fxblob().await;
    let fixture = start_recovery_fixture(system_container).await;
    let image = create_fxblob_image().await;
    let recovery: ffshost::RecoveryProxy =
        fixture.realm.root.connect_to_protocol_at_exposed_dir().unwrap();
    let (image_file, _mount_token) = recovery
        .get_blob_image_handle()
        .await
        .unwrap()
        .expect("get_blob_image_handle returned error")
        .as_mounted_system_container();
    copy_image_to_file(&image, &image_file).await;
    // Ensure that when we tear down the fixture, the installation volume is still present. When
    // we re-mount the filesystem for verification, the new blob volume should be installed
    // automatically.
    let system_container = fixture.tear_down().await.unwrap();
    let volumes = list_all_fxfs_volumes(&system_container).await;
    assert!(volumes.len() > expected_fxblob_volumes().len(), "install volume missing");
    verify_installed_fxblob_system(system_container).await;
}

/// Verifies that if we write an incomplete blob volume image and do not install it, it is cleaned
/// up automatically on the next boot.
#[fuchsia::test]
async fn new_blob_volume_is_cleaned_up_on_install_failure() {
    let system_container = build_fxblob().await;
    let fixture = start_recovery_fixture(system_container).await;
    let recovery: ffshost::RecoveryProxy =
        fixture.realm.root.connect_to_protocol_at_exposed_dir().unwrap();

    // Write a truncated version of a valid fxblob image. This should fail to install since we
    // haven't written the entire image.
    {
        let truncated_image = {
            let image = create_fxblob_image().await;
            image.set_stream_size(2 * BLOCK_SIZE as u64).unwrap();
            image
        };
        let (image_file, _mount_token) = recovery
            .get_blob_image_handle()
            .await
            .unwrap()
            .expect("get_blob_image_handle returned error")
            .as_mounted_system_container();
        copy_image_to_file(&truncated_image, &image_file).await;
    }

    // Tear down the fixture, and inspect the volumes in the filesystem. We should see an extra
    // volume containing the truncated blob image that has not yet been installed.
    let system_container = fixture.tear_down().await.unwrap();
    let volumes = list_all_fxfs_volumes(&system_container).await;
    assert!(volumes.len() > expected_fxblob_volumes().len());

    // Now boot up fshost normally, which should trigger an attempt to install our truncated image.
    // This should fail, deleting the install volume, and leaving the existing blob volume intact.
    let fixture = new_builder().with_disk_from(system_container).build().await;
    // Ensure the original system blobs are still present.
    fixture.check_test_blob().await;
    // Ensure the install volume is gone.
    let system_container = fixture.tear_down().await.unwrap();
    let volumes = list_all_fxfs_volumes(&system_container).await;
    assert_eq!(volumes, expected_fxblob_volumes());
}

/// Verifies that fuchsia.fshost/Recovery.GetBlobImageHandle will detect incorrect disk formats.
/// This allows callers to take remedial action if they suspect the system was never provisioned.
#[fuchsia::test]
async fn get_blob_image_handle_formats_unformatted_disk() {
    let mut builder = new_builder();
    builder.with_disk().with_gpt();
    let system_container = builder.build().await.tear_down().await.unwrap();
    let fixture = start_recovery_fixture(system_container).await;
    let recovery: ffshost::RecoveryProxy =
        fixture.realm.root.connect_to_protocol_at_exposed_dir().unwrap();
    let response = recovery
        .get_blob_image_handle()
        .await
        .unwrap()
        .expect("get_blob_image_handle should succeed with unprovisioned disk");
    assert!(matches!(response, ffshost::RecoveryGetBlobImageHandleResponse::Unformatted(_)));
    fixture.tear_down().await.unwrap();
}
