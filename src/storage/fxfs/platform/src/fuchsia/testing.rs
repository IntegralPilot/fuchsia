// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::fuchsia::directory::FxDirectory;
use crate::fuchsia::file::FxFile;
use crate::fuchsia::fxblob::BlobDirectory;
use crate::fuchsia::memory_pressure::MemoryPressureMonitor;
use crate::fuchsia::pager::PagerBacked;
use crate::fuchsia::volume::{FxVolumeAndRoot, MemoryPressureConfig};
use crate::fuchsia::volumes_directory::VolumesDirectory;
use anyhow::{Context, Error};
use fidl::endpoints::create_proxy;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_memorypressure::WatcherProxy;
use fuchsia_sync::Mutex;
use fxfs::filesystem::{FxFilesystem, FxFilesystemBuilder, OpenFxFilesystem};
use fxfs::fsck::errors::FsckIssue;
use fxfs::fsck::{FsckOptions, fsck_volume_with_options, fsck_with_options};
pub use fxfs::hooks::Hooks;
use fxfs::hooks::HooksHandle;
use fxfs::object_store::volume::root_volume;
use fxfs::object_store::{NewChildStoreOptions, StoreOptions};
use fxfs_crypt_common::CryptBase;
use fxfs_crypto::Crypt;
use fxfs_insecure_crypto::new_insecure_crypt;
use refaults_vmo::PageRefaultCounter;
use std::sync::{Arc, Weak};
use storage_device::DeviceHolder;
use storage_device::fake_device::FakeDevice;
use vfs::temp_clone::unblock;
use zx::{self as zx, Status};

struct State {
    filesystem: OpenFxFilesystem,
    volume: FxVolumeAndRoot,
    volume_out_dir: fio::DirectoryProxy,
    root: fio::DirectoryProxy,
    volumes_directory: Arc<VolumesDirectory>,
    mem_pressure_proxy: WatcherProxy,
}

pub struct TestFixture {
    state: Option<State>,
    encrypted: Option<Arc<CryptBase>>,
}

pub struct TestFixtureOptions {
    pub encrypted: bool,
    pub as_blob: bool,
    pub format: bool,
    pub hooks: Option<Arc<HooksHandle>>,
    pub allow_type3_blobs: bool,
}

impl Default for TestFixtureOptions {
    fn default() -> Self {
        Self {
            encrypted: true,
            as_blob: false,
            format: true,
            hooks: None,
            allow_type3_blobs: false,
        }
    }
}

fn ensure_unique_or_poison(holder: DeviceHolder) -> DeviceHolder {
    if Arc::strong_count(&*holder) > 1 {
        // All old references should be dropped by now, but they aren't. So we're going to try
        // to crash that thread to get a stack of who is holding on to it. This is risky, and
        // might still just crash in this thread, but it's worth a try.
        if (*holder).poison().is_err() {
            // Can't poison it unless it is a FakeDevice.
            panic!("Remaining reference to device that doesn't support poison.");
        };

        // Dropping all the local references. May crash due to the poison if the extra reference was
        // cleaned up since the last check.
        std::mem::drop(holder);

        // We've successfully poisoned the device for Drop. Now we wait and hope that the dangling
        // reference isn't in a thread that is totally hung.
        std::thread::sleep(std::time::Duration::from_secs(5));
        panic!("Timed out waiting for poison to trigger.");
    }
    holder
}

impl TestFixture {
    pub async fn new() -> Self {
        Self::open(DeviceHolder::new(FakeDevice::new(16384, 512)), TestFixtureOptions::default())
            .await
    }

    pub async fn new_with_device(device: DeviceHolder) -> Self {
        Self::open(device, TestFixtureOptions { format: false, ..Default::default() }).await
    }

    pub async fn new_unencrypted() -> Self {
        Self::open(
            DeviceHolder::new(FakeDevice::new(16384, 512)),
            TestFixtureOptions { encrypted: false, ..Default::default() },
        )
        .await
    }

