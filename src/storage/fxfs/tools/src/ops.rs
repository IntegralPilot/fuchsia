// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, bail};
use chrono::{TimeZone, Utc};
use fidl_fuchsia_io as fio;
use fxfs::errors::FxfsError;
use fxfs::filesystem::OpenFxFilesystem;
use fxfs::fsck;
use fxfs::object_handle::{ObjectHandle, WriteObjectHandle};
use fxfs::object_store::directory::{ReplacedChild, replace_child};
use fxfs::object_store::transaction::{LockKey, Options, lock_keys};
use fxfs::object_store::volume::root_volume;
use fxfs::object_store::{
    Directory, HandleOptions, NewChildStoreOptions, ObjectDescriptor, ObjectStore, ProjectId,
    SetExtendedAttributeMode, StoreObjectHandle, StoreOptions,
};
use fxfs_crypto::{Crypt, WrappingKeyId};
use sparse::reader::SparseReader;
use sparse::{build_sparse_files, unsparse};
use std::io::Seek;
use std::ops::Deref;
use std::path::Path;
use std::sync::Arc;
use storage_device::DeviceHolder;
use storage_device::file_backed_device::FileBackedDevice;

pub async fn print_ls(dir: &Directory<ObjectStore>) -> Result<(), Error> {
    const DATE_FMT: &str = "%b %d %Y %T+00";
    let layer_set = dir.store().tree().layer_set();
    let mut merger = layer_set.merger();
    let mut iter = dir.iter(&mut merger).await?;
    while let Some((name, object_id, descriptor)) = iter.get() {
        match descriptor {
            ObjectDescriptor::File => {
                let handle = ObjectStore::open_object(
                    dir.owner(),
                    object_id,
                    HandleOptions::default(),
                    None,
                )
                .await?;
                let properties = handle.get_properties().await?;
                let size = properties.data_attribute_size;
                let mtime = Utc
                    .timestamp_opt(
                        properties.modification_time.as_secs() as i64,
                        properties.modification_time.subsec_nanos(),
                    )
                    .unwrap();
                println!(
                    "-rwx------    1 nobody   nogroup    {:>8} {:>12} {}",
                    size,
                    mtime.format(DATE_FMT),
                    name
                );
            }
            ObjectDescriptor::Directory => {
                let mtime = Utc.timestamp_opt(0, 0).unwrap();
                println!(
                    "d---------    1 nobody   nogroup           0 {:>12} {}",
                    mtime.format(DATE_FMT),
                    name
                );
            }
            ObjectDescriptor::Volume => unimplemented!(),
            ObjectDescriptor::Symlink => {
                let link = dir.store().read_symlink(object_id).await?;
                let mtime = Utc::now();
                println!(
                    "l---------    1 nobody   nogroup           0 {:>12} {} -> {}",
                    mtime.format(DATE_FMT),
                    name,
                    String::from_utf8_lossy(&link),
                );
            }
        }
        iter.advance().await?;
    }
    Ok(())
}

/// Make a volume
pub async fn create_volume(
    fs: &OpenFxFilesystem,
    name: &str,
    crypt: Option<Arc<dyn Crypt>>,
) -> Result<Arc<ObjectStore>, Error> {
    let root_volume = root_volume(fs.deref().clone()).await?;
    root_volume
        .new_volume(
            name,
            NewChildStoreOptions {
                options: StoreOptions { crypt, ..StoreOptions::default() },
                ..Default::default()
            },
        )
        .await
}

/// Opens a volume on a device and returns a Directory to it's root.
pub async fn open_volume(
    fs: &OpenFxFilesystem,
    name: &str,
    crypt: Option<Arc<dyn Crypt>>,
) -> Result<Arc<ObjectStore>, Error> {
    let root_volume = root_volume(fs.deref().clone()).await?;
    root_volume.volume(name, StoreOptions { crypt }).await.map(|v| v.into())
}

/// Walks a directory path from a given root.
pub async fn walk_dir(
    vol: &Arc<ObjectStore>,
    path: &Path,
) -> Result<Directory<ObjectStore>, Error> {
    let mut dir: Directory<ObjectStore> =
        Directory::open(vol, vol.root_directory_object_id()).await?;
    for path in path.to_str().unwrap().split('/') {
        if path.len() == 0 {
            continue;
        }
        if let Some((object_id, descriptor, _)) = dir.lookup(&path).await? {
            if descriptor != ObjectDescriptor::Directory {
                bail!("Not a directory: {}", path);
            }
            dir = Directory::open(&dir.owner(), object_id).await?;
        } else {
            bail!("Not found: {}", path);
        }
    }
    Ok(dir)
}

