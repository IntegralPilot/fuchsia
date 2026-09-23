// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::golden_common::{
    BLOB_LIST_PATH, DEFAULT_VOLUME, DELETED_FILE_PATH, EXPECTED_FILE_CONTENT, IMAGE_BLOCK_SIZE,
    LARGE_ATTR_COUNT, LARGE_ATTR_PREFIX, MAX_INLINE_XATTR_SIZE, REGULAR_DIRECTORY_PATH,
    REGULAR_FILE_PATH, UNENCRYPTED_VOLUME, VERITY_FILE_PATH, WRAPPING_KEY_ID,
    latest_image_filename,
};
use crate::ops;
use anyhow::{Context, Error, bail};
use chrono::Local;
use fxfs::errors::FxfsError;
use fxfs::filesystem::{FxFilesystem, OpenFxFilesystem, SyncOptions};
use fxfs::object_store::transaction::{LockKey, Mutation, Options, lock_keys};
use fxfs::object_store::{ObjectKey, ObjectStore, ObjectValue, ProjectId};
use fxfs_crypto::Crypt;
use fxfs_insecure_crypto::new_insecure_crypt;
use fxfs_make_blob_image::{CompressionAlgorithm, FxBlobBuilder};
use rand::rngs::SmallRng;
use rand::{RngExt as _, SeedableRng as _};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use storage_device::DeviceHolder;
use storage_device::fake_device::FakeDevice;

const FSCRYPT_UNICODE_FILE_PATH: &str = "fscrypt/Straße.txt";
const FXFS_GOLDEN_IMAGE_DIR: &str = "src/storage/fxfs/testdata";
const IMAGE_BLOCKS: u64 = 8192;
const PROJECT_ID: ProjectId = ProjectId::new(4).unwrap();

/// Uses FUCHSIA_DIR environment variable to generate a path to the expected location of golden
/// images. Note that we do this largely for ergonomics because this binary is typically invoked
/// by running `fx fxfs create_golden` from an arbitrary directory.
fn golden_image_dir() -> Result<PathBuf, Error> {
    let fuchsia_dir = std::env::vars().find(|(k, _)| k == "FUCHSIA_DIR");
    if fuchsia_dir.is_none() {
        bail!("FUCHSIA_DIR environment variable is not set.");
    }
    let (_, fuchsia_dir) = fuchsia_dir.unwrap();
    Ok(PathBuf::from(fuchsia_dir).join(FXFS_GOLDEN_IMAGE_DIR))
}

/// Compresses contents of a device into a zstd compressed local image.
async fn save_device(device: DeviceHolder, path: &Path) -> Result<(), Error> {
    device.reopen(true);
    let mut writer = zstd::Encoder::new(std::fs::File::create(path)?, 6)?;
    let mut buf = device.allocate_buffer(device.block_size() as usize).await;
    let mut offset: u64 = 0;
    while offset < IMAGE_BLOCKS * IMAGE_BLOCK_SIZE as u64 {
        device.read(offset, buf.as_mut()).await?;
        let data = buf.to_vec();
        writer.write_all(&data)?;
        offset += device.block_size() as u64;
    }
    writer.finish()?;
    Ok(())
}

/// Takes a path, removing the file at the end.
fn drop_file_from_path(original: &str) -> PathBuf {
    Path::new(original).parent().map(Path::to_path_buf).unwrap_or_else(PathBuf::new)
}

