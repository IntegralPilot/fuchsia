// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use assert_matches::assert_matches;
use blob_writer::BlobWriter;
use crypt_policy as _;
use delivery_blob::{CompressionMode, Type1Blob};
use fidl::endpoints::{DiscoverableProtocolMarker as _, ServiceMarker as _};
use fidl_fuchsia_fs_startup::VolumeMarker as FsStartupVolumeMarker;
use fidl_fuchsia_fshost::{AdminProxy, RecoveryProxy};
use fidl_fuchsia_fxfs::{BlobCreatorProxy, BlobReaderProxy};
use fidl_fuchsia_io as fio;
use fidl_fuchsia_storage_block as fpartition;
use fidl_fuchsia_storage_block::BlockMarker;
use fidl_fuchsia_storage_partitions as fpartitions;
use fidl_fuchsia_update_verify::HealthStatus;
use fshost_test_fixture::disk_builder::{
    BLOBFS_MAX_BYTES, DataSpec, Disk, DiskBuilder, TEST_DISK_BLOCK_SIZE, VolumesSpec,
};
use fshost_test_fixture::{
    BlockDeviceConfig, BlockDeviceIdentifiers, BlockDeviceParent, TestFixture, VFS_TYPE_MEMFS,
    round_down,
};
use fuchsia_async as fasync;

fn make_partition_entry(
    name: &str,
    type_guid: fpartition::Guid,
    instance_guid: fpartition::Guid,
    start_block: u64,
    num_blocks: u64,
    flags: u64,
) -> fpartitions::PartitionEntry {
    fpartitions::PartitionEntry {
        name: name.to_string(),
        type_guid,
        instance_guid,
        start_block,
        num_blocks,
        flags,
    }
}

#[allow(unused)]
fn make_partition_info(
    name: &str,
    type_guid: fpartition::Guid,
    instance_guid: fpartition::Guid,
    start_block_offset: u64,
    num_blocks: u64,
    flags: u64,
) -> fpartitions::PartitionInfo {
    fpartitions::PartitionInfo {
        name: Some(name.to_string()),
        type_guid: Some(type_guid),
        instance_guid: Some(instance_guid),
        start_block_offset: Some(start_block_offset),
        num_blocks: Some(num_blocks),
        flags: Some(flags),
        ..Default::default()
    }
}

use fuchsia_component::client::connect_to_named_protocol_at_dir_root;
use futures::FutureExt as _;
use regex::Regex;

pub mod config;

use config::{
    blob_fs_type, data_fs_spec, data_fs_type, data_fs_zxcrypt, data_max_bytes, fvm_slice_size,
    new_builder, volumes_spec,
};

#[fuchsia::test]
async fn blobfs_and_data_mounted() {
    let mut builder = new_builder();
    builder.with_disk().format_volumes(volumes_spec()).format_data(data_fs_spec());
    let fixture = builder.build().await;

    fixture.check_fs_type("blob", blob_fs_type()).await;
    fixture.check_fs_type("blob-exec", blob_fs_type()).await;
    fixture.check_fs_type("data", data_fs_type()).await;
    // Also make sure tmpfs is getting exported.
    fixture.check_fs_type("tmp", VFS_TYPE_MEMFS).await;
    fixture.check_test_data_file().await;
    fixture.check_test_blob().await;

    let blob_dir =
        fixture.dir("blob-exec", fio::PERM_READABLE | fio::PERM_WRITABLE | fio::PERM_EXECUTABLE);
    assert!(fuchsia_fs::directory::readdir(&blob_dir).await.unwrap().len() > 0);

    fixture.tear_down().await;
}

#[fuchsia::test]
async fn blobfs_and_data_mounted_with_extra_volume() {
    let mut builder = new_builder();
    builder
        .with_disk()
        .format_volumes(volumes_spec())
        .format_data(data_fs_spec())
        .with_extra_volume("internal");
    let fixture = builder.build().await;

    fixture.check_fs_type("blob", blob_fs_type()).await;
    fixture.check_fs_type("blob-exec", blob_fs_type()).await;
    fixture.check_fs_type("data", data_fs_type()).await;
    fixture.check_test_data_file().await;
    fixture.check_test_blob().await;

    fixture.tear_down().await;
}

#[fuchsia::test]
async fn blobfs_and_data_mounted_legacy_label() {
    let mut builder = new_builder();
    builder
        .with_disk()
        .format_volumes(volumes_spec())
        .format_data(data_fs_spec())
        .with_legacy_data_label();
    let fixture = builder.build().await;

    fixture.check_fs_type("blob", blob_fs_type()).await;
    fixture.check_fs_type("data", data_fs_type()).await;
    fixture.check_test_data_file().await;
    fixture.check_test_blob().await;

    fixture.tear_down().await;
}

#[fuchsia::test]
async fn data_formatted() {
    let mut builder = new_builder();
    builder.with_disk().format_volumes(volumes_spec());
    let fixture = builder.build().await;

    fixture.check_fs_type("blob", blob_fs_type()).await;
    fixture.check_fs_type("data", data_fs_type()).await;
    fixture.check_test_blob().await;

    fixture.tear_down().await;
}

#[fuchsia::test]
async fn data_partition_nonexistent() {
    let mut builder = new_builder();
    builder
        .with_disk()
        .format_volumes(VolumesSpec { create_data_partition: false, ..volumes_spec() });
    let fixture = builder.build().await;

    fixture.check_fs_type("blob", blob_fs_type()).await;
    fixture.check_fs_type("data", data_fs_type()).await;
    fixture.check_test_blob().await;

    fixture.tear_down().await;
}

#[fuchsia::test]
async fn data_formatted_legacy_label() {
    let mut builder = new_builder();
    builder.with_disk().format_volumes(volumes_spec()).with_legacy_data_label();
    let fixture = builder.build().await;

    fixture.check_fs_type("blob", blob_fs_type()).await;
    fixture.check_fs_type("data", data_fs_type()).await;

    fixture.tear_down().await;
}

#[fuchsia::test]
async fn data_formatted_no_fuchsia_boot() {
    let mut builder = new_builder().no_fuchsia_boot();
    builder.with_disk().format_volumes(volumes_spec());
    let fixture = builder.build().await;

    fixture.check_fs_type("blob", blob_fs_type()).await;
    fixture.check_fs_type("data", data_fs_type()).await;

    fixture.tear_down().await;
}

#[fuchsia::test]
async fn data_formatted_with_small_initial_volume() {
    let mut builder = new_builder();
    builder.with_disk().format_volumes(volumes_spec()).data_volume_size(fvm_slice_size());
    let fixture = builder.build().await;

    fixture.check_fs_type("blob", blob_fs_type()).await;
    fixture.check_fs_type("data", data_fs_type()).await;

    fixture.tear_down().await;
}

#[fuchsia::test]
async fn data_formatted_with_small_initial_volume_big_target() {
    let mut builder = new_builder();
    // The formatting uses the max bytes argument as the initial target to resize to. If this
    // target is larger than the disk, the resize should still succeed.
    builder.fshost().set_config_value("data_max_bytes", data_max_bytes() * 2);
    builder.with_disk().format_volumes(volumes_spec()).data_volume_size(fvm_slice_size());
    let fixture = builder.build().await;

    fixture.check_fs_type("blob", blob_fs_type()).await;
    fixture.check_fs_type("data", data_fs_type()).await;

    fixture.tear_down().await;
}

#[fuchsia::test]
async fn ramdisk_blob_and_data_mounted() {
    let mut builder = new_builder();
    builder.fshost().set_config_value("ramdisk_image", true);
    builder
        .with_zbi_ramdisk()
        .format_volumes(volumes_spec())
        .format_data(DataSpec { zxcrypt: false, ..data_fs_spec() });
    let fixture = builder.build().await;

    fixture.check_fs_type("blob", blob_fs_type()).await;
    fixture.check_fs_type("data", data_fs_type()).await;
    fixture.check_test_data_file().await;
    fixture.check_test_blob().await;

    fixture.tear_down().await;
}

#[fuchsia::test]
async fn ramdisk_blob_and_data_mounted_no_existing_data_partition() {
    let mut builder = new_builder();
    builder.fshost().set_config_value("ramdisk_image", true);
    builder
        .with_zbi_ramdisk()
        .format_volumes(VolumesSpec { create_data_partition: false, ..volumes_spec() })
        .format_data(DataSpec { zxcrypt: false, ..data_fs_spec() });
    let fixture = builder.build().await;

    fixture.check_fs_type("blob", blob_fs_type()).await;
    fixture.check_fs_type("data", data_fs_type()).await;
    fixture.check_test_blob().await;

    fixture.tear_down().await;
}

#[fuchsia::test]
async fn ramdisk_data_ignores_non_ramdisk() {
    let mut builder = new_builder();
    builder.fshost().set_config_value("ramdisk_image", true);
    builder
        .with_disk()
        .format_volumes(volumes_spec())
        .format_data(DataSpec { zxcrypt: false, ..data_fs_spec() });
    let fixture = builder.build().await;

    // There isn't really a good way to tell that something is not mounted, but at this point we
    // would be pretty close to it, so a timeout of a couple seconds should safeguard against
    // potential issues.
    futures::select! {
        _ = fixture.check_fs_type("data", data_fs_type()).fuse() => {
            panic!("check_fs_type returned unexpectedly - data was mounted");
        },
        _ = fixture.check_fs_type("blob", blob_fs_type()).fuse() => {
            panic!("check_fs_type returned unexpectedly - blob was mounted");
        },
        _ = fasync::Timer::new(std::time::Duration::from_secs(2)).fuse() => (),
    }

    fixture.tear_down().await;
}

