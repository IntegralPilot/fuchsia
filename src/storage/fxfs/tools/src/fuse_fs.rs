// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::fuse_attr::{create_dir_attr, create_file_attr, create_symlink_attr};
use crate::fuse_errors::FxfsResult;
use event_listener::{Event, Listener as _};
use fuse3::raw::prelude::*;
use fxfs::errors::FxfsError;
use fxfs::filesystem::{FxFilesystem, OpenFxFilesystem};
use fxfs::log::info;
use fxfs::object_handle::ObjectProperties;
use fxfs::object_store::volume::root_volume;
use fxfs::object_store::{
    DataObjectHandle, Directory, HandleOptions, NewChildStoreOptions, ObjectDescriptor, ObjectKey,
    ObjectKind, ObjectStore, ObjectValue, StoreOptions, Timestamp,
};
use fxfs_crypto::Crypt;
use rustc_hash::FxHashMap as HashMap;
use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::path::PathBuf;
use std::process;
use std::sync::{Arc, OnceLock};
use storage_device::DeviceHolder;
use storage_device::fake_device::FakeDevice;
use storage_device::file_backed_device::FileBackedDevice;
use tokio::sync::RwLock;

const DEFAULT_DEVICE_BLOCK_SIZE: u32 = 512;
const DEFAULT_DEVICE_SIZE: u64 = 1024u64.pow(4);

/// FUSE assumes that the root directory has an object id of 1,
/// which needs to be casted to be correct root directory id in Fxfs.
const DEFAULT_FUSE_ROOT: u64 = 1;
const DEFAULT_VOLUME_NAME: &str = "fxfs";

/// In-memory fake devices are used for unit tests.
const IN_MEMORY_DEVICE_BLOCK_COUNT: u64 = 8192;

/// CLOSE_EVENT listens to the signals from user to gracefully close the filesystem.
static CLOSE_EVENT: OnceLock<Event> = OnceLock::new();

fn register_signal_handlers() {
    unsafe {
        libc::signal(libc::SIGTERM, handle_sigterm as *const () as usize);
        // Listen to SIGINT to gracefully close the filesystem with Ctrl+C.
        libc::signal(libc::SIGINT, handle_sigterm as *const () as usize);
    }
}

/// Handler for the kernel signal.
extern "C" fn handle_sigterm(_signal: i32) {
    register_signal_handlers();
    info!("Kernel signal received");
    CLOSE_EVENT.get().unwrap().notify(usize::MAX);
}

pub struct FuseFs {
    pub fs: OpenFxFilesystem,
    pub default_store: Arc<ObjectStore>,
    pub mount_path: String,
    // Each entry in object_handle_cache stores an object_handle and a counter that is incremented
    // on its open/create and decremented on its release.
    pub object_handle_cache: Arc<RwLock<HashMap<u64, (Arc<DataObjectHandle<ObjectStore>>, u32)>>>,
}

impl FuseFs {
    /// Initialize the filesystem with a fake in-memory device.
    pub async fn new_in_memory(mount_path: String) -> Self {
        let device = DeviceHolder::new(FakeDevice::new(
            IN_MEMORY_DEVICE_BLOCK_COUNT,
            DEFAULT_DEVICE_BLOCK_SIZE,
        ));
        let crypt = None;
        FuseFs::new(device, crypt, mount_path).await
    }

    /// Initialize the filesystem by creating a new file-backed device.
    pub async fn new_file_backed(device_path: &str, mount_path: String) -> Self {
        let file = FuseFs::create_file(device_path);
        let device = DeviceHolder::new(FileBackedDevice::new(file, DEFAULT_DEVICE_BLOCK_SIZE));
        let crypt = None;
        FuseFs::new(device, crypt, mount_path).await
    }

    /// Initialize the filesystem by opening an existing file-backed device.
    pub async fn open_file_backed(device_path: &str, mount_path: String) -> Self {
        let file = FuseFs::create_file(device_path);
        let device = DeviceHolder::new(FileBackedDevice::new(file, DEFAULT_DEVICE_BLOCK_SIZE));
        let crypt = None;
        FuseFs::open(device, crypt, mount_path).await
    }

    /// Create a file on disk that is used for file-backed device.
    pub fn create_file(path: &str) -> File {
        let path_buf = PathBuf::from(path);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(path_buf.as_path())
            .unwrap_or_else(|e| panic!("failed to create {:?}: {:?}", path_buf.as_path(), e));
        file.set_len(DEFAULT_DEVICE_SIZE).expect("failed to truncate file");
        file
    }

    /// Create a new FxFilesystem using given device and cryption, with a root volume initialized.
    pub async fn new(
        device: DeviceHolder,
        crypt: Option<Arc<dyn Crypt>>,
        mount_path: String,
    ) -> Self {
        let fs = FxFilesystem::new_empty(device).await.expect("fxfs new_empty failed");
        let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
        root_volume
            .new_volume(
                DEFAULT_VOLUME_NAME,
                NewChildStoreOptions {
                    options: StoreOptions { crypt: crypt.clone(), ..StoreOptions::default() },
                    ..NewChildStoreOptions::default()
                },
            )
            .await
            .expect("new_volume failed");
        let default_store = root_volume
            .volume(
                DEFAULT_VOLUME_NAME,
                StoreOptions { crypt: crypt.clone(), ..StoreOptions::default() },
            )
            .await
            .expect("failed to open default store");

        Self {
            fs,
            default_store,
            mount_path,
            object_handle_cache: Arc::new(RwLock::new(HashMap::default())),
        }
    }