async fn activity_in_volume(fs: &OpenFxFilesystem, vol: &Arc<ObjectStore>) -> Result<(), Error> {
    let regular_dir_path = Path::new(REGULAR_DIRECTORY_PATH);
    ops::mkdir(fs, vol, regular_dir_path).await?;
    // Apply limit to project id and apply that both to the "some" directory to have it get applied
    // everywhere else.
    ops::set_project_limit(vol, PROJECT_ID, 102400, 1024).await?;
    ops::set_project_for_node(vol, PROJECT_ID, regular_dir_path).await?;

    ops::put(fs, vol, &Path::new(REGULAR_FILE_PATH), EXPECTED_FILE_CONTENT.to_vec()).await?;
    ops::put(fs, vol, &Path::new(DELETED_FILE_PATH), EXPECTED_FILE_CONTENT.to_vec()).await?;
    // Compact here and below so that there are some persistent files added.
    fs.journal().force_compact().await?;
    ops::unlink(fs, vol, &Path::new(DELETED_FILE_PATH)).await?;
    ops::put(fs, vol, &Path::new(VERITY_FILE_PATH), EXPECTED_FILE_CONTENT.to_vec()).await?;
    ops::enable_verity(vol, &Path::new(VERITY_FILE_PATH)).await?;

    fs.journal().force_compact().await?;

    ops::set_extended_attribute_for_node(vol, regular_dir_path, b"security.selinux", b"test value")
        .await?;
    ops::set_extended_attribute_for_node(
        vol,
        &Path::new(REGULAR_FILE_PATH),
        b"user.hash",
        b"different value",
    )
    .await?;

    // Create a large transaction with many extended attributes.
    {
        let reg_file_path = Path::new(REGULAR_FILE_PATH);
        let dir = ops::walk_dir(vol, reg_file_path.parent().unwrap()).await?;
        let filename = reg_file_path.file_name().unwrap().to_str().unwrap();
        let (node_id, _, _) = dir.lookup(filename).await?.ok_or(FxfsError::NotFound)?;
        let mut transaction = vol
            .new_transaction(
                lock_keys![LockKey::object(vol.store_object_id(), node_id)],
                Options::default(),
            )
            .await?;
        for i in 0..LARGE_ATTR_COUNT {
            let name = format!("{LARGE_ATTR_PREFIX}{i}").into_bytes();
            let mutation = Mutation::replace_or_insert_object(
                ObjectKey::extended_attribute(node_id, name),
                ObjectValue::inline_extended_attribute(vec![0u8; MAX_INLINE_XATTR_SIZE]),
            );
            transaction.add(vol.store_object_id(), mutation);
        }
        transaction.commit().await?;
    }

    // Exercise fscrypt and casefold with unicode filenames.
    if vol.crypt().is_some() {
        let fscrypt_dir_path = drop_file_from_path(FSCRYPT_UNICODE_FILE_PATH);
        ops::mkdir(fs, vol, fscrypt_dir_path.as_path()).await?;
        ops::enable_fscrypt(fs, vol, fscrypt_dir_path.as_path(), WRAPPING_KEY_ID).await?;
        ops::enable_casefold(vol, fscrypt_dir_path.as_path()).await?;
        ops::put(fs, vol, &Path::new(FSCRYPT_UNICODE_FILE_PATH), EXPECTED_FILE_CONTENT.to_vec())
            .await?;
    }

    Ok(())
}

pub async fn install_blobs(builder: &FxBlobBuilder) -> Result<Vec<[u8; 32]>, Error> {
    let mut blob_names = Vec::new();

    // A large and easily compressed blob, since compression is enabled. Aim to exceed a single
    // compression chunk.
    {
        let blob_info = builder
            .generate_blob(vec![42; 200000], Some(CompressionAlgorithm::Zstd))
            .context("Generate compressible blob")?;
        builder.install_blob(&blob_info).await.context("Install compressible blob")?;
        blob_names.push(blob_info.hash().as_array::<32>().unwrap().clone());
    }

    // A random blob, big enough to have a merkle tree.
    {
        let mut rng = SmallRng::seed_from_u64(42);
        let mut data = Vec::new();
        data.resize_with(32000, || rng.random());
        let blob_info = builder
            .generate_blob(data, Some(CompressionAlgorithm::Zstd))
            .context("Generate random blob")?;
        builder.install_blob(&blob_info).await.context("Install random blob")?;
        blob_names.push(blob_info.hash().as_array::<32>().unwrap().clone());
    }

    // A smaller blob, small enough to not have a merkle tree.
    {
        let blob_info = builder
            .generate_blob(vec![42; 100], Some(CompressionAlgorithm::Zstd))
            .context("Generate small blob")?;
        builder.install_blob(&blob_info).await.context("Install small blob")?;
        blob_names.push(blob_info.hash().as_array::<32>().unwrap().clone());
    }

    Ok(blob_names)
}