#[fuchsia::test]
async fn set_volume_limit() {
    let mut builder = new_builder();
    builder
        .fshost()
        .set_config_value("data_max_bytes", data_max_bytes())
        .set_config_value("blob_max_bytes", BLOBFS_MAX_BYTES);
    builder.with_disk().format_volumes(volumes_spec()).format_data(data_fs_spec());
    let fixture = builder.build().await;

    fixture.check_fs_type("blob", blob_fs_type()).await;
    fixture.check_fs_type("data", data_fs_type()).await;

    let volumes_dir = fixture.dir("volumes", fio::PERM_READABLE);
    let blob_volume_name = if cfg!(feature = "fxblob") { "blob" } else { "blobfs" };
    let blob_volume_proxy = connect_to_named_protocol_at_dir_root::<FsStartupVolumeMarker>(
        &volumes_dir,
        blob_volume_name,
    )
    .unwrap();
    let blobfs_limit =
        blob_volume_proxy.get_limit().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
    let expected_blobfs_limit = if cfg!(feature = "fxblob") {
        BLOBFS_MAX_BYTES
    } else {
        // The fvm component rounds the max bytes down to the nearest slice size.
        round_down(BLOBFS_MAX_BYTES, fvm_slice_size())
    };
    assert_eq!(blobfs_limit, expected_blobfs_limit);
    let data_volume_proxy =
        connect_to_named_protocol_at_dir_root::<FsStartupVolumeMarker>(&volumes_dir, "data")
            .unwrap();
    let data_limit =
        data_volume_proxy.get_limit().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
    let expected_data_limit = if cfg!(feature = "fxblob") {
        data_max_bytes()
    } else if data_fs_zxcrypt() {
        // The fvm component rounds the max bytes down to the nearest slice size, and fshost adds
        // an additional slice to account for the zxcrypt metadata.
        round_down(data_max_bytes(), fvm_slice_size()) + fvm_slice_size()
    } else {
        // The fvm component rounds the max bytes down to the nearest slice size.
        round_down(data_max_bytes(), fvm_slice_size())
    };
    assert_eq!(data_limit, expected_data_limit);

    fixture.tear_down().await;
}

#[fuchsia::test]
async fn set_data_and_blob_max_bytes_zero() {
    let mut builder = new_builder();
    builder.fshost().set_config_value("data_max_bytes", 0).set_config_value("blob_max_bytes", 0);
    builder.with_disk().format_volumes(volumes_spec());
    let fixture = builder.build().await;

    fixture.check_fs_type("blob", blob_fs_type()).await;
    fixture.check_fs_type("data", data_fs_type()).await;
    let flags = fio::Flags::FLAG_MAYBE_CREATE | fio::PERM_READABLE | fio::PERM_WRITABLE;

    let data_root = fixture.dir("data", flags);
    let file = fuchsia_fs::directory::open_file(&data_root, "file", flags).await.unwrap();
    fuchsia_fs::file::write(&file, "file contents!").await.unwrap();

    let blob_contents = vec![0; 8192];
    let hash = fuchsia_merkle::root_from_slice(&blob_contents);
    let compressed_data: Vec<u8> = Type1Blob::generate(&blob_contents, CompressionMode::Always);

    let blob_proxy: BlobCreatorProxy = fixture
        .realm
        .root
        .connect_to_protocol_at_exposed_dir()
        .expect("connect_to_protocol_at_exposed_dir failed");

    let writer_client_end = blob_proxy
        .create(&hash.into(), false)
        .await
        .expect("transport error on BlobCreator.Create")
        .expect("failed to create blob");
    let writer = writer_client_end.into_proxy();
    let mut blob_writer = BlobWriter::create(writer, compressed_data.len() as u64)
        .await
        .expect("failed to create BlobWriter");
    blob_writer.write(&compressed_data).await.unwrap();

    fixture.tear_down().await;
}

#[fuchsia::test]
async fn ramdisk_image_set() {
    // Set the ramdisk_image flag
    let mut builder = new_builder();
    builder.fshost().set_config_value("ramdisk_image", true);
    builder.with_disk().format_volumes(volumes_spec());
    let fixture = builder.build().await;

    // Use the same approach as ramdisk_data_ignores_non_ramdisk() to ensure that
    // neither blobfs nor data were mounted using a timeout
    futures::select! {
        _ = fixture.check_fs_type("data", data_fs_type()).fuse() => {
            panic!("check_fs_type returned unexpectedly - data was mounted");
        },
        _ = fixture.check_fs_type("blob", blob_fs_type()).fuse() => {
            panic!("check_fs_type returned unexpectedly - blob was mounted");
        },
        _ = fasync::Timer::new(std::time::Duration::from_secs(2)).fuse() => {
        },
    }

    fixture.tear_down().await;
}

#[fuchsia::test]
async fn ramdisk_image_serves_zbi_ramdisk_contents_with_unformatted_data() {
    let mut builder = new_builder();
    builder.fshost().set_config_value("ramdisk_image", true);
    builder.with_zbi_ramdisk().format_volumes(volumes_spec());
    let fixture = builder.build().await;

    fixture.check_fs_type("blob", blob_fs_type()).await;
    fixture.check_fs_type("data", data_fs_type()).await;

    fixture.tear_down().await;
}

#[fuchsia::test]
#[cfg_attr(not(any(feature = "f2fs", feature = "minfs-no-zxcrypt")), ignore)]
async fn shred_data_volume_not_supported() {
    let mut builder = new_builder();
    builder.with_disk().format_volumes(volumes_spec());
    let fixture = builder.build().await;

    let admin: AdminProxy = fixture
        .realm
        .root
        .connect_to_protocol_at_exposed_dir()
        .expect("connect_to_protcol_at_exposed_dir failed");

    let status = admin
        .shred_data_volume()
        .await
        .expect("shred_data_volume FIDL failed")
        .expect_err("shred_data_volume should fail");
    assert_eq!(zx::Status::ok(status), Err(zx::Status::NOT_SUPPORTED));

    fixture.tear_down().await;
}

#[fuchsia::test]
#[cfg_attr(any(feature = "f2fs", feature = "minfs-no-zxcrypt"), ignore)]
async fn shred_data_volume_when_mounted() {
    let mut builder = new_builder();
    builder.with_disk().format_volumes(volumes_spec());
    let fixture = builder.build().await;

    fuchsia_fs::directory::open_file(
        &fixture.dir("data", fio::PERM_READABLE | fio::PERM_WRITABLE),
        "test-file",
        fio::Flags::FLAG_MAYBE_CREATE,
    )
    .await
    .expect("open_file failed");

    let admin: AdminProxy = fixture
        .realm
        .root
        .connect_to_protocol_at_exposed_dir()
        .expect("connect_to_protcol_at_exposed_dir failed");

    admin
        .shred_data_volume()
        .await
        .expect("shred_data_volume FIDL failed")
        .expect("shred_data_volume failed");

    let disk = fixture.tear_down().await.unwrap();

    let fixture = new_builder().with_disk_from(disk).build().await;

    // If we try and open the same test file, it shouldn't exist because the data volume should have
    // been shredded.
    assert_matches!(
        fuchsia_fs::directory::open_file(
            &fixture.dir("data", fio::PERM_READABLE),
            "test-file",
            fio::PERM_READABLE,
        )
        .await
        .expect_err("open_file failed"),
        fuchsia_fs::node::OpenError::OpenError(zx::Status::NOT_FOUND)
    );

    fixture.tear_down().await;
}

#[fuchsia::test]
#[cfg_attr(any(feature = "f2fs", feature = "minfs-no-zxcrypt"), ignore)]
async fn shred_data_volume_from_recovery() {
    let mut builder = new_builder();
    builder.with_disk().with_gpt().format_volumes(volumes_spec());
    let fixture = builder.build().await;

    fuchsia_fs::directory::open_file(
        &fixture.dir("data", fio::PERM_READABLE | fio::PERM_WRITABLE),
        "test-file",
        fio::Flags::FLAG_MAYBE_CREATE,
    )
    .await
    .expect("open_file failed");

    let disk = fixture.tear_down().await.unwrap();

    // Launch a version of fshost that will behave like recovery: it will mount data and blob from
    // a ramdisk it launches, binding the fvm on the "regular" disk but otherwise leaving it alone.
    let mut builder = new_builder().with_disk_from(disk);
    builder.fshost().set_config_value("ramdisk_image", true);
    builder.with_zbi_ramdisk().format_volumes(volumes_spec());
    let fixture = builder.build().await;

    let admin: AdminProxy = fixture
        .realm
        .root
        .connect_to_protocol_at_exposed_dir()
        .expect("connect_to_protcol_at_exposed_dir failed");

    admin
        .shred_data_volume()
        .await
        .expect("shred_data_volume FIDL failed")
        .expect("shred_data_volume failed");

    let disk = fixture.tear_down().await.unwrap();

    let fixture = new_builder().with_disk_from(disk).build().await;

    // If we try and open the same test file, it shouldn't exist because the data volume should have
    // been shredded.
    assert_matches!(
        fuchsia_fs::directory::open_file(
            &fixture.dir("data", fio::PERM_READABLE),
            "test-file",
            fio::PERM_READABLE
        )
        .await
        .expect_err("open_file failed"),
        fuchsia_fs::node::OpenError::OpenError(zx::Status::NOT_FOUND)
    );

    fixture.tear_down().await;
}

#[fuchsia::test]
async fn disable_block_watcher() {
    let mut builder = new_builder();
    builder.fshost().set_config_value("disable_block_watcher", true);
    builder.with_disk().format_volumes(volumes_spec()).format_data(data_fs_spec());
    let fixture = builder.build().await;

    // The filesystems are not mounted when the block watcher is disabled.
    futures::select! {
        _ = fixture.check_fs_type("data", data_fs_type()).fuse() => {
            panic!("check_fs_type returned unexpectedly - data was mounted");
        },
        _ = fixture.check_fs_type("blob", blob_fs_type()).fuse() => {
            panic!("check_fs_type returned unexpectedly - blob was mounted");
        },
        _ = fasync::Timer::new(std::time::Duration::from_secs(2)).fuse() => (),
    }

    fixture.tear_down().await;
}