    /// Open an existing FxFilesystem using given device and cryption
    pub async fn open(
        device: DeviceHolder,
        crypt: Option<Arc<dyn Crypt>>,
        mount_path: String,
    ) -> Self {
        let fs = FxFilesystem::open(device).await.expect("new_empty failed");
        let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");
        let default_store = root_volume
            .volume(
                DEFAULT_VOLUME_NAME,
                StoreOptions { crypt: crypt.clone(), ..StoreOptions::default() },
            )
            .await
            .expect("failed to open default store");

        Self {
            fs,
            default_store,
            mount_path,
            object_handle_cache: Arc::new(RwLock::new(HashMap::default())),
        }
    }

    /// Listen to the signals to gracefully close the filesystem.
    pub async fn notify_destroy(&self) -> tokio::task::JoinHandle<()> {
        register_signal_handlers();
        CLOSE_EVENT.set(Event::new()).unwrap();
        let fs_clone = self.fs.clone();
        let mount_path = self.mount_path.clone();

        let handle = tokio::spawn(async move {
            let listener = CLOSE_EVENT.get().unwrap().listen();
            listener.wait();
            fs_clone.close().await.expect("Close failed");
            info!("FUSE-Fxfs filesystem was gracefully closed");

            info!("Trying to unmount directory: {:?} ...", mount_path.as_str());
            process::Command::new("umount").args([mount_path.as_str()]).output().unwrap_or_else(
                |e| panic!("failed to unmount directory {:?}: {:?}", mount_path.as_str(), e),
            );
            info!("{:?} is successfully unmounted", mount_path);

            process::exit(0);
        });
        handle
    }

    /// FUSE assumes that inode 1 is the root of a filesystem, which needs
    /// to be changed to the correct root directory object id of Fxfs.
    pub fn fuse_inode_to_object_id(&self, inode: u64) -> u64 {
        match inode {
            DEFAULT_FUSE_ROOT => self.default_store.root_directory_object_id(),
            _ => inode,
        }
    }

    /// Get the object store of Fxfs's root directory.
    pub async fn root_dir(&self) -> FxfsResult<Directory<ObjectStore>> {
        Directory::open(&self.default_store, self.default_store.root_directory_object_id()).await
    }

    /// Open the directory with specified object id.
    pub async fn open_dir(&self, object_id: u64) -> FxfsResult<Directory<ObjectStore>> {
        Directory::open(&self.default_store, object_id).await
    }

    /// Get the handle of an object with specified id.
    pub async fn get_object_handle(
        &self,
        object_id: u64,
    ) -> FxfsResult<Arc<DataObjectHandle<ObjectStore>>> {
        {
            // Check in the cache if the handle of requested object exists.
            // Functions that have a matching create/open call should have the object handle in cache.
            if let Some((object_handle, 0)) = self.object_handle_cache.read().await.get(&object_id)
            {
                info!("Cache hit for handle of object {}", object_id);
                return Ok(object_handle.clone());
            }
        }

        // If object handle is not in cache, then it is called by functions that do not have a matching
        // create/open call. Hence do not store the handle in cache.
        let handle = Arc::new(
            ObjectStore::open_object(
                &self.default_store,
                object_id,
                HandleOptions::default(),
                None,
            )
            .await?,
        );

        Ok(handle)
    }

    /// Load the handle of an object with specified id into cache.
    /// Increase the cache counter by 1 if the handle is already in cache.
    pub async fn load_object_handle(
        &self,
        object_id: u64,
    ) -> FxfsResult<Arc<DataObjectHandle<ObjectStore>>> {
        let mut object_handle_cache = self.object_handle_cache.write().await;

        if let Some((handle, counter)) = object_handle_cache.get_mut(&object_id) {
            *counter += 1;
            Ok(handle.clone())
        } else {
            let handle = Arc::new(
                ObjectStore::open_object(
                    &self.default_store,
                    object_id,
                    HandleOptions::default(),
                    None,
                )
                .await?,
            );

            object_handle_cache.insert(object_id, (handle.clone(), 1));

            Ok(handle)
        }
    }

    /// Reduce the counter of object handle with specified id in cache by 1.
    /// Remove the object from cache if the counter reaches 0.
    pub async fn release_object_handle(&self, object_id: u64) {
        let mut object_handle_cache = self.object_handle_cache.write().await;

        if let Some((_, counter)) = object_handle_cache.get_mut(&object_id) {
            if *counter == 1 {
                object_handle_cache.remove(&object_id);
            } else {
                *counter -= 1;
            }
        } else {
            panic!(
                "release does not have a matching open/create: {} not found in cache.",
                object_id
            );
        }
    }