    pub async fn open(device: DeviceHolder, options: TestFixtureOptions) -> Self {
        let crypt: Arc<CryptBase> = Arc::new(new_insecure_crypt());
        let (mem_pressure_proxy, watcher_server) = create_proxy();
        let mem_pressure = MemoryPressureMonitor::try_from(watcher_server)
            .expect("Failed to create MemoryPressureMonitor");

        let blob_resupplied_count =
            Arc::new(PageRefaultCounter::new().expect("Failed to create PageRefaultCounter"));
        let volume_name = if options.as_blob { "blob" } else { "vol" };
        let (filesystem, volume, volumes_directory) = if options.format {
            let mut builder = FxFilesystemBuilder::new()
                .format(true)
                .allow_type3_blobs(options.allow_type3_blobs);
            if let Some(hooks) = options.hooks {
                builder = builder.hooks(hooks);
            }
            let filesystem = builder.open(device).await.unwrap();
            let root_volume = root_volume(filesystem.clone()).await.unwrap();
            let store = root_volume
                .new_volume(
                    volume_name,
                    NewChildStoreOptions {
                        options: StoreOptions {
                            crypt: if options.encrypted { Some(crypt.clone()) } else { None },
                            ..StoreOptions::default()
                        },
                        ..NewChildStoreOptions::default()
                    },
                )
                .await
                .unwrap();
            let store_object_id = store.store_object_id();

            let volumes_directory = VolumesDirectory::new(
                root_volume,
                Weak::new(),
                Some(mem_pressure),
                blob_resupplied_count.clone(),
                MemoryPressureConfig::default(),
            )
            .await
            .unwrap();
            let vol = if options.as_blob {
                FxVolumeAndRoot::new::<BlobDirectory>(
                    Arc::downgrade(&volumes_directory),
                    store,
                    store_object_id,
                    volume_name.to_owned(),
                    blob_resupplied_count.clone(),
                    *volumes_directory.memory_pressure_config(),
                )
                .await
                .unwrap()
            } else {
                FxVolumeAndRoot::new::<FxDirectory>(
                    Arc::downgrade(&volumes_directory),
                    store,
                    store_object_id,
                    volume_name.to_owned(),
                    blob_resupplied_count.clone(),
                    *volumes_directory.memory_pressure_config(),
                )
                .await
                .unwrap()
            };
            (filesystem, vol, volumes_directory)
        } else {
            let filesystem = FxFilesystemBuilder::new()
                .allow_type3_blobs(options.allow_type3_blobs)
                .open(device)
                .await
                .unwrap();
            let root_volume = root_volume(filesystem.clone()).await.unwrap();
            let store = root_volume
                .volume(
                    volume_name,
                    StoreOptions {
                        crypt: if options.encrypted { Some(crypt.clone()) } else { None },
                        ..StoreOptions::default()
                    },
                )
                .await
                .unwrap();
            let store_object_id = store.store_object_id();
            let volumes_directory = VolumesDirectory::new(
                root_volume,
                Weak::new(),
                Some(mem_pressure),
                blob_resupplied_count.clone(),
                MemoryPressureConfig::default(),
            )
            .await
            .unwrap();
            let vol = if options.as_blob {
                FxVolumeAndRoot::new::<BlobDirectory>(
                    Arc::downgrade(&volumes_directory),
                    store,
                    store_object_id,
                    volume_name.to_owned(),
                    blob_resupplied_count.clone(),
                    *volumes_directory.memory_pressure_config(),
                )
                .await
                .unwrap()
            } else {
                FxVolumeAndRoot::new::<FxDirectory>(
                    Arc::downgrade(&volumes_directory),
                    store,
                    store_object_id,
                    volume_name.to_owned(),
                    blob_resupplied_count.clone(),
                    *volumes_directory.memory_pressure_config(),
                )
                .await
                .unwrap()
            };

            (filesystem, vol, volumes_directory)
        };

        let (root, server_end) = create_proxy::<fio::DirectoryMarker>();
        volume.root().clone().serve(fio::PERM_READABLE | fio::PERM_WRITABLE, server_end);

        let (volume_out_dir, server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        volumes_directory.lock().await.add_mount(volume_name, &volume);
        volumes_directory
            .serve_volume(&volume, server_end, options.as_blob)
            .expect("serve_volume failed");

        let encrypted = if options.encrypted { Some(crypt.clone()) } else { None };
        Self {
            state: Some(State {
                filesystem,
                volume,
                volume_out_dir,
                root,
                volumes_directory,
                mem_pressure_proxy,
            }),
            encrypted,
        }
    }

    /// Closes the test fixture, shutting down the filesystem. Returns the device, which can be
    /// reused for another TestFixture.
    ///
    /// Ensures that:
    ///   * The filesystem shuts down cleanly.
    ///   * fsck passes.
    ///   * There are no dangling references to the device or the volume.
    pub async fn close(mut self) -> DeviceHolder {
        let State { filesystem, volume, volume_out_dir, root, volumes_directory, .. } =
            std::mem::take(&mut self.state).unwrap();
        volume_out_dir
            .close()
            .await
            .expect("FIDL call failed")
            .map_err(Status::err_from_raw)
            .expect("close out_dir failed");
        // Close the root node and ensure that there's no remaining references to |vol|, which would
        // indicate a reference cycle or other leak.
        root.close()
            .await
            .expect("FIDL call failed")
            .map_err(Status::err_from_raw)
            .expect("close root failed");

        // This should terminate all volumes.  This should ensure that there are no other references
        // to the volume (which can be associated with connections that we have not yet noticed are
        // closed).  This will then allow `FxVolume::try_unwrap()` to work below.
        volumes_directory.terminate().await;
        drop(volumes_directory);

        let store_id = volume.volume().store().store_object_id();

        if volume.into_volume().try_unwrap().is_none() {
            log::error!("References to volume still exist; hanging");
            let () = std::future::pending().await;
        }

        // We have to reopen the filesystem briefly to fsck it. (We could fsck before closing, but
        // there might be pending operations that go through after fsck but before we close the
        // filesystem, and we want to be sure that we catch all possible issues with fsck.)
        filesystem.close().await.expect("close filesystem failed");
        let device = ensure_unique_or_poison(filesystem.take_device().await);
        device.reopen(false);
        let filesystem = FxFilesystem::open(device).await.expect("open failed");
        let options = FsckOptions {
            fail_on_warning: true,
            on_error: Box::new(|err: &FsckIssue| {
                eprintln!("Fsck error: {:?}", err);
            }),
            ..Default::default()
        };
        fsck_with_options(filesystem.clone(), &options).await.expect("fsck failed");
        let encrypted = if let Some(crypt) = &self.encrypted {
            Some(crypt.clone() as Arc<dyn Crypt>)
        } else {
            None
        };
        fsck_volume_with_options(filesystem.as_ref(), &options, store_id, encrypted)
            .await
            .expect("fsck_volume failed");

        filesystem.close().await.expect("close filesystem failed");
        let device = ensure_unique_or_poison(filesystem.take_device().await);
        device.reopen(false);

        device
    }

    pub fn root(&self) -> &fio::DirectoryProxy {
        &self.state.as_ref().unwrap().root
    }

    pub fn crypt(&self) -> Option<Arc<CryptBase>> {
        self.encrypted.clone()
    }

    pub fn fs(&self) -> &Arc<FxFilesystem> {
        &self.state.as_ref().unwrap().filesystem
    }

    pub fn volume(&self) -> &FxVolumeAndRoot {
        &self.state.as_ref().unwrap().volume
    }

    pub fn volumes_directory(&self) -> &Arc<VolumesDirectory> {
        &self.state.as_ref().unwrap().volumes_directory
    }

    pub fn volume_out_dir(&self) -> &fio::DirectoryProxy {
        &self.state.as_ref().unwrap().volume_out_dir
    }

    pub fn memory_pressure_proxy(&self) -> &WatcherProxy {
        &self.state.as_ref().unwrap().mem_pressure_proxy
    }
}

pub struct TestCallback(Mutex<Option<Arc<dyn Fn() + Send + Sync>>>);
pub struct TestCallbackGuard(&'static TestCallback);

impl Drop for TestCallbackGuard {
    fn drop(&mut self) {
        *self.0.0.lock() = None;
    }
}

impl TestCallback {
    pub const fn new() -> Self {
        Self(Mutex::new(None))
    }

    /// Returns a guard that invalidates this callback and releases the resources when dropped.
    pub fn set<F>(&'static self, callback: F) -> TestCallbackGuard
    where
        F: Fn() + Send + Sync + 'static,
    {
        let arc: Arc<dyn Fn() + Send + Sync> = Arc::new(callback);
        {
            let mut inner = self.0.lock();
            assert!(inner.is_none(), "Resetting TestCallback without dropping old guard.");
            *inner = Some(arc.clone());
        }
        TestCallbackGuard(&self)
    }

    pub fn call(&self) {
        let cb = self.0.lock().as_ref().map(|cb| cb.clone());
        // Call the callback outside the lock. This isn't really a race though, since just calling
        // the callback doesn't ensure that any action will actually be done inside it, and this
        // delay to calling the callback is impossible to differentiate from that delay.
        if let Some(cb) = cb {
            cb();
        }
    }
}

impl Drop for TestFixture {
    fn drop(&mut self) {
        assert!(self.state.is_none(), "Did you forget to call TestFixture::close?");
    }
}

pub async fn close_file_checked(file: fio::FileProxy) {
    file.sync()
        .await
        .expect("FIDL call failed")
        .map_err(Status::err_from_raw)
        .expect("sync failed");
    file.close()
        .await
        .expect("FIDL call failed")
        .map_err(Status::err_from_raw)
        .expect("close failed");
}

pub async fn close_dir_checked(dir: fio::DirectoryProxy) {
    dir.close()
        .await
        .expect("FIDL call failed")
        .map_err(Status::err_from_raw)
        .expect("close failed");
}

// Utility function to open a new node connection under |dir| using open.
pub async fn open_file(
    dir: &fio::DirectoryProxy,
    path: &str,
    flags: fio::Flags,
    options: &fio::Options,
) -> Result<fio::FileProxy, Error> {
    let (proxy, server_end) = create_proxy::<fio::FileMarker>();
    dir.open(path, flags | fio::Flags::PROTOCOL_FILE, options, server_end.into_channel())?;
    let _: Vec<_> = proxy.query().await?;
    Ok(proxy)
}

// Like |open_file|, but asserts if the open call fails.
pub async fn open_file_checked(
    dir: &fio::DirectoryProxy,
    path: &str,
    flags: fio::Flags,
    options: &fio::Options,
) -> fio::FileProxy {
    open_file(dir, path, flags, options).await.expect("open_file failed")
}

// Utility function to open a new node connection under |dir|.
pub async fn open_dir(
    dir: &fio::DirectoryProxy,
    path: &str,
    flags: fio::Flags,
    options: &fio::Options,
) -> Result<fio::DirectoryProxy, Error> {
    let (proxy, server_end) = create_proxy::<fio::DirectoryMarker>();
    dir.open(path, flags | fio::Flags::PROTOCOL_DIRECTORY, options, server_end.into_channel())?;
    let _: Vec<_> = proxy.query().await?;
    Ok(proxy)
}

// Like |open_dir|, but asserts if the open call fails.
pub async fn open_dir_checked(
    dir: &fio::DirectoryProxy,
    path: &str,
    flags: fio::Flags,
    options: fio::Options,
) -> fio::DirectoryProxy {
    open_dir(dir, path, flags, &options).await.expect("open_dir failed")
}

/// Utility function to write to an `FxFile`.
pub async fn write_at(file: &FxFile, offset: u64, content: &[u8]) -> Result<usize, Error> {
    let stream = zx::Stream::create(zx::StreamOptions::MODE_WRITE, file.vmo(), 0)
        .context("stream create failed")?;
    let content = content.to_vec();
    unblock(move || {
        stream
            .write_at(zx::StreamWriteOptions::empty(), offset, &content)
            .context("stream write failed")
    })
    .await
}
