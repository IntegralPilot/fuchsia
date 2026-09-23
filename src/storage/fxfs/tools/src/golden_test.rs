// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![recursion_limit = "256"]

mod golden_common;

use crate::golden_common::{
    BLOB_LIST_PATH, DEFAULT_VOLUME, DELETED_FILE_PATH, EXPECTED_FILE_CONTENT, IMAGE_BLOCK_SIZE,
    LARGE_ATTR_COUNT, LARGE_ATTR_PREFIX, MAX_INLINE_XATTR_SIZE, REGULAR_DIRECTORY_PATH,
    REGULAR_FILE_PATH, UNENCRYPTED_VOLUME, VERITY_FILE_PATH, WRAPPING_KEY_ID,
    latest_image_filename,
};
use anyhow::{Context, Error, ensure};
use fidl::endpoints::create_proxy;
use fidl_fuchsia_io as fio;
use fuchsia_fs::directory::{DirEntry, DirentKind, open_file, read_file, readdir_inclusive};
use fuchsia_hash::Hash;
use fxfs::filesystem::FxFilesystem;
use fxfs::fsck;
use fxfs::log::*;
use fxfs::object_store::volume::root_volume;
use fxfs::serialized_types::{AES_JOURNAL_ENCRYPTION_VERSION, LATEST_VERSION, Version};
use fxfs_crypto::Crypt;
use fxfs_insecure_crypto::new_insecure_crypt;
use fxfs_make_blob_image::BLOB_VOLUME_NAME;
use fxfs_platform::fuchsia::fxblob::BlobDirectory;
use fxfs_platform::fuchsia::volume::{FxVolumeAndRoot, MemoryPressureConfig};
use fxfs_platform::fuchsia::volumes_directory::VolumesDirectory;
use refaults_vmo::PageRefaultCounter;
use std::path::Path;
use std::sync::{Arc, Weak};
use storage_device::DeviceHolder;
use storage_device::fake_device::FakeDevice;
use zx::Status;

const ADDED_FILE_CONTENT: &[u8; 6] = &[0, 1, 2, 3, 4, 5];
const ADDED_FILE_PATH: &str = "some/test_file.txt";
const FSCRYPT_FILE_PATH: &str = "fscrypt/Strasse.txt";
const FXFS_GOLDEN_IMAGE_DATA_DIR: &str = "pkg/data/fxfs_golden_images";
const FXFS_GOLDEN_IMAGE_MANIFEST: &str = "golden_image_manifest.json";
// We started supporting fscrypt in version 47, but we changed to use the lblk32 algorithm from
// version 51, and we have since removed support for using fscrypt with Fxfs keys so we only verify
// fscrypt from version 51 onwards.
const FSCRYPT_VERSION_START: Version = Version { major: 51, minor: 0 };
// Version of first image with blobs included.
const BLOB_IN_GOLDEN_START: Version = Version { major: 52, minor: 0 };
const SECOND_VOLUME_VERSION: Version = Version { major: 38, minor: 0 };

/// Decompresses a zstd compressed local image into a RAM backed FakeDevice.
fn load_device(path: &Path) -> Result<FakeDevice, Error> {
    Ok(FakeDevice::from_image(zstd::Decoder::new(std::fs::File::open(path)?)?, IMAGE_BLOCK_SIZE)?)
}