pub async fn unlink(
    fs: &OpenFxFilesystem,
    vol: &Arc<ObjectStore>,
    path: &Path,
) -> Result<(), Error> {
    let dir = walk_dir(vol, path.parent().unwrap()).await?;
    // Not worried about the race between lookup and lock, this is a single-threaded mode.
    let replace_context = dir
        .acquire_context_for_replace(None, path.file_name().unwrap().to_str().unwrap(), true)
        .await?;
    let mut transaction = replace_context.transaction;
    if replace_context.dst_id_and_descriptor.is_none() {
        bail!("Object not found.");
    }
    let replaced_child =
        replace_child(&mut transaction, None, (&dir, path.file_name().unwrap().to_str().unwrap()))
            .await?;
    transaction.commit().await?;
    // In FxFile, when a handle goes out of scope we'd deref and potentially tombstone
    // it (delete it from graveyard). We don't refcount so we just manually queue tombstone here.
    if let ReplacedChild::Object(object_id) = replaced_child {
        fs.graveyard()
            .tombstone_object(
                dir.store().store_object_id(),
                object_id,
                replace_context.truncate_guard.as_ref(),
            )
            .await?;
    }
    Ok(())
}

// Used to fsck after mutating operations.
pub async fn fsck(fs: &OpenFxFilesystem, verbose: bool) -> Result<fsck::FsckResult, Error> {
    // Re-open the filesystem to ensure it's locked.
    fsck::fsck_with_options(
        fs.deref().clone(),
        &fsck::FsckOptions {
            on_error: Box::new(|err| eprintln!("{:?}", err.to_string())),
            verbose,
            ..Default::default()
        },
    )
    .await
}

/// Read a file's contents into a Vec and return it.
pub async fn get(vol: &Arc<ObjectStore>, src: &Path) -> Result<Vec<u8>, Error> {
    let dir = walk_dir(vol, src.parent().unwrap()).await?;
    if let Some((object_id, descriptor, _)) =
        dir.lookup(&src.file_name().unwrap().to_str().unwrap()).await?
    {
        if descriptor != ObjectDescriptor::File {
            bail!("Expected File. Found {:?}", descriptor);
        }
        let handle =
            ObjectStore::open_object(dir.owner(), object_id, HandleOptions::default(), None)
                .await?;
        let mut out: Vec<u8> = Vec::new();
        let mut buf = handle.allocate_buffer(handle.block_size().get() as usize).await;
        let mut ofs = 0;
        loop {
            let bytes = handle.read(ofs, buf.as_mut()).await?;
            ofs += bytes as u64;
            buf.subslice(..bytes).append_to(&mut out);
            if bytes as u64 != handle.block_size() {
                break;
            }
        }
        Ok(out)
    } else {
        bail!("File not found: {}", src.display());
    }
}

/// Write the contents of a Vec to a file in the image.
pub async fn put(
    fs: &OpenFxFilesystem,
    vol: &Arc<ObjectStore>,
    dst: &Path,
    data: Vec<u8>,
) -> Result<(), Error> {
    let dir = walk_dir(vol, dst.parent().unwrap()).await?;
    let filename = dst.file_name().unwrap().to_str().unwrap();
    let mut transaction = fs
        .root_store()
        .new_transaction(
            lock_keys![LockKey::object(vol.store_object_id(), dir.object_id())],
            Options::default(),
        )
        .await?;
    if let Some(_) = dir.lookup(filename).await? {
        bail!("{} already exists", filename);
    }
    let handle = dir.create_child_file(&mut transaction, &filename).await?;
    transaction.commit().await?;
    let mut buf = handle.allocate_buffer(data.len()).await;
    buf.copy_from_slice(&data);
    handle.write_or_append(Some(0), buf.as_ref()).await?;
    handle.flush().await
}

/// Enable verity on an existing file. Upon enabling verity, the file will be readonly.
pub async fn enable_verity(vol: &Arc<ObjectStore>, dst: &Path) -> Result<(), Error> {
    let dir = walk_dir(vol, dst.parent().unwrap()).await?;
    let filename = dst.file_name().unwrap().to_str().unwrap();
    let handle = if let Some((oid, _, _)) = dir.lookup(filename).await? {
        ObjectStore::open_object(vol, oid, HandleOptions::default(), None).await?
    } else {
        bail!("{} does not exist", filename);
    };

    handle
        .enable_verity(fio::VerificationOptions {
            hash_algorithm: Some(fio::HashAlgorithm::Sha256),
            salt: Some(vec![0xFF; 8]),
            ..Default::default()
        })
        .await
}

pub async fn enable_casefold(vol: &Arc<ObjectStore>, dst: &Path) -> Result<(), Error> {
    walk_dir(vol, dst).await?.set_casefold(true).await
}

pub async fn enable_fscrypt(
    fs: &OpenFxFilesystem,
    vol: &Arc<ObjectStore>,
    dst: &Path,
    wrapping_key_id: WrappingKeyId,
) -> Result<(), Error> {
    let dir = walk_dir(vol, dst).await?;
    let mut transaction = fs
        .root_store()
        .new_transaction(
            lock_keys![LockKey::object(dir.store().store_object_id(), dir.object_id())],
            Options::default(),
        )
        .await?;
    dir.set_wrapping_key(&mut transaction, wrapping_key_id).await?;
    transaction.commit().await?;
    Ok(())
}