/// Create a new golden image (at the current version).
pub async fn create_image() -> Result<(), Error> {
    let path = golden_image_dir()?.join(latest_image_filename());

    let (device, blob_names) = {
        let blob_image_builder =
            FxBlobBuilder::new(DeviceHolder::new(FakeDevice::new(IMAGE_BLOCKS, IMAGE_BLOCK_SIZE)))
                .await
                .context("Creating blob image builder")?;
        let blob_names = install_blobs(&blob_image_builder).await?;
        (blob_image_builder.finalize().await.context("Finalize blob image")?.0, blob_names)
    };
    device.ensure_unique();
    device.reopen(false);
    let fs = FxFilesystem::open(device).await?;

    let insecure_crypt = new_insecure_crypt();
    insecure_crypt.add_wrapping_key(WRAPPING_KEY_ID, [1; 32].into()).expect("Failed to add key");
    let crypt: Arc<dyn Crypt> = Arc::new(insecure_crypt);
    let default_vol = ops::create_volume(&fs, DEFAULT_VOLUME, Some(crypt.clone())).await?;

    let unencrypted_vol = ops::create_volume(&fs, UNENCRYPTED_VOLUME, None).await?;

    // Write the blob names that need to be verified into the unencrypted volume.
    ops::put(
        &fs,
        &unencrypted_vol,
        &Path::new(BLOB_LIST_PATH),
        blob_names.iter().flatten().cloned().collect(),
    )
    .await
    .context("Writing blob list")?;

    for (vol, msg) in [(&default_vol, "default volume"), (&unencrypted_vol, "unencrypted volume")] {
        activity_in_volume(&fs, vol).await.context(msg)?;
    }

    // Write enough stuff to the journal (journal::BLOCK_SIZE per sync) to ensure we would fill
    // the disk without reclaim of both journal and file data.
    let num_iters = 2000;
    let before_generation = fs.super_block_header().generation;
    for _i in 0..num_iters {
        ops::put(&fs, &default_vol, &Path::new("some/repeat.txt"), EXPECTED_FILE_CONTENT.to_vec())
            .await?;
        fs.sync(SyncOptions { flush_device: true, precondition: None }).await?;
        ops::unlink(&fs, &default_vol, &Path::new("some/repeat.txt")).await?;
        fs.sync(SyncOptions { flush_device: true, precondition: None }).await?;
    }

    // Ensure that we have reclaimed the journal at least once.
    assert_ne!(before_generation, fs.super_block_header().generation);
    fs.close().await?;
    let device = fs.take_device().await;
    save_device(device, &path).await?;

    let mut file = std::fs::File::create(golden_image_dir()?.join("images.gni").as_path())?;
    file.write_all(
        format!(
            "# Copyright {} The Fuchsia Authors. All rights reserved.\n\
             #\n\
             # Use of this source code is governed by a BSD-style license that can be\n\
             # found in the LICENSE file.\n\
             #\n\
             # Auto-generated by `fx fxfs create_golden`\n\
             # on {}\n",
            Local::now().format("%Y"),
            Local::now()
        )
        .as_bytes(),
    )?;
    file.write_all(b"fxfs_golden_images = [\n")?;
    let mut paths = std::fs::read_dir(golden_image_dir()?)?.collect::<Result<Vec<_>, _>>()?;
    paths.sort_unstable_by_key(|path| path.path().to_str().unwrap().to_string());
    for file_name in
        paths.iter().map(|e| e.file_name()).filter(|x| x.to_str().unwrap().ends_with(".zstd"))
    {
        file.write_all(format!("  \"{}\",\n", file_name.to_str().unwrap()).as_bytes())?;
    }
    file.write_all(b"]\n")?;
    Ok(())
}