async fn check_data_volume(
    dir: fio::DirectoryProxy,
    check_fscrypt: bool,
    version: Version,
) -> Result<(), Error> {
    let all_attributes = fio::NodeAttributesQuery::all();

    let (dir_mut_attrs, dir_imm_attrs) = dir
        .get_attributes(all_attributes)
        .await
        .context("get_attributes FIDL call on volume root dir")?
        .map_err(Status::err_from_raw)
        .context("get_attributes on volume root dir")?;
    ensure!(dir_imm_attrs.id.is_some(), "Expected ID for volume root dir");
    ensure!(
        dir_imm_attrs.protocols == Some(fio::NodeProtocolKinds::DIRECTORY),
        "Expected directory protocols"
    );
    ensure!(dir_mut_attrs.creation_time.is_some(), "Expected creation_time for volume root dir");

    let root_dirents = readdir_inclusive(&dir).await?;
    ensure!(
        root_dirents.contains(&DirEntry { name: ".".to_owned(), kind: DirentKind::Directory }),
        "Expected . in root dirents: {:?}",
        root_dirents
    );
    ensure!(
        root_dirents.contains(&DirEntry { name: "some".to_owned(), kind: DirentKind::Directory }),
        "Expected some in root dirents: {:?}",
        root_dirents
    );

    let some_dir =
        fuchsia_fs::directory::open_directory(&dir, REGULAR_DIRECTORY_PATH, fio::PERM_READABLE)
            .await?;
    let some_dirents = readdir_inclusive(&some_dir).await?;
    ensure!(
        some_dirents.contains(&DirEntry { name: "file.txt".to_owned(), kind: DirentKind::File }),
        "Expected file.txt in some dirents: {:?}",
        some_dirents
    );
    ensure!(
        some_dirents
            .contains(&DirEntry { name: "fsverity.txt".to_owned(), kind: DirentKind::File }),
        "Expected fsverity.txt in some dirents: {:?}",
        some_dirents
    );
    ensure!(
        !some_dirents.iter().any(|e| e.name == "deleted_file.txt"),
        "deleted_file.txt should not be in dirents"
    );
    ensure!(
        some_dir
            .get_extended_attribute(b"security.selinux")
            .await
            .context("FIDL call get_extended_attribute on directory")?
            .map_err(Status::err_from_raw)
            .context("get_extended_attribute on directory")?
            == fio::ExtendedAttributeValue::Bytes(b"test value".to_vec()),
        "Expected security.selinux xattr on some directory"
    );
    let (iterator_client, iterator_server) =
        fidl::endpoints::create_proxy::<fio::ExtendedAttributeIteratorMarker>();
    some_dir
        .list_extended_attributes(iterator_server)
        .context("list_extended_attributes on dir")?;
    let (entries, _) = iterator_client
        .get_next()
        .await
        .context("get_next FIDL call on dir xattrs")?
        .map_err(Status::err_from_raw)
        .context("get_next on dir xattrs")?;
    ensure!(
        entries.contains(&b"security.selinux".to_vec()),
        "Expected security.selinux in dir xattrs list"
    );

    let file = open_file(&dir, REGULAR_FILE_PATH, fio::PERM_READABLE).await?;
    let (mut_attrs, imm_attrs) = file
        .get_attributes(all_attributes)
        .await
        .context("get_attributes FIDL call on regular file")?
        .map_err(Status::err_from_raw)
        .context("get_attributes wrapping_key_id on regular file")?;
    ensure!(
        mut_attrs.wrapping_key_id == None,
        "Expected wrapping_key_id to be None for regular non-fscrypt file."
    );
    ensure!(imm_attrs.id.is_some(), "Expected ID for regular file");
    ensure!(
        imm_attrs.content_size == Some(EXPECTED_FILE_CONTENT.len() as u64),
        "Expected content_size for regular file"
    );
    ensure!(mut_attrs.creation_time.is_some(), "Expected creation_time for regular file");

    ensure!(
        &read_file(&dir, REGULAR_FILE_PATH).await? == EXPECTED_FILE_CONTENT,
        "Expected file content incorrect."
    );

    let reg_file = open_file(&dir, REGULAR_FILE_PATH, fio::PERM_READABLE).await?;
    ensure!(
        reg_file
            .get_extended_attribute(b"user.hash")
            .await
            .context("FIDL call get_extended_attribute on file")?
            .map_err(Status::err_from_raw)
            .context("get_extended_attribute on file")?
            == fio::ExtendedAttributeValue::Bytes(b"different value".to_vec()),
        "Expected user.hash xattr on regular file"
    );
    let (iterator_client, iterator_server) =
        fidl::endpoints::create_proxy::<fio::ExtendedAttributeIteratorMarker>();
    reg_file
        .list_extended_attributes(iterator_server)
        .context("list_extended_attributes on file")?;
    let (entries, _) = iterator_client
        .get_next()
        .await
        .context("get_next FIDL call on file xattrs")?
        .map_err(Status::err_from_raw)
        .context("get_next on file xattrs")?;
    ensure!(entries.contains(&b"user.hash".to_vec()), "Expected user.hash in file xattrs list");

    if version >= AES_JOURNAL_ENCRYPTION_VERSION {
        for i in 0..LARGE_ATTR_COUNT {
            let attr_name = format!("{LARGE_ATTR_PREFIX}{i}").into_bytes();
            ensure!(
                reg_file
                    .get_extended_attribute(&attr_name)
                    .await
                    .context("FIDL call get_extended_attribute on file for large attr")?
                    .map_err(Status::err_from_raw)
                    .context("get_extended_attribute on file for large attr")?
                    == fio::ExtendedAttributeValue::Bytes(vec![0u8; MAX_INLINE_XATTR_SIZE]),
                "Expected large attribute {attr_name:?} on regular file"
            );
        }
    }

    let verity_file = open_file(&dir, VERITY_FILE_PATH, fio::PERM_READABLE).await?;
    let (verity_mut_attrs, verity_imm_attrs) = verity_file
        .get_attributes(all_attributes)
        .await
        .context("get_attributes FIDL call on verity file")?
        .map_err(Status::err_from_raw)
        .context("get_attributes on verity file")?;
    ensure!(verity_imm_attrs.id.is_some(), "Expected ID for verity file");
    ensure!(
        verity_imm_attrs.content_size == Some(EXPECTED_FILE_CONTENT.len() as u64),
        "Expected content_size for verity file"
    );
    ensure!(verity_mut_attrs.creation_time.is_some(), "Expected creation_time for verity file");

    ensure!(
        &read_file(&dir, VERITY_FILE_PATH).await? == EXPECTED_FILE_CONTENT,
        "Expected fsverity content incorrect."
    );

    ensure!(
        dir.unlink(DELETED_FILE_PATH, &fio::UnlinkOptions::default()).await.unwrap().is_err(),
        "Found deleted file."
    );

    if check_fscrypt {
        let file = open_file(&dir, FSCRYPT_FILE_PATH, fio::PERM_READABLE).await?;
        let (mut_attrs, imm_attrs) = file
            .get_attributes(all_attributes)
            .await
            .context("get_attributes FIDL call on fscrypt file")?
            .map_err(Status::err_from_raw)
            .context("get_attributes wrapping_key_id on fscrypt file")?;
        ensure!(
            mut_attrs.wrapping_key_id == Some(WRAPPING_KEY_ID),
            "Expected wrapping_key_id for fscrypt file."
        );
        ensure!(imm_attrs.id.is_some(), "Expected ID for fscrypt file");
        ensure!(
            imm_attrs.content_size == Some(EXPECTED_FILE_CONTENT.len() as u64),
            "Expected content_size for fscrypt file"
        );

        let fscrypt_dir =
            fuchsia_fs::directory::open_directory(&dir, "fscrypt", fio::PERM_READABLE).await?;
        let fscrypt_dirents = readdir_inclusive(&fscrypt_dir).await?;
        ensure!(
            fscrypt_dirents
                .contains(&DirEntry { name: "Straße.txt".to_owned(), kind: DirentKind::File }),
            "Expected Straße.txt in fscrypt dirents: {:?}",
            fscrypt_dirents
        );

        ensure!(
            &read_file(&fscrypt_dir, "Straße.txt").await? == EXPECTED_FILE_CONTENT,
            "Expected fscrypt content via exact name."
        );
        ensure!(
            &read_file(&fscrypt_dir, "Strasse.txt").await? == EXPECTED_FILE_CONTENT,
            "Expected fscrypt content via casefolded Strasse.txt."
        );
        ensure!(
            &read_file(&fscrypt_dir, "STRASSE.TXT").await? == EXPECTED_FILE_CONTENT,
            "Expected fscrypt content via casefolded uppercase STRASSE.TXT."
        );

        // Check fscrypt file read with a casefolded unicode filename.
        ensure!(
            &read_file(&dir, FSCRYPT_FILE_PATH).await? == EXPECTED_FILE_CONTENT,
            "Expected fscrypt content."
        );
    }

    let file = open_file(
        &dir,
        ADDED_FILE_PATH,
        fio::Flags::PROTOCOL_FILE
            | fio::Flags::FLAG_MUST_CREATE
            | fio::PERM_READABLE
            | fio::PERM_WRITABLE,
    )
    .await?;
    ensure!(
        file.write(ADDED_FILE_CONTENT).await.unwrap().map_err(Status::err_from_raw)?
            == ADDED_FILE_CONTENT.len() as u64,
        "Writing file"
    );
    file.set_extended_attribute(
        b"user.new_attr",
        fio::ExtendedAttributeValue::Bytes(b"new value".to_vec()),
        fio::SetExtendedAttributeMode::Set,
    )
    .await
    .context("FIDL set_extended_attribute")?
    .map_err(Status::err_from_raw)
    .context("set_extended_attribute on test file")?;
    ensure!(
        file.get_extended_attribute(b"user.new_attr")
            .await
            .context("FIDL get_extended_attribute on test file")?
            .map_err(Status::err_from_raw)?
            == fio::ExtendedAttributeValue::Bytes(b"new value".to_vec()),
        "Expected new xattr value"
    );
    file.remove_extended_attribute(b"user.new_attr")
        .await
        .context("FIDL remove_extended_attribute")?
        .map_err(Status::err_from_raw)
        .context("remove_extended_attribute on test file")?;
    ensure!(
        file.get_extended_attribute(b"user.new_attr")
            .await
            .context("FIDL get_extended_attribute after remove")?
            .is_err(),
        "Expected xattr to be removed"
    );

    let some_dir_rw = fuchsia_fs::directory::open_directory(
        &dir,
        "some",
        fio::PERM_READABLE | fio::PERM_WRITABLE,
    )
    .await?;
    let (status, handle) = some_dir_rw.get_token().await.context("FIDL get_token for link")?;
    Status::ok(status).context("get_token status")?;
    let token = zx::Event::from(handle.unwrap());
    let status = some_dir_rw
        .link("test_file.txt", token.into(), "linked_test_file.txt")
        .await
        .context("FIDL link")?;
    Status::ok(status).context("link on directory")?;
    ensure!(
        &read_file(&some_dir_rw, "linked_test_file.txt").await? == ADDED_FILE_CONTENT,
        "Linked file contents"
    );

    let (status, handle) = some_dir_rw.get_token().await.context("FIDL get_token for rename")?;
    Status::ok(status).context("get_token status")?;
    let token = zx::Event::from(handle.unwrap());
    let res = some_dir_rw
        .rename("linked_test_file.txt", token.into(), "renamed_test_file.txt")
        .await
        .context("FIDL rename")?;
    res.map_err(Status::err_from_raw).context("rename on directory")?;
    ensure!(
        &read_file(&some_dir_rw, "renamed_test_file.txt").await? == ADDED_FILE_CONTENT,
        "Renamed file contents"
    );
    ensure!(
        some_dir_rw
            .unlink("linked_test_file.txt", &fio::UnlinkOptions::default())
            .await
            .unwrap()
            .is_err(),
        "Old file name after rename should not exist"
    );
    let res = some_dir_rw
        .unlink("renamed_test_file.txt", &fio::UnlinkOptions::default())
        .await
        .context("FIDL unlink renamed file")?;
    res.map_err(Status::err_from_raw).context("unlink renamed file")?;

    Ok(())
}