/// Create a directory.
pub async fn mkdir(
    fs: &OpenFxFilesystem,
    vol: &Arc<ObjectStore>,
    path: &Path,
) -> Result<(), Error> {
    let dir = walk_dir(vol, path.parent().unwrap()).await?;
    let filename = path.file_name().unwrap().to_str().unwrap();
    let mut transaction = fs
        .root_store()
        .new_transaction(
            lock_keys![LockKey::object(vol.store_object_id(), dir.object_id())],
            Options::default(),
        )
        .await?;
    if let Some(_) = dir.lookup(filename).await? {
        bail!("{} already exists", filename);
    }
    dir.create_child_dir(&mut transaction, &filename).await?;
    transaction.commit().await?;
    Ok(())
}

pub async fn set_project_limit(
    vol: &Arc<ObjectStore>,
    project_id: ProjectId,
    byte_limit: u64,
    node_limit: u64,
) -> Result<(), Error> {
    vol.set_project_limit(project_id, byte_limit, node_limit).await
}

pub async fn set_project_for_node(
    vol: &Arc<ObjectStore>,
    project_id: ProjectId,
    path: &Path,
) -> Result<(), Error> {
    let dir = walk_dir(vol, path.parent().unwrap()).await?;
    let filename = path.file_name().unwrap().to_str().unwrap();
    let (node_id, _, _) = dir.lookup(filename).await?.ok_or(FxfsError::NotFound)?;
    vol.set_project_for_node(node_id, project_id).await
}

pub async fn set_extended_attribute_for_node(
    vol: &Arc<ObjectStore>,
    path: &Path,
    name: &[u8],
    value: &[u8],
) -> Result<(), Error> {
    let dir = walk_dir(vol, path.parent().unwrap()).await?;
    let filename = path.file_name().unwrap().to_str().unwrap();
    let (node_id, _, _) = dir.lookup(filename).await?.ok_or(FxfsError::NotFound)?;
    let handle = StoreObjectHandle::new(
        vol.clone(),
        node_id,
        /* permanent_keys: */ false,
        HandleOptions::default(),
        false,
    );
    handle
        .set_extended_attribute(name.to_vec(), value.to_vec(), SetExtendedAttributeMode::Set)
        .await
}

pub async fn open_device(
    file_path: &str,
    read_only: bool,
) -> Result<(DeviceHolder, Option<tempfile::TempPath>), Error> {
    let mut file = std::fs::OpenOptions::new().read(true).write(!read_only).open(file_path)?;

    let is_sparse = match SparseReader::is_sparse_file(&mut file) {
        Ok(res) => res,
        Err(_) => {
            file.seek(std::io::SeekFrom::Start(0))?;
            false
        }
    };

    if is_sparse {
        file.seek(std::io::SeekFrom::Start(0))?;
        let (mut temp_file, temp_path) = tempfile::NamedTempFile::new()?.into_parts();
        unsparse(&mut file, &mut temp_file)?;
        let device_file =
            std::fs::OpenOptions::new().read(true).write(!read_only).open(&*temp_path)?;
        let device = DeviceHolder::new(FileBackedDevice::new(device_file, 512));
        return Ok((device, Some(temp_path)));
    }

    file.seek(std::io::SeekFrom::Start(0))?;
    let device = DeviceHolder::new(FileBackedDevice::new(file, 512));
    Ok((device, None))
}

pub fn sparsify_and_save(temp_file_path: &std::path::Path, dst_path: &str) -> Result<(), Error> {
    let parent =
        std::path::Path::new(dst_path).parent().unwrap_or_else(|| std::path::Path::new("."));
    let mut sparse_files =
        build_sparse_files("fxfs", temp_file_path.to_str().unwrap(), parent, u64::MAX)?;
    anyhow::ensure!(sparse_files.len() == 1, "Expected exactly one sparse file");
    let temp_path = sparse_files.pop().unwrap();
    temp_path.persist(dst_path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sparse::builder::{DataSource, SparseImageBuilder};
    use tempfile::NamedTempFile;

    #[test]
    fn test_open_device_sparse() {
        let mut temp_file = NamedTempFile::new().unwrap();
        SparseImageBuilder::new().add_source(DataSource::Skip(4096)).build(&mut temp_file).unwrap();

        let (_, temp_path) = temp_file.into_parts();
        futures::executor::block_on(async {
            let (_device, open_path) =
                open_device(temp_path.to_str().unwrap(), true).await.unwrap();
            assert!(open_path.is_some());
        });
    }

    #[test]
    fn test_open_device_regular() {
        let mut temp_file = NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut temp_file, &[1, 2, 3, 4]).unwrap();

        futures::executor::block_on(async {
            let (_device, temp_path) =
                open_device(temp_file.path().to_str().unwrap(), true).await.unwrap();
            assert!(temp_path.is_none());
        });
    }

    #[test]
    fn test_sparsify_and_save() {
        let mut temp_file = NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut temp_file, &[0; 8192]).unwrap();

        let dst_file = NamedTempFile::new().unwrap();
        sparsify_and_save(temp_file.path(), dst_file.path().to_str().unwrap()).unwrap();

        let mut check_file = std::fs::File::open(dst_file.path()).unwrap();
        assert!(SparseReader::is_sparse_file(&mut check_file).unwrap());
    }
}