    /// Get the properties of an object with specified id.
    pub async fn get_object_properties(
        &self,
        object_id: u64,
        object_type: ObjectDescriptor,
    ) -> FxfsResult<ObjectProperties> {
        if object_type == ObjectDescriptor::File {
            let handle = self.get_object_handle(object_id).await?;
            handle.get_properties().await
        } else if object_type == ObjectDescriptor::Directory {
            let dir = self.open_dir(object_id).await?;
            dir.get_properties().await
        } else {
            panic!("Invalid input object type");
        }
    }

    /// Get the type of an object with specified id.
    pub async fn get_object_type(&self, object_id: u64) -> FxfsResult<ObjectDescriptor> {
        {
            // If the requested object exists in the handle cache, then it must be a file.
            if self.object_handle_cache.clone().read().await.contains_key(&object_id) {
                info!("Cache hit for handle of object {}", object_id);
                return Ok(ObjectDescriptor::File);
            }
        }

        self.default_store
            .tree()
            .find_map(&ObjectKey::object(object_id), |item| match item.value {
                ObjectValue::Object { kind, .. } => match kind {
                    ObjectKind::Directory { .. } => Ok(ObjectDescriptor::Directory),
                    ObjectKind::File { .. } => Ok(ObjectDescriptor::File),
                    ObjectKind::Symlink { .. } => Ok(ObjectDescriptor::Symlink),
                    _ => Err(FxfsError::Inconsistent.into()),
                },
                _ => Err(FxfsError::Inconsistent.into()),
            })
            .await?
            .ok_or(FxfsError::NotFound)?
    }

    /// Create the FUSE-style attribute for an object based on its type.
    pub async fn create_object_attr(
        &self,
        object_id: u64,
        object_type: ObjectDescriptor,
    ) -> FxfsResult<FileAttr> {
        match object_type {
            ObjectDescriptor::Directory => {
                let properties = self.get_object_properties(object_id, object_type.clone()).await?;
                Ok(create_dir_attr(
                    object_id,
                    properties.data_attribute_size,
                    properties.creation_time,
                    properties.modification_time,
                ))
            }
            ObjectDescriptor::File => {
                let properties = self.get_object_properties(object_id, object_type.clone()).await?;
                Ok(create_file_attr(
                    object_id,
                    properties.data_attribute_size,
                    properties.creation_time,
                    properties.modification_time,
                    properties.refs as u32,
                ))
            }
            // Volume attributes are treated as directory attributes.
            ObjectDescriptor::Volume => {
                let properties = self.get_object_properties(object_id, object_type.clone()).await?;
                Ok(create_dir_attr(
                    object_id,
                    properties.data_attribute_size,
                    properties.creation_time,
                    properties.modification_time,
                ))
            }
            ObjectDescriptor::Symlink => {
                Ok(create_symlink_attr(object_id, Timestamp::now(), Timestamp::now()))
            }
        }
    }
}

pub trait FuseStrParser {
    fn osstr_to_str(&self) -> FxfsResult<&str>;
}

impl FuseStrParser for OsStr {
    /// Convert from OsStr to str.
    fn osstr_to_str(&self) -> FxfsResult<&str> {
        if let Some(s) = self.to_str() { Ok(s) } else { Err(FxfsError::InvalidArgs.into()) }
    }
}

#[cfg(test)]
mod tests {
    use crate::fuse_fs::FuseFs;
    use fxfs::object_handle::ObjectHandle;
    use fxfs::object_store::transaction::{LockKey, Options, lock_keys};

    /// Load object handle for three times, then continuously release the object handle.
    /// Check the handle counter value after each release.
    #[fuchsia::test]
    async fn test_load_object_handle_and_release_object_handle() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");
        let mut transaction = fs
            .fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(fs.default_store.store_object_id(), dir.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let file =
            dir.create_child_file(&mut transaction, "foo").await.expect("create_child_file failed");
        transaction.commit().await.expect("transaction commit failed");
        let object_id = file.object_id();

        fs.load_object_handle(object_id).await.expect("load_object_handle failed");
        fs.load_object_handle(object_id).await.expect("load_object_handle failed");
        fs.load_object_handle(object_id).await.expect("load_object_handle failed");
        {
            let cache = fs.object_handle_cache.read().await;
            let handle = cache.get(&object_id);
            assert!(handle.is_some());
            assert_eq!(handle.unwrap().1, 3);
        }

        fs.release_object_handle(object_id).await;
        fs.release_object_handle(object_id).await;
        {
            let cache = fs.object_handle_cache.read().await;
            let handle = cache.get(&object_id);
            assert!(handle.is_some());
            assert_eq!(handle.unwrap().1, 1);
        }

        fs.release_object_handle(object_id).await;
        {
            let cache = fs.object_handle_cache.read().await;
            let handle = cache.get(&object_id);
            assert!(handle.is_none());
        }

        fs.fs.close().await.expect("failed to close filesystem");
    }
}