async fn assert_volumes_are_expected(fixture: &TestFixture) {
    let (volumes_dir, expected) = if cfg!(feature = "fxfs") {
        (fixture.dir("volumes", fio::PERM_READABLE), vec![r"^blob$", r"^data$", r"^unencrypted$"])
    } else {
        (fixture.dir("volumes", fio::PERM_READABLE), vec![r"^blobfs$", r"^data$"])
    };

    let mut expected: Vec<_> = expected.into_iter().map(|r| Regex::new(r).unwrap()).collect();

    // Ensure that the account and virtualization volumes were successfully destroyed. The volumes
    // are removed from devfs asynchronously, so use a timeout.
    let start_time = std::time::Instant::now();
    let mut dir_entries =
        fuchsia_fs::directory::readdir(&volumes_dir).await.expect("Failed to readdir the volumes");
    while dir_entries
        .iter()
        .find(|x| x.name.contains("account") || x.name.contains("virtualization"))
        .is_some()
    {
        let elapsed = start_time.elapsed().as_secs() as u64;
        if elapsed >= 30 {
            panic!("The account or virtualization partition still exists in devfs after 30 secs");
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
        dir_entries = fuchsia_fs::directory::readdir(&volumes_dir)
            .await
            .expect("Failed to readdir the fvm DirectoryProxy");
    }
    for entry in dir_entries {
        let name = entry.name;
        let position = expected
            .iter()
            .position(|r| r.is_match(&name))
            .unwrap_or_else(|| panic!("Unexpected entry name: {name}"));
        expected.swap_remove(position);
    }
    assert!(expected.is_empty(), "Missing {expected:?}");
}

#[fuchsia::test]
async fn reset_volumes() {
    let mut builder = new_builder();
    builder
        .with_disk()
        .format_volumes(volumes_spec())
        .with_extra_volume("account")
        .with_extra_volume("virtualization");
    let fixture = builder.build().await;

    fixture.check_fs_type("blob", blob_fs_type()).await;
    fixture.check_fs_type("data", data_fs_type()).await;

    assert_volumes_are_expected(&fixture).await;

    fixture.tear_down().await;
}

#[fuchsia::test]
async fn reset_volumes_no_existing_data_volume() {
    let mut builder = new_builder();
    builder
        .with_disk()
        .format_volumes(VolumesSpec { create_data_partition: false, ..volumes_spec() })
        .with_extra_volume("account")
        .with_extra_volume("virtualization");
    let fixture = builder.build().await;

    fixture.check_fs_type("blob", blob_fs_type()).await;
    fixture.check_fs_type("data", data_fs_type()).await;

    assert_volumes_are_expected(&fixture).await;

    fixture.tear_down().await;
}

#[fuchsia::test]
async fn delivery_blob_support() {
    let mut builder = new_builder();
    builder.fshost().set_config_value("blob_max_bytes", BLOBFS_MAX_BYTES);
    builder.with_disk().format_volumes(volumes_spec());
    let fixture = builder.build().await;

    let data: Vec<u8> = vec![0xff; 65536];
    let hash = fuchsia_merkle::root_from_slice(&data);
    let payload = Type1Blob::generate(&data, CompressionMode::Always);

    let blob_creator: BlobCreatorProxy = fixture
        .realm
        .root
        .connect_to_protocol_at_exposed_dir()
        .expect("connect_to_protocol_at_exposed_dir failed");
    let blob_writer_client_end = blob_creator
        .create(&hash.into(), false)
        .await
        .expect("transport error on create")
        .expect("failed to create blob");

    let writer = blob_writer_client_end.into_proxy();
    let mut blob_writer = BlobWriter::create(writer, payload.len() as u64)
        .await
        .expect("failed to create BlobWriter");
    blob_writer.write(&payload).await.unwrap();

    // We should now be able to open the blob by its hash and read the contents back.
    let blob_reader: BlobReaderProxy = fixture
        .realm
        .root
        .connect_to_protocol_at_exposed_dir()
        .expect("connect_to_protocol_at_exposed_dir failed");
    let vmo = blob_reader.get_vmo(&hash.into()).await.unwrap().unwrap();

    // Read the last 1024 bytes of the file and ensure the bytes match the original `data`.
    let mut buf = vec![0; 1024];
    let offset: u64 = data.len().checked_sub(1024).unwrap() as u64;
    let () = vmo.read(&mut buf, offset).unwrap();
    assert_eq!(&buf, &data[offset as usize..]);

    fixture.tear_down().await;
}

#[fuchsia::test]
async fn data_persists() {
    let mut builder = new_builder();
    builder.with_disk().format_volumes(volumes_spec()).format_data(data_fs_spec());
    let fixture = builder.build().await;

    fixture.check_fs_type("blob", blob_fs_type()).await;
    fixture.check_fs_type("data", data_fs_type()).await;

    let data_root = fixture.dir("data", fio::PERM_READABLE | fio::PERM_WRITABLE);
    let file = fuchsia_fs::directory::open_file(
        &data_root,
        "file",
        fio::Flags::FLAG_MUST_CREATE | fio::PERM_READABLE | fio::PERM_WRITABLE,
    )
    .await
    .unwrap();
    fuchsia_fs::file::write(&file, "file contents!").await.unwrap();

    // Shut down fshost, which should propagate to the data filesystem too.
    let disk = fixture.tear_down().await.unwrap();
    let builder = new_builder().with_disk_from(disk);
    let fixture = builder.build().await;

    fixture.check_fs_type("data", data_fs_type()).await;

    let data_root = fixture.dir("data", fio::PERM_READABLE);
    let file =
        fuchsia_fs::directory::open_file(&data_root, "file", fio::PERM_READABLE).await.unwrap();
    assert_eq!(&fuchsia_fs::file::read(&file).await.unwrap()[..], b"file contents!");

    fixture.tear_down().await;
}

async fn gpt_num_partitions(fixture: &TestFixture) -> usize {
    let partitions = fixture.dir(
        fidl_fuchsia_storage_partitions::PartitionServiceMarker::SERVICE_NAME,
        fuchsia_fs::PERM_READABLE,
    );
    fuchsia_fs::directory::readdir(&partitions).await.expect("Failed to read partitions").len()
}

#[fuchsia::test]
async fn initialized_gpt() {
    let mut builder = new_builder();
    builder.with_disk().format_volumes(volumes_spec()).with_gpt().format_data(data_fs_spec());
    // TODO(https://fxbug.dev/399197713): re-enable extra disk once flake is fixed
    // builder.with_extra_disk().set_uninitialized();
    let fixture = builder.build().await;

    fixture.check_fs_type("blob", blob_fs_type()).await;
    fixture.check_fs_type("data", data_fs_type()).await;
    fixture.check_test_data_file().await;
    fixture.check_test_blob().await;

    assert_eq!(gpt_num_partitions(&fixture).await, 1);

    fixture.tear_down().await;
}

#[fuchsia::test]
async fn uninitialized_gpt() {
    let mut builder = new_builder().with_uninitialized_disk();
    builder.fshost().set_config_value("ramdisk_image", true);
    // TODO(https://fxbug.dev/399197713): re-enable extra disk once flake is fixed
    // builder.with_extra_disk().set_uninitialized();
    let fixture = builder.build().await;

    assert_eq!(gpt_num_partitions(&fixture).await, 0);

    fixture.tear_down().await;
}

#[fuchsia::test]
async fn reset_uninitialized_gpt() {
    let mut builder = new_builder().with_uninitialized_disk();
    builder.fshost().set_config_value("ramdisk_image", true);
    // TODO(https://fxbug.dev/399197713): re-enable extra disk once flake is fixed
    // builder.with_extra_disk().set_uninitialized();
    let fixture = builder.build().await;

    assert_eq!(gpt_num_partitions(&fixture).await, 0);

    let recovery: RecoveryProxy = fixture.realm.root.connect_to_protocol_at_exposed_dir().unwrap();
    recovery
        .init_system_partition_table(&[make_partition_entry(
            "part",
            fpartition::Guid { value: [0xabu8; 16] },
            fpartition::Guid { value: [0xcdu8; 16] },
            4,
            1,
            0,
        )])
        .await
        .expect("FIDL error")
        .expect("init_system_partition_table failed");

    assert_eq!(gpt_num_partitions(&fixture).await, 1);

    fixture.tear_down().await;
}

#[fuchsia::test]
async fn reset_initialized_gpt() {
    let mut builder = new_builder();
    builder.with_disk().format_volumes(volumes_spec()).with_gpt().format_data(data_fs_spec());
    builder.fshost().set_config_value("ramdisk_image", true);
    // TODO(https://fxbug.dev/399197713): re-enable extra disk once flake is fixed
    // builder.with_extra_disk().set_uninitialized();
    let fixture = builder.build().await;

    assert_eq!(gpt_num_partitions(&fixture).await, 1);

    let recovery: RecoveryProxy = fixture.realm.root.connect_to_protocol_at_exposed_dir().unwrap();
    recovery
        .init_system_partition_table(&[
            make_partition_entry(
                "part",
                fpartition::Guid { value: [0xabu8; 16] },
                fpartition::Guid { value: [0xcdu8; 16] },
                4,
                1,
                0,
            ),
            make_partition_entry(
                "part2",
                fpartition::Guid { value: [0x11u8; 16] },
                fpartition::Guid { value: [0x22u8; 16] },
                5,
                1,
                0,
            ),
        ])
        .await
        .expect("FIDL error")
        .expect("init_system_partition_table failed");

    assert_eq!(gpt_num_partitions(&fixture).await, 2);

    fixture.tear_down().await;
}

#[fuchsia::test]
async fn health_check_service() {
    let mut builder = new_builder();
    builder.with_disk().format_volumes(volumes_spec()).format_data(data_fs_spec());
    let fixture = builder.build().await;

    let proxy = fuchsia_component::client::connect_to_protocol_at_dir_root::<
        fidl_fuchsia_update_verify::ComponentOtaHealthCheckMarker,
    >(fixture.exposed_dir())
    .unwrap();
    let status = proxy.get_health_status().await.expect("FIDL error");
    assert_eq!(status, HealthStatus::Healthy);

    fixture.tear_down().await;
}

#[fuchsia::test]
async fn debug_block_directory() {
    let mut builder = new_builder();
    builder.with_disk().format_volumes(volumes_spec()).format_data(data_fs_spec());
    let fixture = builder.build().await;

    // Make sure the filesystems are enumerated before trying to access the block devices. The
    // debug directory is populated as the devices are emitted by the watcher.
    fixture.check_fs_type("blob", blob_fs_type()).await;
    fixture.check_fs_type("data", data_fs_type()).await;

    let block = fuchsia_fs::directory::open_directory(
        fixture.exposed_dir(),
        "debug_block",
        fio::PERM_READABLE,
    )
    .await
    .unwrap();

    // Check that the block directory contains some of the required things for the shell tools
    let source =
        fuchsia_fs::directory::open_file(&block, "000/source", fio::PERM_READABLE).await.unwrap();
    // This is a smoke check - we can't check for a concrete source because it's different (and
    // potentially unstable) depending on the configuration, and it's not that useful to be a
    // change detector.
    assert!(fuchsia_fs::file::read_to_string(&source).await.unwrap().len() > 0);

    let volume = connect_to_named_protocol_at_dir_root::<BlockMarker>(
        &block,
        "000/fuchsia.storage.block.Block",
    )
    .unwrap();
    assert_eq!(
        volume.get_info().await.unwrap().map_err(zx::Status::err_from_raw).unwrap().block_size,
        512,
    );

    fixture.tear_down().await;
}

// TODO(https://fxbug.dev/399197713): Enable this test when extra disks don't flake
#[ignore]
#[fuchsia::test]
async fn expose_unmanaged_block_devices() {
    let mut builder = new_builder();
    builder.with_disk().format_volumes(volumes_spec()).with_gpt().format_data(data_fs_spec());
    builder.with_extra_disk().set_uninitialized().size(8192);
    let fixture = builder.build().await;

    // Make sure the filesystems are enumerated before trying to access the block devices. The block
    // directory is populated as the devices are emitted by the watcher.
    fixture.check_fs_type("blob", blob_fs_type()).await;
    fixture.check_fs_type("data", data_fs_type()).await;

    let block_dir =
        fuchsia_fs::directory::open_directory(fixture.exposed_dir(), "block", fio::PERM_READABLE)
            .await
            .unwrap();
    let mut dirents = fuchsia_fs::directory::readdir(&block_dir).await.expect("readdir failed");
    let device_path = dirents.pop().unwrap().name;
    assert!(dirents.is_empty(), "Multiple devices published");

    let path = format!("{}/{}", &device_path, BlockMarker::PROTOCOL_NAME);
    let volume = fuchsia_component::client::connect_to_named_protocol_at_dir_root::<BlockMarker>(
        &block_dir, &path,
    )
    .unwrap();
    let metadata =
        volume.get_metadata().await.expect("FIDL error").expect("Failed to get metadata");
    assert_eq!(metadata.num_blocks, Some(8192 / TEST_DISK_BLOCK_SIZE as u64));

    fixture.tear_down().await;
}

// Regression test for https://fxbug.dev/408423972.
#[fuchsia::test]
async fn fuse_gpt_once_container_found() {
    let mut builder = new_builder();
    builder.with_disk().format_volumes(volumes_spec());
    let fixture = builder.build().await;

    let partitions = fixture.dir(
        fidl_fuchsia_storage_partitions::PartitionServiceMarker::SERVICE_NAME,
        fuchsia_fs::PERM_READABLE,
    );
    let task = fasync::Task::spawn(async move {
        // This call will block until fshost finds the system container, and at that point it will
        // fuse shut PartitionService, failing all callers.  See logic in mount_fxblob in
        // FshostEnvironment.
        fuchsia_fs::directory::readdir(&partitions)
            .await
            .expect_err("readdir should (eventually) fail")
    });

    // Once the filesystems are enumerated, the above task should be unblocked.
    fixture.check_fs_type("blob", blob_fs_type()).await;
    fixture.check_fs_type("data", data_fs_type()).await;

    task.await;

    fixture.tear_down().await;
}

#[fuchsia::test]
async fn device_config() {
    let builder = new_builder().with_device_config(vec![
        BlockDeviceConfig {
            device: String::from("fts"),
            from: BlockDeviceIdentifiers {
                label: String::from("fts"),
                parent: BlockDeviceParent::Gpt,
            },
        },
        BlockDeviceConfig {
            device: String::from("test-device"),
            from: BlockDeviceIdentifiers {
                label: String::from("boot_a"),
                parent: BlockDeviceParent::Gpt,
            },
        },
        BlockDeviceConfig {
            device: String::from("boot_b"),
            from: BlockDeviceIdentifiers {
                label: String::from("boot_b"),
                parent: BlockDeviceParent::Gpt,
            },
        },
    ]);
    let mut fixture = builder.build().await;

    // By attempting to open and use the block directory before we add the disk, we confirm that
    // queuing requests for configured devices works as expected. If the queuing doesn't work, this
    // will fail with PEER_CLOSED instead.
    let fts_dir = fixture.dir("block/fts", fio::PERM_READABLE);
    let volume =
        fuchsia_component::client::connect_to_protocol_at_dir_root::<BlockMarker>(&fts_dir)
            .unwrap();
    let task =
        fasync::Task::spawn(
            async move { volume.get_metadata().await.unwrap().unwrap().num_blocks },
        );

    let mut disk = DiskBuilder::new();
    disk.with_gpt()
        .format_volumes(volumes_spec())
        .with_extra_gpt_partition("fts", 1)
        .with_extra_gpt_partition("boot_a", 1)
        .with_extra_gpt_partition("boot_b", 1);
    fixture.add_main_disk(Disk::Builder(disk)).await;
    // Add a second disk, to make sure that fshost only enumerates the right one.  Fshost
    // disambiguates by the presence of the system partition.
    let mut secondary_disk = DiskBuilder::new();
    secondary_disk.with_gpt().with_extra_gpt_partition("fts", 5);
    fixture.add_disk(Disk::Builder(secondary_disk)).await;

    fixture.check_fs_type("blob", blob_fs_type()).await;
    fixture.check_fs_type("data", data_fs_type()).await;

    assert_eq!(task.await, Some(1));

    let fts_dir = fixture.dir("block/fts", fio::PERM_READABLE);
    let volume =
        fuchsia_component::client::connect_to_protocol_at_dir_root::<BlockMarker>(&fts_dir)
            .unwrap();
    let metadata = volume.get_metadata().await.unwrap().unwrap();
    assert_eq!(metadata.num_blocks, Some(1));

    let boot_a_dir = fixture.dir("block/test-device", fio::PERM_READABLE);
    let volume =
        fuchsia_component::client::connect_to_protocol_at_dir_root::<BlockMarker>(&boot_a_dir)
            .unwrap();
    let metadata = volume.get_metadata().await.unwrap().unwrap();
    assert_eq!(metadata.num_blocks, Some(1));

    let boot_b_dir = fixture.dir("block/boot_b", fio::PERM_READABLE);
    let volume =
        fuchsia_component::client::connect_to_protocol_at_dir_root::<BlockMarker>(&boot_b_dir)
            .unwrap();
    let metadata = volume.get_metadata().await.unwrap().unwrap();
    assert_eq!(metadata.num_blocks, Some(1));

    fixture.tear_down().await;
}

#[fuchsia::test]
async fn gpt_all_binds_multiple_disks() {
    let mut builder = new_builder().with_device_config(vec![BlockDeviceConfig {
        device: String::from("test-part"),
        from: BlockDeviceIdentifiers {
            label: String::from("test_part"),
            parent: BlockDeviceParent::Gpt,
        },
    }]);
    builder.fshost().set_config_value("gpt_all", true);
    builder.with_disk().format_volumes(volumes_spec()).with_gpt().format_data(data_fs_spec());
    builder
        .with_extra_disk()
        .with_gpt()
        .with_unformatted_volume_manager()
        .with_extra_gpt_partition("test_part", 1);
    let fixture = builder.build().await;

    fixture.check_fs_type("blob", blob_fs_type()).await;
    fixture.check_fs_type("data", data_fs_type()).await;

    // Check that the extra partition is available.
    let test_part_dir = fixture.dir("block/test-part", fio::PERM_READABLE);
    let volume =
        fuchsia_component::client::connect_to_protocol_at_dir_root::<BlockMarker>(&test_part_dir)
            .unwrap();
    let metadata = volume.get_metadata().await.unwrap().unwrap();
    assert_eq!(metadata.num_blocks, Some(1));

    // One of the disks has one gpt partition and the other has two. Because of a quirk of the
    // integration tests, the one that actually doesn't have any formatted information (the one
    // with two partitions) gets registered as the system gpt and exported via the partition
    // service. We double check that happened how we expect.
    assert_eq!(gpt_num_partitions(&fixture).await, 2);

    fixture.tear_down().await;
}

#[fuchsia::test]
async fn expose_system_gpt() {
    let mut builder = new_builder();
    builder.with_disk().format_volumes(volumes_spec()).format_data(data_fs_spec());
    let fixture = builder.build().await;

    // Make sure the filesystems are enumerated before trying to access the block devices. The
    // debug directory is populated as the devices are emitted by the watcher.
    fixture.check_fs_type("blob", blob_fs_type()).await;
    fixture.check_fs_type("data", data_fs_type()).await;

    let block = fuchsia_fs::directory::open_directory(
        fixture.exposed_dir(),
        "debug_block",
        fio::PERM_READABLE,
    )
    .await
    .unwrap();

    // Check that the block directory contains some of the required things for the shell tools
    let source =
        fuchsia_fs::directory::open_file(&block, "000/source", fio::PERM_READABLE).await.unwrap();
    // This is a smoke check - we can't check for a concrete source because it's different (and
    // potentially unstable) depending on the configuration, and it's not that useful to be a
    // change detector.
    assert!(fuchsia_fs::file::read_to_string(&source).await.unwrap().len() > 0);

    let volume = connect_to_named_protocol_at_dir_root::<BlockMarker>(
        &block,
        "000/fuchsia.storage.block.Block",
    )
    .unwrap();
    assert_eq!(
        volume.get_info().await.unwrap().map_err(zx::Status::err_from_raw).unwrap().block_size,
        512,
    );

    fixture.tear_down().await;
}

/// Populates the data volume with a set of well-known test files.
pub async fn create_test_data_file(data_dir: &fio::DirectoryProxy) {
    fuchsia_fs::directory::open_file(&data_dir, ".testdata", fio::Flags::FLAG_MAYBE_CREATE)
        .await
        .unwrap();
    fuchsia_fs::directory::create_directory(
        &data_dir,
        "ssh",
        fio::PERM_READABLE | fio::PERM_WRITABLE,
    )
    .await
    .unwrap();
    fuchsia_fs::directory::create_directory(
        &data_dir,
        "ssh/config",
        fio::PERM_READABLE | fio::PERM_WRITABLE,
    )
    .await
    .unwrap();
    fuchsia_fs::directory::create_directory(
        &data_dir,
        "problems",
        fio::PERM_READABLE | fio::PERM_WRITABLE,
    )
    .await
    .unwrap();
    let ssh_key = fuchsia_fs::directory::open_file(
        &data_dir,
        "ssh/authorized_keys",
        fio::Flags::FLAG_MAYBE_CREATE | fio::PERM_WRITABLE,
    )
    .await
    .unwrap();
    fuchsia_fs::file::write(&ssh_key, "public key!").await.unwrap();
}

#[cfg(feature = "fxblob")]
mod fxblob {
    use super::*;

    use diagnostics_assertions::assert_data_tree;
    use diagnostics_reader::ArchiveReader;
    use fidl::endpoints::Proxy;
    use fidl_fuchsia_fshost::StarnixVolumeProviderProxy;
    use fshost_test_fixture::{STARNIX_VOLUME_NAME, VFS_TYPE_FXFS};

    fn keymint_data_fs_spec() -> fshost_test_fixture::disk_builder::DataSpec {
        fshost_test_fixture::disk_builder::DataSpec {
            format: Some("fxfs"),
            zxcrypt: false,
            crypt_policy: crypt_policy::Policy::Keymint,
        }
    }

    async fn shutdown_starnix_volume(exposed_dir: fio::DirectoryProxy) {
        let (proxy, server_end) = fidl::endpoints::create_proxy::<fidl_fuchsia_fs::AdminMarker>();
        exposed_dir
            .open(
                &format!("svc/{}", fidl_fuchsia_fs::AdminMarker::PROTOCOL_NAME),
                fio::Flags::PROTOCOL_SERVICE,
                &fio::Options::default(),
                server_end.into(),
            )
            .expect("fidl transport error");

        proxy.shutdown().await.expect("fidl transport error");
    }

    #[fuchsia::test]
    async fn create_unmount_and_remount_starnix_volume() {
        let mut builder = new_builder();
        builder
            .fshost()
            .create_starnix_volume_crypt()
            .set_config_value("starnix_volume_name", STARNIX_VOLUME_NAME);
        builder.with_disk().format_volumes(volumes_spec());
        let fixture = builder.build().await;

        fixture.check_fs_type("blob", blob_fs_type()).await;
        fixture.check_fs_type("data", data_fs_type()).await;

        let volume_provider: StarnixVolumeProviderProxy =
            fixture.realm.root.connect_to_protocol_at_exposed_dir().expect(
                "connect_to_protocol_at_exposed_dir failed for the StarnixVolumeProvider protocol",
            );
        // Check should succeed when there's no volume
        let (crypt, _crypt_management) = fixture.setup_starnix_crypt().await;
        volume_provider
            .check(crypt.into_client_end().unwrap())
            .await
            .expect("fidl transport error")
            .expect("check with absent volume failed");

        let crypt = fixture.connect_to_crypt();
        let (exposed_dir_proxy, exposed_dir_server) =
            fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        volume_provider
            .mount(
                crypt.into_client_end().unwrap(),
                fidl_fuchsia_fshost::MountMode::AlwaysCreate,
                exposed_dir_server,
            )
            .await
            .expect("fidl transport error")
            .expect("mount failed");

        async fn check_inspect(child_name: &str) {
            let inspector = ArchiveReader::inspect()
                .add_selector(format!("realm_builder\\:{}/test-fshost/fxfs:root", child_name,))
                .snapshot()
                .await
                .expect("inspect snapshot failed")
                .into_iter()
                .next()
                .and_then(|result| result.payload)
                .expect("expected one inspect hierarchy");

            assert_data_tree!(inspector, root: contains {
                stores: contains {
                    STARNIX_VOLUME_NAME.to_string() => contains {
                        low_32_bit_object_ids: true,
                    }
                }
            });
        }

        check_inspect(fixture.realm.root.child_name()).await;

        let starnix_volume_root_dir = fuchsia_fs::directory::open_directory(
            &exposed_dir_proxy,
            "root",
            fio::PERM_READABLE | fio::PERM_WRITABLE,
        )
        .await
        .expect("Failed to open the root dir of the starnix volume");

        let starnix_volume_file = fuchsia_fs::directory::open_file(
            &starnix_volume_root_dir,
            "file",
            fio::Flags::FLAG_MAYBE_CREATE | fio::PERM_READABLE | fio::PERM_WRITABLE,
        )
        .await
        .expect("Failed to create file in starnix volume");
        fuchsia_fs::file::write(&starnix_volume_file, "file contents!").await.unwrap();

        shutdown_starnix_volume(exposed_dir_proxy).await;

        let disk = fixture.tear_down().await.unwrap();
        let mut builder = new_builder().with_disk_from(disk);
        builder
            .fshost()
            .create_starnix_volume_crypt()
            .set_config_value("starnix_volume_name", STARNIX_VOLUME_NAME);
        let fixture = builder.build().await;

        fixture.check_fs_type("blob", blob_fs_type()).await;
        fixture.check_fs_type("data", data_fs_type()).await;

        let volume_provider: StarnixVolumeProviderProxy =
            fixture.realm.root.connect_to_protocol_at_exposed_dir().expect(
                "connect_to_protocol_at_exposed_dir failed for the StarnixVolumeProvider protocol",
            );
        let (crypt, _crypt_management) = fixture.setup_starnix_crypt().await;
        volume_provider
            .check(crypt.into_client_end().unwrap())
            .await
            .expect("fidl transport error")
            .expect("check failed");

        let crypt = fixture.connect_to_crypt();
        let (exposed_dir_proxy, exposed_dir_server) =
            fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        volume_provider
            .mount(
                crypt.into_client_end().unwrap(),
                fidl_fuchsia_fshost::MountMode::MaybeCreate,
                exposed_dir_server,
            )
            .await
            .expect("fidl transport error")
            .expect("mount failed");

        let starnix_volume_root_dir =
            fuchsia_fs::directory::open_directory(&exposed_dir_proxy, "root", fio::PERM_READABLE)
                .await
                .expect("Failed to open the root dir of the starnix volume");

        let starnix_volume_file =
            fuchsia_fs::directory::open_file(&starnix_volume_root_dir, "file", fio::PERM_READABLE)
                .await
                .expect("Failed to create file in starnix volume");
        assert_eq!(
            &fuchsia_fs::file::read(&starnix_volume_file).await.unwrap()[..],
            b"file contents!"
        );

        check_inspect(fixture.realm.root.child_name()).await;

        fixture.tear_down().await;
    }

    #[fuchsia::test]
    async fn create_mount_and_remount_starnix_volume() {
        let mut builder = new_builder();
        builder
            .fshost()
            .create_starnix_volume_crypt()
            .set_config_value("starnix_volume_name", STARNIX_VOLUME_NAME);
        builder.with_disk().format_volumes(volumes_spec());
        let fixture = builder.build().await;

        fixture.check_fs_type("blob", blob_fs_type()).await;
        fixture.check_fs_type("data", data_fs_type()).await;

        let volume_provider: StarnixVolumeProviderProxy =
            fixture.realm.root.connect_to_protocol_at_exposed_dir().expect(
                "connect_to_protocol_at_exposed_dir failed for the StarnixVolumeProvider protocol",
            );
        let (crypt, _crypt_management) = fixture.setup_starnix_crypt().await;
        let (exposed_dir_proxy, exposed_dir_server) =
            fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        volume_provider
            .mount(
                crypt.into_client_end().unwrap(),
                fidl_fuchsia_fshost::MountMode::AlwaysCreate,
                exposed_dir_server,
            )
            .await
            .expect("fidl transport error")
            .expect("mount failed");

        let starnix_volume_root_dir = fuchsia_fs::directory::open_directory(
            &exposed_dir_proxy,
            "root",
            fio::PERM_READABLE | fio::PERM_WRITABLE,
        )
        .await
        .expect("Failed to open the root dir of the starnix volume");

        let starnix_volume_file = fuchsia_fs::directory::open_file(
            &starnix_volume_root_dir,
            "file",
            fio::Flags::FLAG_MAYBE_CREATE | fio::PERM_READABLE | fio::PERM_WRITABLE,
        )
        .await
        .expect("Failed to create file in starnix volume");
        fuchsia_fs::file::write(&starnix_volume_file, "file contents!").await.unwrap();

        shutdown_starnix_volume(exposed_dir_proxy).await;

        let crypt = fixture.connect_to_crypt();
        let (exposed_dir_proxy, exposed_dir_server) =
            fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        volume_provider
            .mount(
                crypt.into_client_end().unwrap(),
                fidl_fuchsia_fshost::MountMode::MaybeCreate,
                exposed_dir_server,
            )
            .await
            .expect("fidl transport error")
            .expect("mount failed");

        let starnix_volume_root_dir =
            fuchsia_fs::directory::open_directory(&exposed_dir_proxy, "root", fio::PERM_READABLE)
                .await
                .expect("Failed to open the root dir of the starnix volume");

        let starnix_volume_file =
            fuchsia_fs::directory::open_file(&starnix_volume_root_dir, "file", fio::PERM_READABLE)
                .await
                .expect("Failed to create file in starnix volume");
        assert_eq!(
            &fuchsia_fs::file::read(&starnix_volume_file).await.unwrap()[..],
            b"file contents!"
        );

        fixture.tear_down().await;
    }

    #[fuchsia::test]
    async fn create_starnix_volume_wipes_previous_volume() {
        let mut builder = new_builder();
        builder
            .fshost()
            .create_starnix_volume_crypt()
            .set_config_value("starnix_volume_name", STARNIX_VOLUME_NAME);
        builder.with_disk().format_volumes(volumes_spec());
        let fixture = builder.build().await;

        fixture.check_fs_type("blob", blob_fs_type()).await;
        fixture.check_fs_type("data", data_fs_type()).await;

        let volume_provider: StarnixVolumeProviderProxy =
            fixture.realm.root.connect_to_protocol_at_exposed_dir().expect(
                "connect_to_protocol_at_exposed_dir failed for the StarnixVolumeProvider protocol",
            );
        let (crypt, _crypt_management) = fixture.setup_starnix_crypt().await;
        let (exposed_dir_proxy, exposed_dir_server) =
            fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        volume_provider
            .mount(
                crypt.into_client_end().unwrap(),
                fidl_fuchsia_fshost::MountMode::AlwaysCreate,
                exposed_dir_server,
            )
            .await
            .expect("fidl transport error")
            .expect("mount failed");

        let starnix_volume_root_dir = fuchsia_fs::directory::open_directory(
            &exposed_dir_proxy,
            "root",
            fio::PERM_READABLE | fio::PERM_WRITABLE,
        )
        .await
        .expect("Failed to open the root dir of the starnix volume");

        let starnix_volume_file = fuchsia_fs::directory::open_file(
            &starnix_volume_root_dir,
            "file",
            fio::Flags::FLAG_MAYBE_CREATE | fio::PERM_READABLE | fio::PERM_WRITABLE,
        )
        .await
        .expect("Failed to create file in starnix volume");
        fuchsia_fs::file::write(&starnix_volume_file, "file contents!").await.unwrap();

        shutdown_starnix_volume(exposed_dir_proxy).await;

        let disk = fixture.tear_down().await.unwrap();
        let mut builder = new_builder().with_disk_from(disk);
        builder
            .fshost()
            .create_starnix_volume_crypt()
            .set_config_value("starnix_volume_name", STARNIX_VOLUME_NAME);
        let fixture = builder.build().await;

        fixture.check_fs_type("blob", blob_fs_type()).await;
        fixture.check_fs_type("data", data_fs_type()).await;

        let volume_provider: StarnixVolumeProviderProxy =
            fixture.realm.root.connect_to_protocol_at_exposed_dir().expect(
                "connect_to_protocol_at_exposed_dir failed for the StarnixVolumeProvider protocol",
            );
        let (crypt, _crypt_management) = fixture.setup_starnix_crypt().await;
        let (exposed_dir_proxy, exposed_dir_server) =
            fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        volume_provider
            .mount(
                crypt.into_client_end().unwrap(),
                fidl_fuchsia_fshost::MountMode::AlwaysCreate,
                exposed_dir_server,
            )
            .await
            .expect("fidl transport error")
            .expect("mount failed");

        let starnix_volume_root_dir =
            fuchsia_fs::directory::open_directory(&exposed_dir_proxy, "root", fio::PERM_READABLE)
                .await
                .expect("Failed to open the root dir of the starnix volume");

        assert_matches!(
            fuchsia_fs::directory::open_file(&starnix_volume_root_dir, "file", fio::PERM_READABLE)
                .await
                .expect_err(
                    "StarnixVolumeProvider.Create should wipe the Starnix volume if it exists"
                ),
            fuchsia_fs::node::OpenError::OpenError(zx::Status::NOT_FOUND)
        );

        fixture.tear_down().await;
    }

    #[fuchsia::test]
    async fn shred_data_deletes_starnix_volume() {
        let mut builder = new_builder();
        builder.with_disk().format_volumes(volumes_spec());
        builder
            .fshost()
            .create_starnix_volume_crypt()
            .set_config_value("starnix_volume_name", STARNIX_VOLUME_NAME);
        let fixture = builder.build().await;

        fixture.check_fs_type("blob", blob_fs_type()).await;
        fixture.check_fs_type("data", data_fs_type()).await;

        // Need to connect to the StarnixVolumeProvider protocol that fshost exposes and Mount the
        // starnix volume.
        let volume_provider: StarnixVolumeProviderProxy =
            fixture.realm.root.connect_to_protocol_at_exposed_dir().expect(
                "connect_to_protocol_at_exposed_dir failed for the StarnixVolumeProvider protocol",
            );
        let (crypt, _crypt_management) = fixture.setup_starnix_crypt().await;
        let (_exposed_dir_proxy, exposed_dir_server) =
            fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        volume_provider
            .mount(
                crypt.into_client_end().unwrap(),
                fidl_fuchsia_fshost::MountMode::AlwaysCreate,
                exposed_dir_server,
            )
            .await
            .expect("fidl transport error")
            .expect("mount failed");

        let admin: AdminProxy = fixture
            .realm
            .root
            .connect_to_protocol_at_exposed_dir()
            .expect("connect_to_protcol_at_exposed_dir failed");

        admin
            .shred_data_volume()
            .await
            .expect("shred_data_volume FIDL failed")
            .expect("shred_data_volume failed");
        let disk = fixture.tear_down().await.unwrap();

        let mut builder = new_builder().with_disk_from(disk);
        builder
            .fshost()
            .create_starnix_volume_crypt()
            .set_config_value("starnix_volume_name", STARNIX_VOLUME_NAME);
        let fixture = builder.build().await;

        fixture.check_fs_type("blob", blob_fs_type()).await;
        fixture.check_fs_type("data", data_fs_type()).await;

        let volumes_dir = fixture.dir("volumes", fio::PERM_READABLE);
        let dir_entries = fuchsia_fs::directory::readdir(&volumes_dir)
            .await
            .expect("Failed to readdir the volumes");
        assert!(dir_entries.iter().find(|x| x.name.contains(STARNIX_VOLUME_NAME)).is_none());

        fixture.tear_down().await;
    }

    #[fuchsia::test]
    async fn vend_a_fresh_starnix_test_volume_on_each_mount() {
        let mut builder = new_builder();
        builder.with_disk().format_volumes(volumes_spec());
        builder.fshost().create_starnix_volume_crypt();
        let fixture = builder.build().await;

        fixture.check_fs_type("blob", blob_fs_type()).await;
        fixture.check_fs_type("data", data_fs_type()).await;

        // Need to connect to the StarnixVolumeProvider protocol that fshost exposes and Mount the
        // starnix volume.
        let volume_provider: StarnixVolumeProviderProxy =
            fixture.realm.root.connect_to_protocol_at_exposed_dir().expect(
                "connect_to_protocol_at_exposed_dir failed for the StarnixVolumeProvider protocol",
            );
        let (crypt, _crypt_management) = fixture.setup_starnix_crypt().await;
        let (exposed_dir_proxy, exposed_dir_server) =
            fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        volume_provider
            .mount(
                crypt.into_client_end().unwrap(),
                fidl_fuchsia_fshost::MountMode::MaybeCreate,
                exposed_dir_server,
            )
            .await
            .expect("fidl transport error")
            .expect("mount failed");

        let starnix_volume_root_dir = fuchsia_fs::directory::open_directory(
            &exposed_dir_proxy,
            "root",
            fio::PERM_READABLE | fio::PERM_WRITABLE,
        )
        .await
        .expect("Failed to open the root dir of the starnix volume");

        let starnix_volume_file = fuchsia_fs::directory::open_file(
            &starnix_volume_root_dir,
            "file",
            fio::Flags::FLAG_MAYBE_CREATE | fio::PERM_READABLE | fio::PERM_WRITABLE,
        )
        .await
        .expect("Failed to create file in starnix volume");
        fuchsia_fs::file::write(&starnix_volume_file, "file contents!").await.unwrap();

        shutdown_starnix_volume(exposed_dir_proxy).await;

        let disk = fixture.tear_down().await.unwrap();
        let mut builder = new_builder().with_disk_from(disk);
        builder.fshost().create_starnix_volume_crypt();
        let fixture = builder.build().await;

        fixture.check_fs_type("blob", blob_fs_type()).await;
        fixture.check_fs_type("data", data_fs_type()).await;

        // Need to connect to the StarnixVolumeProvider protocol that fshost exposes and Mount the
        // starnix volume.
        let volume_provider: StarnixVolumeProviderProxy =
            fixture.realm.root.connect_to_protocol_at_exposed_dir().expect(
                "connect_to_protocol_at_exposed_dir failed for the StarnixVolumeProvider protocol",
            );
        let (crypt, _crypt_management) = fixture.setup_starnix_crypt().await;
        let (exposed_dir_proxy, exposed_dir_server) =
            fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        volume_provider
            .mount(
                crypt.into_client_end().unwrap(),
                fidl_fuchsia_fshost::MountMode::MaybeCreate,
                exposed_dir_server,
            )
            .await
            .expect("fidl transport error")
            .expect("mount failed");

        let starnix_volume_root_dir =
            fuchsia_fs::directory::open_directory(&exposed_dir_proxy, "root", fio::PERM_READABLE)
                .await
                .expect("Failed to open the root dir of the starnix volume");

        // fshost should vend a fresh Starnix test volume on every mount so this file should no
        // longer exist.
        fuchsia_fs::directory::open_file(&starnix_volume_root_dir, "file", fio::PERM_READABLE)
            .await
            .expect_err("fshost should vend a fresh Starnix test volume on every mount");

        fixture.tear_down().await;
    }

    #[fuchsia::test]
    async fn health_check_blobs() {
        let mut builder = new_builder();
        builder.with_disk().format_volumes(volumes_spec()).format_data(data_fs_spec());
        let fixture = builder.build().await;

        let blobfs_health_check: fidl_fuchsia_update_verify::ComponentOtaHealthCheckProxy = fixture
            .realm
            .root
            .connect_to_protocol_at_exposed_dir()
            .expect("connect_to_protcol_at_exposed_dir failed");
        let status = blobfs_health_check.get_health_status().await.expect("FIDL failure");
        assert_eq!(status, HealthStatus::Healthy);

        fixture.tear_down().await;
    }

    // This test exercises merging super and userdata into a single logical "fxfs" partition. The
    // GPT will be formatted "super" (which contains Fxfs) and "userdata" (which is unformatted),
    // and it is expected that fshost sees a merged "fxfs" partition. The test only works on fxfs,
    // because fxfs supports mounting on a larger partition than it was formatted with.
    #[fuchsia::test]
    async fn merge_super_and_userdata() {
        use fidl_fuchsia_storage_partitions::OverlayPartitionMarker;
        use fs_management::FVM_TYPE_GUID;
        use fshost_test_fixture::disk_builder::{DEFAULT_TEST_TYPE_GUID, FVM_PART_INSTANCE_GUID};

        // fxfs will ignore blocks that are not aligned to a page, so make sure that we give at
        // least this many blocks to super, so we can exercise Fxfs claiming the additional space.
        const USERDATA_NUM_BLOCKS: u64 = 4096 / TEST_DISK_BLOCK_SIZE as u64;

        let mut builder = new_builder();
        builder.fshost().set_config_value("merge_super_and_userdata", true);
        let mut fixture = builder.build().await;

        let mut disk = DiskBuilder::new();
        disk.with_gpt()
            .format_volumes(volumes_spec())
            .with_system_partition_label("super")
            // NOTE: The "userdata" partition will be physically contiguous with the "super"
            // partition.
            .with_extra_gpt_partition("userdata", USERDATA_NUM_BLOCKS)
            .with_extra_gpt_partition("other", 1);
        fixture.add_main_disk(Disk::Builder(disk)).await;

        fixture.check_fs_type("blob", blob_fs_type()).await;
        fixture.check_fs_type("data", data_fs_type()).await;
        fixture.check_test_blob().await;

        let partitions = fixture.dir(
            fidl_fuchsia_storage_partitions::PartitionServiceMarker::SERVICE_NAME,
            fuchsia_fs::PERM_READABLE,
        );
        let instances = fuchsia_fs::directory::readdir(&partitions)
            .await
            .expect("readdir failed")
            .into_iter()
            .map(|entry| entry.name)
            .collect::<Vec<_>>();
        assert_eq!(instances.len(), 2);
        let mut found_merged_partition = false;
        for instance in instances {
            let dir =
                fuchsia_fs::directory::open_directory(&partitions, &instance, fio::PERM_READABLE)
                    .await
                    .expect("open dir failed");
            let volume =
                connect_to_named_protocol_at_dir_root::<BlockMarker>(&dir, "volume").unwrap();
            let metadata =
                volume.get_metadata().await.expect("FIDL error").expect("Failed to get metadata");
            assert_ne!(metadata.name.as_ref().unwrap(), "super");
            assert_ne!(metadata.name.as_ref().unwrap(), "userdata");
            if metadata.name.as_ref().unwrap() == "super_and_userdata" {
                found_merged_partition = true;
                assert_eq!(metadata.start_block_offset, None);
                assert_eq!(metadata.flags, None);
                assert_eq!(metadata.num_blocks, Some(221789));
                assert_eq!(metadata.type_guid, Some(fpartition::Guid { value: FVM_TYPE_GUID }));
                assert_eq!(
                    metadata.instance_guid,
                    Some(fpartition::Guid { value: FVM_PART_INSTANCE_GUID })
                );
                let overlay = connect_to_named_protocol_at_dir_root::<OverlayPartitionMarker>(
                    &dir, "overlay",
                )
                .unwrap();
                let infos = overlay
                    .get_partitions()
                    .await
                    .expect("FIDL error")
                    .expect("Failed to get parts");
                assert_eq!(
                    infos,
                    vec![
                        make_partition_info(
                            "super",
                            fpartition::Guid { value: FVM_TYPE_GUID },
                            fpartition::Guid { value: FVM_PART_INSTANCE_GUID },
                            64,
                            221781,
                            0,
                        ),
                        make_partition_info(
                            "userdata",
                            fpartition::Guid { value: DEFAULT_TEST_TYPE_GUID },
                            fpartition::Guid { value: FVM_PART_INSTANCE_GUID },
                            221845,
                            USERDATA_NUM_BLOCKS,
                            0,
                        ),
                    ]
                )
            }
        }
        assert!(found_merged_partition, "No super+userdata found");

        fixture.tear_down().await;
    }

    #[fuchsia::test]
    async fn blobfs_and_data_mounted_with_keymint() {
        let mut builder = new_builder().with_crypt_policy(crypt_policy::Policy::Keymint);
        let data_spec = DataSpec {
            zxcrypt: false,
            crypt_policy: crypt_policy::Policy::Keymint,
            ..data_fs_spec()
        };
        builder.with_disk().format_volumes(volumes_spec()).format_data(data_spec);
        let fixture = builder.build().await;

        fixture.check_fs_type("blob", blob_fs_type()).await;
        fixture.check_fs_type("data", data_fs_type()).await;
        fixture.check_test_data_file().await;
        fixture.check_test_blob().await;
        fixture.tear_down().await;
    }

    #[fuchsia::test]
    async fn data_formatted_keymint() {
        let mut builder = new_builder().with_crypt_policy(crypt_policy::Policy::Keymint);
        builder.with_disk().format_volumes(volumes_spec());
        let fixture = builder.build().await;

        fixture.check_fs_type("blob", blob_fs_type()).await;
        fixture.check_fs_type("data", data_fs_type()).await;
        fixture.check_test_blob().await;

        fixture.tear_down().await;
    }

    /// Tests the early-boot recovery mechanism when an `old_blob` is found in the KeyMint
    /// persistence file. This indicates a previous upgrade was interrupted by a power failure
    /// before the old key could be deleted from the hardware (or a failure of the 'delete_key'
    /// method).
    ///
    /// We must ensure fshost deletes the old key from the TEE and cleans up the persistent state
    /// during early boot to prevent the hardware slot from leaking. Hardware slots may be limited,
    /// so failing to clean them up across multiple interrupted upgrades could eventually brick a
    /// device.
    #[fuchsia::test]
    async fn data_formatted_keymint_upgrade_recovery() {
        let mut builder = new_builder().with_crypt_policy(crypt_policy::Policy::Keymint);

        // Provide a valid "upgraded" mock blob format that FakeKeymint tracks, so it will
        // actually delete
        // it.
        let old_blob = {
            let km = fake_keymint::FakeKeymint::default();
            km.bump_epoch();
            km.generate_static_sealing_key(b"fuchsia")
        };
        let shared_keymint = builder.keymint();

        // We must forcibly inject the simulated old_blob into the keymint hardware state so the
        // test has something to actually scrub.
        shared_keymint.insert_sealing_key(b"fuchsia", vec![old_blob.clone()]);

        builder
            .with_disk()
            .with_keymint_instance(shared_keymint.clone())
            .with_keymint_old_blob(old_blob.clone())
            .format_volumes(volumes_spec());
        let fixture = builder.build().await;

        // Confirm that fshost mounted the data volume smoothly despite the interrupted upgrade
        // state.
        fixture.check_fs_type("blob", blob_fs_type()).await;
        fixture.check_fs_type("data", data_fs_type()).await;
        fixture.check_test_blob().await;

        // Verify the successful hardware cleanup path!
        assert!(!shared_keymint.has_key_blob(&old_blob));

        fixture.tear_down().await;
    }

    /// Tests the fallback behavior of the early-boot recovery mechanism when the hardware deletion
    /// of the `old_blob` fails. This simulates a scenario where a power failure interrupted an
    /// upgrade, and upon reboot, the KeyMint refuses to delete the old key (e.g., due to an
    /// internal fault).
    ///
    /// In this case, fshost must swallow the deletion error, remove the `old_blob` from the
    /// persistence file, and proceed with mounting the filesystem. While this leaks a single
    /// hardware key slot, it prevents an infinite bootloop that could permanently brick the device.
    /// We prioritize device availability while tolerating an unrecoverable hardware fault.
    #[fuchsia::test]
    async fn data_formatted_keymint_upgrade_deletion_failure_recovery() {
        use fidl_fuchsia_security_keymint as fkeymint;

        let mut builder = new_builder().with_crypt_policy(crypt_policy::Policy::Keymint);

        builder
            .keymint()
            .set_delete_hook(|_| async { Some(fkeymint::DeleteError::FailedDelete) }.boxed());

        let old_blob = vec![0x11, 0x22, 0x33, 0x44];
        let shared_keymint = builder.keymint();
        builder
            .with_disk()
            .with_keymint_instance(shared_keymint.clone())
            .with_keymint_old_blob(old_blob)
            .format_volumes(volumes_spec())
            .format_data(keymint_data_fs_spec());
        let fixture = builder.build().await;

        fixture.check_fs_type("data", data_fs_type()).await;

        fixture.tear_down().await;
    }

    /// Tests the primary inline key upgrade path. During a normal filesystem mount, KeyMint may
    /// return `KeyRequiresUpgrade` during `Unseal`. fshost must successfully re-seal the key with
    /// the upgraded material, persist the new blob, delete the old blob from hardware, and continue
    /// mounting.
    ///
    /// This test validates the happy path of the upgrade process, where both the persistent file
    /// update and the hardware key deletion complete successfully without interruption or errors.
    #[fuchsia::test]
    async fn data_formatted_keymint_upgrade_on_unseal() {
        let mut builder = new_builder().with_crypt_policy(crypt_policy::Policy::Keymint);
        let shared_keymint = builder.keymint();
        let initial_blob = shared_keymint.generate_static_sealing_key(b"fuchsia");
        let disk = {
            builder.with_disk().format_volumes(volumes_spec()).format_data(keymint_data_fs_spec());
            let fixture = builder.build().await;
            fixture.tear_down().await.unwrap()
        };

        shared_keymint.bump_epoch();
        let fixture = new_builder()
            .with_crypt_policy(crypt_policy::Policy::Keymint)
            .with_disk_from(disk)
            .with_keymint_instance(shared_keymint.clone())
            .build()
            .await;

        fixture.check_fs_type("data", data_fs_type()).await;

        let upgraded_blob = shared_keymint.generate_static_sealing_key(b"fuchsia");
        assert!(shared_keymint.has_key_blob(&upgraded_blob));
        assert!(!shared_keymint.has_key_blob(&initial_blob));

        fixture.tear_down().await;
    }

    /// Tests the resilience of the inline upgrade path when the hardware deletion of the old key
    /// fails synchronously during the upgrade operation.
    ///
    /// If `DeleteSealingKey` fails immediately after we've successfully persisted the newly
    /// upgraded blob, fshost must log the error but still complete the mount successfully. This
    /// leaks a key slot in the TEE but ensures the device remains usable. This validates the
    /// primary path's resilience, distinct from the early-boot fallback cleanup mechanism tested
    /// elsewhere.
    ///
    /// Note that the failed deletion gets one more attempt at next load time, so a single failed
    /// deletion does not necessarily leak anything. A leak will only occur if the deletion fails
    /// multiple times in a row.
    #[fuchsia::test]
    async fn data_formatted_keymint_upgrade_on_unseal_with_deletion_failure() {
        let mut builder = new_builder().with_crypt_policy(crypt_policy::Policy::Keymint);
        let shared_keymint = builder.keymint();
        let initial_blob = shared_keymint.generate_static_sealing_key(b"fuchsia");
        let disk = {
            builder.with_disk().format_volumes(volumes_spec()).format_data(keymint_data_fs_spec());
            let fixture = builder.build().await;
            fixture.tear_down().await.unwrap()
        };

        shared_keymint.bump_epoch();
        shared_keymint.set_delete_hook(|_| {
            async { Some(fidl_fuchsia_security_keymint::DeleteError::FailedDelete) }.boxed()
        });

        let fixture = new_builder()
            .with_crypt_policy(crypt_policy::Policy::Keymint)
            .with_disk_from(disk)
            .with_keymint_instance(shared_keymint.clone())
            .build()
            .await;

        fixture.check_fs_type("data", data_fs_type()).await;

        // We failed the deletion, so the old key MUST leak into the TEE slot.
        let upgraded_blob = shared_keymint.generate_static_sealing_key(b"fuchsia");
        assert!(shared_keymint.has_key_blob(&upgraded_blob));
        assert!(shared_keymint.has_key_blob(&initial_blob));

        fixture.tear_down().await;
    }

    /// Tests the scenario where multiple keys (e.g., both `data.data` and `data.metadata`) require
    /// an upgrade during the same mount cycle.
    ///
    /// We intentionally fail `Unseal` with `KeyRequiresUpgrade` multiple times to force both keys
    /// to traverse the upgrade path. This ensures that the fshost upgrade logic correctly handles
    /// multiple sequential upgrades without corrupting the persistence file or panicking, and
    /// successfully mounts the filesystem after all keys have been upgraded.
    #[fuchsia::test]
    async fn data_formatted_keymint_double_upgrade() {
        let mut builder = new_builder().with_crypt_policy(crypt_policy::Policy::Keymint);
        let shared_keymint = builder.keymint();
        let initial_blob = shared_keymint.generate_static_sealing_key(b"fuchsia");
        let disk = {
            builder.with_disk().format_volumes(volumes_spec()).format_data(keymint_data_fs_spec());
            let fixture = builder.build().await;
            fixture.tear_down().await.unwrap()
        };

        shared_keymint.bump_epoch();
        let fixture = new_builder()
            .with_crypt_policy(crypt_policy::Policy::Keymint)
            .with_disk_from(disk)
            .with_keymint_instance(shared_keymint.clone())
            .build()
            .await;

        fixture.check_fs_type("data", data_fs_type()).await;

        // Both data.data and data.metadata trigger an upgrade on the same `fuchsia` key payload.
        // Ensure the old shared key blob gets properly erased.
        let upgraded_blob = shared_keymint.generate_static_sealing_key(b"fuchsia");
        assert!(shared_keymint.has_key_blob(&upgraded_blob));
        assert!(!shared_keymint.has_key_blob(&initial_blob));

        fixture.tear_down().await;
    }

    #[fuchsia::test]
    async fn shred_data_volume_when_mounted_keymint() {
        let mut builder = new_builder().with_crypt_policy(crypt_policy::Policy::Keymint);
        builder.with_disk().format_volumes(volumes_spec()).format_data(keymint_data_fs_spec());
        let fixture = builder.build().await;

        fuchsia_fs::directory::open_file(
            &fixture.dir("data", fio::PERM_READABLE | fio::PERM_WRITABLE),
            "test-file",
            fio::Flags::FLAG_MAYBE_CREATE,
        )
        .await
        .expect("open_file failed");

        let admin: AdminProxy = fixture
            .realm
            .root
            .connect_to_protocol_at_exposed_dir()
            .expect("connect_to_protcol_at_exposed_dir failed");

        admin
            .shred_data_volume()
            .await
            .expect("shred_data_volume FIDL failed")
            .expect("shred_data_volume failed");

        let disk = fixture.tear_down().await.unwrap();

        let fixture = new_builder().with_disk_from(disk).build().await;

        // If we try and open the same test file, it shouldn't exist because the data volume should
        // have been shredded.
        assert_matches!(
            fuchsia_fs::directory::open_file(
                &fixture.dir("data", fio::PERM_READABLE),
                "test-file",
                fio::PERM_READABLE,
            )
            .await
            .expect_err("open_file failed"),
            fuchsia_fs::node::OpenError::OpenError(zx::Status::NOT_FOUND)
        );

        fixture.tear_down().await;
    }

    #[fuchsia::test]
    async fn test_provision_fxfs() {
        let mut builder = new_builder();
        builder.fshost().set_config_value("provision_fxfs", true);
        builder.fshost().set_config_value("merge_super_and_userdata", true);
        let mut fixture = builder.build().await;

        let mut disk = DiskBuilder::new();
        // Use unformatted volume manager to build an unformatted disk
        disk.with_gpt()
            .with_unformatted_volume_manager()
            .with_system_partition_label("super")
            .with_extra_gpt_partition("userdata", 1)
            .with_extra_gpt_partition("other", 1);
        fixture.add_main_disk(Disk::Builder(disk)).await;

        fixture.check_system_partitions(vec!["other", "super_and_userdata"]).await;
        fixture.check_fs_type("data", VFS_TYPE_FXFS).await;
        fixture.check_test_data_file().await;

        fixture.tear_down().await;
    }

    #[fuchsia::test]
    async fn set_volume_bytes_limit() {
        let mut builder = new_builder();
        builder
            .fshost()
            .set_config_value("data_max_bytes", data_max_bytes())
            .set_config_value("blob_max_bytes", BLOBFS_MAX_BYTES);
        builder.with_disk().format_volumes(volumes_spec());
        let fixture = builder.build().await;

        fixture.check_fs_type("blob", blob_fs_type()).await;
        fixture.check_fs_type("data", data_fs_type()).await;

        let volumes_dir = fixture.dir("volumes", fio::PERM_READABLE);

        let blob_volume_proxy =
            connect_to_named_protocol_at_dir_root::<FsStartupVolumeMarker>(&volumes_dir, "blob")
                .unwrap();
        let blob_volume_bytes_limit = blob_volume_proxy.get_limit().await.unwrap().unwrap();

        let data_volume_proxy =
            connect_to_named_protocol_at_dir_root::<FsStartupVolumeMarker>(&volumes_dir, "data")
                .unwrap();
        let data_volume_bytes_limit = data_volume_proxy.get_limit().await.unwrap().unwrap();
        assert_eq!(blob_volume_bytes_limit, BLOBFS_MAX_BYTES);
        assert_eq!(data_volume_bytes_limit, data_max_bytes());
        fixture.tear_down().await;
    }
}

#[fuchsia::test]
async fn debug_block_directory_has_bus_path() {
    let mut builder = new_builder();
    builder.with_disk().format_volumes(volumes_spec()).format_data(data_fs_spec());
    let fixture = builder.build().await;

    fixture.check_fs_type("blob", blob_fs_type()).await;
    fixture.check_fs_type("data", data_fs_type()).await;

    let block = fuchsia_fs::directory::open_directory(
        fixture.exposed_dir(),
        "debug_block",
        fio::PERM_READABLE,
    )
    .await
    .unwrap();

    let bus_topology =
        fuchsia_fs::directory::open_file(&block, "000/bus_path", fio::PERM_READABLE).await.unwrap();

    let content = fuchsia_fs::file::read_to_string(&bus_topology).await.unwrap();
    // Don't be too strict; just make sure it isn't <unknown> or <none> which come from fshost.
    assert!(!content.is_empty());
    assert!(!content.starts_with("<"));

    fixture.tear_down().await;
}