async fn check_blob(dir: &Arc<BlobDirectory>, hash: Hash) -> Result<(), Error> {
    let vmo = dir.get_blob_vmo(hash).await?;
    let size = vmo.get_size()?;
    // Read in the whole blob to verify it.
    const STEP_SIZE: usize = 1024 * 32;
    let mut buf = vec![0u8; STEP_SIZE];
    for offset in (0..size).step_by(STEP_SIZE) {
        let len = std::cmp::min((size - offset) as usize, STEP_SIZE);
        vmo.read(&mut buf[..len], offset)?;
    }
    Ok(())
}

async fn check_blob_volume(
    vol_and_dir: FxVolumeAndRoot,
    blob_list: Vec<[u8; 32]>,
) -> Result<(), Error> {
    let dir = vol_and_dir.root().clone().into_any().downcast::<BlobDirectory>().unwrap();
    for blob in blob_list {
        let hash = Hash::from_array(blob);
        check_blob(&dir, hash).await.with_context(|| format!("Opening blob {:?}", hash))?;
    }
    Ok(())
}

/// Validates an image by looking for expected data and performing an fsck.
async fn check_image(path: &Path) -> Result<(), Error> {
    let device = DeviceHolder::new(load_device(path)?);
    let fs = FxFilesystem::open(device).await?;
    let version = fs.journal().super_block_header().earliest_version;

    let insecure_crypt = new_insecure_crypt();
    insecure_crypt.add_wrapping_key(WRAPPING_KEY_ID, [1; 32].into()).expect("Failed to add key");
    let crypt: Arc<dyn Crypt> = Arc::new(insecure_crypt);
    let mut volumes = vec![(DEFAULT_VOLUME, Some(crypt.clone()), false)];
    if version >= SECOND_VOLUME_VERSION {
        volumes.push((UNENCRYPTED_VOLUME, None, false));
    }
    if version >= BLOB_IN_GOLDEN_START {
        volumes.push((BLOB_VOLUME_NAME, None, true));
    }
    {
        let volumes_dir = VolumesDirectory::new(
            root_volume(fs.clone()).await.expect("Root volume creation"),
            Weak::new(),
            None,
            Arc::new(PageRefaultCounter::new().expect("Failed to create PageRefaultCounter")),
            MemoryPressureConfig::default(),
        )
        .await
        .context("VolumesDirectory creation")?;

        let mut blob_list = Vec::new();

        for (vol_name, vol_crypt, is_blob) in volumes.clone() {
            let check_fscrypt = version >= FSCRYPT_VERSION_START && vol_crypt.is_some();
            let vol_and_dir = volumes_dir
                .mount_volume(vol_name, vol_crypt.as_ref().map(|c| c.clone()), is_blob)
                .await?;
            fsck::fsck_volume(
                &vol_and_dir.volume().store().filesystem(),
                vol_and_dir.volume().store().store_object_id(),
                vol_crypt,
            )
            .await?;

            if is_blob {
                ensure!(
                    !blob_list.is_empty(),
                    "Should have found blobs listed in unencrypted volume. Did they open in the
                    correct order?"
                );
                check_blob_volume(vol_and_dir, std::mem::take(&mut blob_list))
                    .await
                    .with_context(|| format!("Checking {}", vol_name))?;
            } else {
                let (dir, server_end) = create_proxy::<fio::DirectoryMarker>();
                vol_and_dir
                    .root()
                    .clone()
                    .serve(fio::PERM_READABLE | fio::PERM_WRITABLE, server_end);
                if vol_name == UNENCRYPTED_VOLUME && version >= BLOB_IN_GOLDEN_START {
                    let list_bytes = read_file(&dir, BLOB_LIST_PATH).await?;
                    let (blob_hashes, extra) = list_bytes.as_chunks::<32>();
                    ensure!(extra.is_empty(), "Blob listing should be exact");
                    for blob_hash in blob_hashes {
                        blob_list.push(blob_hash.clone());
                    }
                }
                check_data_volume(dir, check_fscrypt, version)
                    .await
                    .with_context(|| format!("Checking {}", vol_name))?;
            }
        }

        fsck::fsck(fs.clone()).await?;
        volumes_dir.terminate().await;
    }
    fs.close().await?;
    let device = fs.take_device().await;
    device.reopen(false);

    let fs = FxFilesystem::open(device).await?;
    {
        let volumes_dir = VolumesDirectory::new(
            root_volume(fs.clone()).await.expect("Root volume creation"),
            Weak::new(),
            None,
            Arc::new(PageRefaultCounter::new().expect("Failed to create PageRefaultCounter")),
            MemoryPressureConfig::default(),
        )
        .await
        .context("VolumesDirectory recreation")?;
        for (vol_name, vol_crypt, is_blob) in volumes {
            let vol_and_dir = volumes_dir
                .mount_volume(vol_name, vol_crypt.as_ref().map(|c| c.clone()), is_blob)
                .await?;
            fsck::fsck_volume(
                &vol_and_dir.volume().store().filesystem(),
                vol_and_dir.volume().store().store_object_id(),
                vol_crypt,
            )
            .await?;
            if !is_blob {
                let (dir, server_end) = create_proxy::<fio::DirectoryMarker>();
                vol_and_dir
                    .root()
                    .clone()
                    .serve(fio::PERM_READABLE | fio::PERM_WRITABLE, server_end);
                ensure!(
                    &read_file(&dir, ADDED_FILE_PATH).await? == ADDED_FILE_CONTENT,
                    "Expected content in new file."
                );
            }
        }
        fsck::fsck(fs.clone()).await?;
        volumes_dir.terminate().await;
    }

    fs.journal().force_compact().await?;
    assert_eq!(fs.journal().super_block_header().earliest_version, LATEST_VERSION);
    fs.close().await
}

#[fuchsia::test(threads = 10)]
async fn test_golden_images() {
    let golden_dir = Path::new(FXFS_GOLDEN_IMAGE_DATA_DIR);
    let mut golden_files: Vec<String> = {
        let manifest_path = golden_dir.join(FXFS_GOLDEN_IMAGE_MANIFEST);
        let manifest_contents =
            std::fs::read_to_string(manifest_path.clone()).expect("Failed to read golden manifest");
        serde_json::from_str(&manifest_contents).expect("Failed to parse manifest json")
    };

    // First check that there exists an image for the latest version.
    assert!(
        golden_files.contains(&latest_image_filename()),
        "Golden image is missing for version {} ({}). Please run 'fx fxfs create_golden'",
        LATEST_VERSION,
        latest_image_filename()
    );

    // Next ensure that we can parse all golden images and validate expected content.
    golden_files.sort();
    for golden_file in golden_files {
        let path_buf = golden_dir.join(&golden_file);
        info!("Validating {}", path_buf.display());
        check_image(path_buf.as_path())
            .await
            .with_context(|| format!("Validating {}", path_buf.display()))
            .expect("Checking image");
    }
}
