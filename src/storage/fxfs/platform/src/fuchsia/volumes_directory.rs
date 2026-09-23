// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::fuchsia::RemoteCrypt;
use crate::fuchsia::component::map_to_raw_status;
use crate::fuchsia::directory::FxDirectory;
use crate::fuchsia::errors::map_to_status;
use crate::fuchsia::fxblob::BlobDirectory;
use crate::fuchsia::memory_pressure::{MemoryPressureLevel, MemoryPressureMonitor};
use crate::fuchsia::profile::new_profile_state;
use crate::fuchsia::volume::{FxVolume, FxVolumeAndRoot, MemoryPressureConfig, RootDir};
use anyhow::{Context, Error, anyhow, ensure};
use fidl::endpoints::{DiscoverableProtocolMarker, ServerEnd};
use fidl_fuchsia_fs::{AdminMarker, AdminRequest, AdminRequestStream};
use fidl_fuchsia_fs_startup::{
    CheckOptions, CreateOptions, MountOptions, VolumeRequest, VolumeRequestStream,
};
use fidl_fuchsia_fxfs::{DebugMarker, FileBackedVolumeProviderMarker, ProjectIdMarker};
use fidl_fuchsia_io as fio;
use fs_inspect::{FsInspectTree, FsInspectVolume};
use fuchsia_async as fasync;
use futures::stream::FuturesUnordered;
use futures::{StreamExt, TryStreamExt};
use fxfs::errors::FxfsError;
use fxfs::filesystem::FxFilesystem;
use fxfs::fsck;
use fxfs::log::*;
use fxfs::object_store::transaction::{LockKey, Options, lock_keys};
use fxfs::object_store::volume::RootVolume;
use fxfs::object_store::{
    Directory, NewChildStoreOptions, ObjectDescriptor, ObjectStore, StoreOptions,
};
use fxfs_crypto::Crypt;
use fxfs_trace::{TraceFutureExt, trace_future_args};
use refaults_vmo::PageRefaultCounter;
use rustc_hash::FxHashMap as HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, Weak};
use vfs::directory::entry_container::MutableDirectory;
use vfs::directory::helper::DirectlyMutable;
const MEBIBYTE: u64 = 1024 * 1024;

struct ProfileState {
    task: fasync::Task<()>,
    profile_name: String,
    all_volumes: bool,
}

/// VolumesDirectory is a special pseudo-directory used to enumerate and operate on volumes.
/// Volume creation happens via fuchsia.fs.startup.Volumes.Create, rather than open.
///
/// Note that VolumesDirectory assumes exclusive access to |root_volume| and if volumes are
/// manipulated from elsewhere, strange things will happen.
pub struct VolumesDirectory {
    root_volume: RootVolume,
    directory_node: Arc<vfs::directory::immutable::Simple>,
    mounted_volumes: futures::lock::Mutex<HashMap<u64, MountedVolume>>,
    inspect_tree: Weak<FsInspectTree>,
    mem_monitor: Option<MemoryPressureMonitor>,
    blob_resupplied_count: Arc<PageRefaultCounter>,
    // The state of profile recordings. Should be locked *after* mounted_volumes.
    profiling_state: futures::lock::Mutex<Option<ProfileState>>,

    /// A running estimate of the number of dirty bytes outstanding in all pager-backed VMOs across
    /// all volumes.
    pager_dirty_bytes_count: PagerDirtyByteCount,

    /// Max outstanding dirty bytes under critical memory pressure. This could be hardcoded, but is
    /// broken out for testing.
    max_dirty_bytes_when_critical: AtomicU64,

    // A callback to invoke when a volume is added.  When the volume is removed, this is called
    // again with `None` as the second parameter.
    on_volume_added:
        OnceLock<Box<dyn Fn(&str, Option<(Arc<FxVolume>, Arc<ObjectStore>)>) + Send + Sync>>,

    /// The cache configuration to use under different memory pressure levels.
    memory_pressure_config: MemoryPressureConfig,
}

/// Operations on VolumesDirectory that cannot be performed concurrently (i.e. most
/// volume creation/removal ops) should exist on this guard instead of VolumesDirectory.
pub struct MountedVolumesGuard<'a> {
    volumes_directory: Arc<VolumesDirectory>,
    mounted_volumes: futures::lock::MutexGuard<'a, HashMap<u64, MountedVolume>>,
}

struct MountedVolume {
    sequence: u64,
    volume: FxVolumeAndRoot,

    // True if the volume was forcibly locked.
    locked: bool,
}

pub(crate) enum Mode {
    Mount,
    Create { guid: Option<[u8; 16]>, low_32_bit_object_ids: bool },
}

impl MountedVolumesGuard<'_> {
    /// Creates or mounts a volume. If |crypt| is set, the volume will be created or mounted as
    /// encrypted. The volume is mounted according to |as_blob|.
    async fn create_or_mount_volume(
        &mut self,
        name: &str,
        crypt: Option<Arc<dyn Crypt>>,
        mode: Mode,
        as_blob: bool,
    ) -> Result<FxVolumeAndRoot, Error> {
        let store = match mode {
            Mode::Create { guid, low_32_bit_object_ids } => self
                .volumes_directory
                .root_volume
                .new_volume(
                    name,
                    NewChildStoreOptions {
                        options: StoreOptions { crypt },
                        guid,
                        low_32_bit_object_ids,
                        ..Default::default()
                    },
                )
                .await
                .context("failed to create new volume")?,
            Mode::Mount => {
                self.volumes_directory.root_volume.volume(name, StoreOptions { crypt }).await?
            }
        };
        ensure!(
            !self.mounted_volumes.contains_key(&store.store_object_id()),
            FxfsError::AlreadyBound
        );

        let volume = if as_blob {
            self.mount_store::<BlobDirectory>(
                name,
                store,
                self.volumes_directory.memory_pressure_config,
            )
            .await?
        } else {
            self.mount_store::<FxDirectory>(
                name,
                store,
                self.volumes_directory.memory_pressure_config,
            )
            .await?
        };
        // If there is an ongoing profile activity, we should apply it to the mounted volume.
        if let Some(ProfileState { profile_name, all_volumes: true, .. }) =
            &(*self.volumes_directory.profiling_state.lock().await)
        {
            if let Err(e) = volume
                .volume()
                .record_and_replay_profile(new_profile_state(as_blob), profile_name)
                .await
            {
                error!(
                    "Failed to record or replay profile '{}' for volume {}: {:?}",
                    profile_name, name, e
                );
            }
        }

        if let Mode::Create { .. } = mode {
            let store_object_id = volume.volume().store().store_object_id();
            self.volumes_directory.add_directory_entry(name, store_object_id);
        }
        Ok(volume)
    }

    /// Returns the volume if it is found and unlocked, along with a bool to indicate that it is or
    /// isn't a blob volume.
    async fn get_unlocked_volume_by_name(
        &self,
        volume_name: &str,
    ) -> Result<(FxVolumeAndRoot, bool), zx::Status> {
        let (store_object_id, _, _) = self
            .volumes_directory
            .root_volume
            .volume_directory()
            .lookup(volume_name)
            .await
            .map_err(map_to_status)?
            .ok_or(zx::Status::NOT_FOUND)?;
        if let Some(MountedVolume { volume, .. }) = self.mounted_volumes.get(&store_object_id) {
            let is_blob = volume.root().clone().into_any().downcast::<BlobDirectory>().is_ok();
            Ok((volume.clone(), is_blob))
        } else {
            Err(zx::Status::UNAVAILABLE)
        }
    }

    // Verifies the integrity of `name` if it exists.  Returns Ok otherwise.
    async fn check_volume(
        &mut self,
        filesystem: &FxFilesystem,
        name: &str,
        crypt: Option<Arc<dyn Crypt>>,
    ) -> Result<Option<Arc<ObjectStore>>, Error> {
        if let Ok(store) = self
            .volumes_directory
            .root_volume
            .volume(name, StoreOptions { crypt: crypt.clone() })
            .await
        {
            fsck::fsck_volume(filesystem, store.store_object_id(), crypt).await?;
            Ok(Some(store))
        } else {
            Ok(None)
        }
    }

    // Mounts the given store.  A lock *must* be held on the volume directory.
    async fn mount_store<T: From<Directory<FxVolume>> + RootDir>(
        &mut self,
        name: &str,
        store: Arc<ObjectStore>,
        flush_task_config: MemoryPressureConfig,
    ) -> Result<FxVolumeAndRoot, Error> {
        let unique_id = zx::Event::create();
        let volume = FxVolumeAndRoot::new::<T>(
            Arc::downgrade(&self.volumes_directory),
            store,
            unique_id.koid().unwrap().raw_koid(),
            name.to_owned(),
            self.volumes_directory.blob_resupplied_count.clone(),
            self.volumes_directory.memory_pressure_config,
        )
        .await?;
        volume
            .volume()
            .start_background_task(flush_task_config, self.volumes_directory.mem_monitor.as_ref());
        self.add_mount(name, &volume);
        Ok(volume)
    }

    /// Adds a volume (`FxVolumeAndRoot`) into the mount list.
    pub fn add_mount(&mut self, name: &str, volume: &FxVolumeAndRoot) {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        self.mounted_volumes.insert(
            volume.volume().store().store_object_id(),
            MountedVolume { sequence, volume: volume.clone(), locked: false },
        );
        if let Some(inspect) = self.volumes_directory.inspect_tree.upgrade() {
            inspect.register_volume(
                name.to_string(),
                Arc::downgrade(volume.volume()) as Weak<dyn FsInspectVolume + Send + Sync>,
            )
        }
        if let Some(callback) = self.volumes_directory.on_volume_added.get() {
            callback(
                name,
                Some((
                    volume.volume().clone(),
                    self.volumes_directory
                        .root_volume
                        .volume_directory()
                        .store()
                        .filesystem()
                        .root_store(),
                )),
            );
        }
    }

    async fn lock_mount(&self, mounted_volume: &mut MountedVolume) {
        let MountedVolume { volume, locked, .. } = mounted_volume;

        if let Some(callback) = self.volumes_directory.on_volume_added.get() {
            callback(&volume.volume().name(), None);
        }
        if !*locked {
            if let Some(inspect) = self.volumes_directory.inspect_tree.upgrade() {
                inspect.unregister_volume(volume.volume().name());
            }
            // We must make sure to remove the root entry which holds strong references to the
            // volume since otherwise `Volume::try_unwrap` might fail.
            let _ = volume.outgoing_dir().remove_entry("root", true);
            volume.volume().terminate().await;
            *locked = true;
        }
    }

    async fn remove_volume(&mut self, name: &str) -> Result<(), Error> {
        let (object_id, transaction) = self
            .volumes_directory
            .root_volume
            .acquire_transaction_for_remove_volume(name, [], false)
            .await?;

        // Cowardly refuse to delete a mounted volume.
        ensure!(!self.mounted_volumes.contains_key(&object_id), FxfsError::AlreadyBound);
        let directory_node = self.volumes_directory.directory_node.clone();
        self.volumes_directory
            .root_volume
            .delete_volume(name, transaction, || {
                // This shouldn't fail because the entry should exist.
                directory_node.remove_entry(name, /* must_be_directory: */ false).unwrap();
            })
            .await?;
        Ok(())
    }

    async fn terminate(&mut self) {
        let mut volumes = std::mem::take(&mut *self.mounted_volumes);
        for mounted_volume in volumes.values_mut() {
            let admin_scope = mounted_volume.volume.admin_scope();
            admin_scope.shutdown();
            admin_scope.wait().await;

            self.lock_mount(mounted_volume).await;
        }
    }

    // Unmounts the volume identified by `store_id`.  The caller should take locks to avoid races if
    // necessary.
    //
    // NOTE: This will not terminate any connections on the admin scope.
    pub async fn unmount(&mut self, store_id: u64) -> Result<FxVolumeAndRoot, Error> {
        let mut mounted_volume =
            self.mounted_volumes.remove(&store_id).ok_or(FxfsError::NotFound)?;
        self.lock_mount(&mut mounted_volume).await;
        Ok(mounted_volume.volume)
    }

    // Auto-unmount the volume when the last connection to the volume is closed.
    fn auto_unmount(&self, store_id: u64) {
        let volumes_directory = self.volumes_directory.clone();
        let mounted_volume = self.mounted_volumes.get(&store_id).unwrap();
        let sequence = mounted_volume.sequence;
        let admin_scope = mounted_volume.volume.admin_scope().clone();
        let scope = mounted_volume.volume.volume().scope().clone();
        fasync::Task::spawn(
            async move {
                // Check the admin_scope first because once that has finished, there can never be
                // any more connections to it.
                admin_scope.wait().await;
                scope.wait().await;

                // Check that the same volume is still mounted i.e. there wasn't an explicit
                // unmount.
                let mut mounted_volumes = volumes_directory.lock().await;
                match mounted_volumes.mounted_volumes.get(&store_id) {
                    Some(m) if m.sequence == sequence => {}
                    _ => return,
                }

                warn!(store_id; "Last connection to volume closed without unmount, shutting down");
                let root_store = volumes_directory.root_volume.volume_directory().store();
                let fs = root_store.filesystem();
                let _guard = fs
                    .lock_manager()
                    .txn_lock(lock_keys![LockKey::object(
                        root_store.store_object_id(),
                        volumes_directory.root_volume.volume_directory().object_id(),
                    )])
                    .await;

                if let Err(e) = mounted_volumes.unmount(store_id).await {
                    warn!(e:?, store_id; "Failed to unmount volume");
                }
            }
            .trace(trace_future_args!("Volume::auto_unmount")),
        )
        .detach();
    }
}

impl VolumesDirectory {
    /// Fills the VolumesDirectory with all volumes in |root_volume|.  No volume is opened during
    /// this.
    pub async fn new(
        root_volume: RootVolume,
        inspect_tree: Weak<FsInspectTree>,
        mem_monitor: Option<MemoryPressureMonitor>,
        blob_resupplied_count: Arc<PageRefaultCounter>,
        memory_pressure_config: MemoryPressureConfig,
    ) -> Result<Arc<Self>, Error> {
        let layer_set = root_volume.volume_directory().store().tree().layer_set();
        let mut merger = layer_set.merger();
        let me = Arc::new(Self {
            root_volume,
            directory_node: vfs::directory::immutable::simple(),
            mounted_volumes: futures::lock::Mutex::new(HashMap::default()),
            inspect_tree,
            mem_monitor,
            blob_resupplied_count,
            profiling_state: futures::lock::Mutex::new(None),
            pager_dirty_bytes_count: PagerDirtyByteCount::new(),
            max_dirty_bytes_when_critical: AtomicU64::new(zx::system_get_physmem() / 100),
            on_volume_added: OnceLock::new(),
            memory_pressure_config,
        });
        let mut iter = me.root_volume.volume_directory().iter(&mut merger).await?;
        while let Some((name, store_id, object_descriptor)) = iter.get() {
            ensure!(*object_descriptor == ObjectDescriptor::Volume, FxfsError::Inconsistent);

            me.add_directory_entry(&name, store_id);

            iter.advance().await?;
        }
        Ok(me)
    }

    /// Clears the directory entry cache (dirent cache) and internal object store caches for all
    /// mounted volumes and filesystem metadata stores.
    pub async fn clear_caches(&self) {
        let volumes: Vec<_> =
            self.mounted_volumes.lock().await.values().map(|v| v.volume.volume().clone()).collect();
        for volume in volumes {
            volume.clear_caches();
        }
        let fs = self.root_volume.volume_directory().store().filesystem();
        fs.root_store().clear_caches();
        fs.root_parent_store().clear_caches();
        fs.allocator().tree().clear_cache();
    }

    /// Delete a profile for a given volume. Fails if that volume isn't mounted or if there is
    /// active profile recording or replay.
    pub async fn delete_profile(
        self: &Arc<Self>,
        volume_name: &str,
        profile_name: &str,
    ) -> Result<(), zx::Status> {
        // Volumes lock is taken first to provide consistent lock ordering with mounting a volume.
        let volumes = self.mounted_volumes.lock().await;
        let state = self.profiling_state.lock().await;

        // Only allow deletion when no operations are in flight. This removes confusion around
        // deleting a profile while one is recording with the same name, as the profile will not be
        // available for deletion until the recording completes. This would also mean that deleting
        // during a recording may succeed for deleting an older version but will be confusingly
        // replaced moments later.
        if state.is_some() {
            warn!("Failing profile deletion while profile operations are in flight.");
            return Err(zx::Status::SHOULD_WAIT);
        }
        for MountedVolume { volume, .. } in volumes.values() {
            if volume.volume().name() == volume_name {
                let dir = Arc::new(FxDirectory::new(
                    None,
                    volume.volume().get_profile_directory().await.map_err(map_to_status)?,
                ));
                return dir.unlink(profile_name, false).await;
            }
        }
        warn!(volume_name, profile_name; "Volume not found while deleting profile");
        Err(zx::Status::NOT_FOUND)
    }

    pub fn memory_pressure_monitor(&self) -> Option<&MemoryPressureMonitor> {
        self.mem_monitor.as_ref()
    }

    /// Stop all ongoing replays, and complete and persist ongoing recordings.
    pub async fn stop_profile_tasks(self: &Arc<Self>) {
        let mut state;
        let volumes;
        // Take the mounted_volumes lock first to keep consistent lock ordering with other
        // operations, but don't need to hold it for the entire operation. We need to take the
        // profiling_state lock before the mounted_volumes lock is dropped to ensure that another
        // thread doesn't mount a volume and start a profile task on it in between.
        {
            volumes = self
                .mounted_volumes
                .lock()
                .await
                .values()
                .map(|v| v.volume.volume().clone()) // Clones of each FxVolume.
                .collect::<Vec<Arc<FxVolume>>>();
            state = self.profiling_state.lock().await;
        }
        for volume in volumes {
            volume.stop_profile_tasks().await;
        }
        *state = None;
    }

    /// Record a named profile for a number of seconds, fails if there is an in flight recording or
    /// replay. The given volume must be unlocked, if no volume is given then all volumes will be
    /// affected and all volumes mounted during the process will also be affected.
    pub async fn record_and_replay_profile(
        self: &Arc<Self>,
        volume_name: Option<String>,
        profile_name: String,
        duration_secs: u32,
    ) -> Result<(), zx::Status> {
        // Volumes lock is taken first to provide consistent lock ordering with mounting a volume.
        let volumes = self.lock().await;
        let mut state = self.profiling_state.lock().await;
        if state.is_some() {
            // Consistency in the recording and replaying cannot be ensured at the volume level
            // if more than one operation can be in flight at a time.
            return Err(zx::Status::SHOULD_WAIT);
        }
        match volume_name.as_ref() {
            Some(volume_name) => {
                let (volume, is_blob) = volumes.get_unlocked_volume_by_name(&volume_name).await?;
                if let Err(error) = volume
                    .volume()
                    .record_and_replay_profile(new_profile_state(is_blob), &profile_name)
                    .await
                {
                    error!(
                        error:?,
                        profile_name = profile_name.as_str(),
                        volume_name = volume_name.as_str();
                        "Failed to record or replay profile",
                    );
                    return Err(map_to_status(error));
                }
            }
            None => {
                for MountedVolume { volume, .. } in volumes.mounted_volumes.values() {
                    let is_blob =
                        volume.root().clone().into_any().downcast::<BlobDirectory>().is_ok();
                    // Just log the errors, don't stop half-way.
                    if let Err(error) = volume
                        .volume()
                        .record_and_replay_profile(new_profile_state(is_blob), &profile_name)
                        .await
                    {
                        error!(
                            error:?,
                            profile_name = profile_name.as_str(),
                            volume_name = volume.volume().name();
                            "Failed to record or replay profile",
                        );
                    }
                }
            }
        }

        let this = self.clone();
        let task = fasync::Task::spawn(async move {
            fasync::Timer::new(fasync::MonotonicDuration::from_seconds(duration_secs.into())).await;
            this.stop_profile_tasks().await;
        });
        *state = Some(ProfileState { task, profile_name, all_volumes: volume_name.is_none() });
        Ok(())
    }

    /// Replays a profile if one exists, and only records if one does not exist.
    /// The given volume must be unlocked.
    pub async fn replay_xor_record_profile(
        self: &Arc<Self>,
        volume_name: String,
        profile_name: String,
        duration_secs: u32,
    ) -> Result<(), zx::Status> {
        // Volumes lock is taken first to provide consistent lock ordering with mounting a volume.
        let volumes = self.lock().await;
        let mut state = self.profiling_state.lock().await;
        if state.is_some() {
            return Err(zx::Status::SHOULD_WAIT);
        }
        let (volume, is_blob) = volumes.get_unlocked_volume_by_name(&volume_name).await?;
        if let Err(error) = volume
            .volume()
            .replay_xor_record_profile(new_profile_state(is_blob), &profile_name)
            .await
        {
            error!(
                error:?,
                profile_name = profile_name.as_str(),
                volume_name = volume_name.as_str();
                "Failed to replay or record profile",
            );
            return Err(map_to_status(error));
        }

        let this = self.clone();
        let task = fasync::Task::spawn(async move {
            fasync::Timer::new(fasync::MonotonicDuration::from_seconds(duration_secs.into())).await;
            this.stop_profile_tasks().await;
        });
        *state = Some(ProfileState { task, profile_name, all_volumes: false });
        Ok(())
    }

    /// Returns the directory node which can be used to provide connections for e.g. enumerating
    /// entries in the VolumesDirectory.
    /// Directly manipulating the entries in this node will result in strange behaviour.
    pub fn directory_node(&self) -> &Arc<vfs::directory::immutable::Simple> {
        &self.directory_node
    }

    // This serves as an exclusive lock for operations that manipulate the set of mounted volumes.
    pub async fn lock<'a>(self: &'a Arc<Self>) -> MountedVolumesGuard<'a> {
        MountedVolumesGuard {
            volumes_directory: self.clone(),
            mounted_volumes: self.mounted_volumes.lock().await,
        }
    }

    fn add_directory_entry(self: &Arc<Self>, name: &str, store_id: u64) {
        let weak = Arc::downgrade(self);
        let name_owned = Arc::new(name.to_string());
        self.directory_node
            .add_entry(
                name,
                vfs::service::host(move |requests| {
                    let weak = weak.clone();
                    let name = name_owned.clone();
                    async move {
                        if let Some(me) = weak.upgrade() {
                            let _ =
                                me.handle_volume_requests(name.as_ref(), requests, store_id).await;
                        }
                    }
                }),
            )
            .unwrap();
    }

    /// Creates and mounts a new volume. If |crypt| is set, the volume will be encrypted. The
    /// volume is mounted according to |as_blob|.
    pub async fn create_and_mount_volume(
        self: &Arc<Self>,
        name: &str,
        crypt: Option<Arc<dyn Crypt>>,
        as_blob: bool,
        options: CreateOptions,
    ) -> Result<FxVolumeAndRoot, Error> {
        self.lock()
            .await
            .create_or_mount_volume(
                name,
                crypt,
                Mode::Create {
                    guid: options.guid,
                    low_32_bit_object_ids: options.restrict_inode_ids_to_32_bit.unwrap_or(false),
                },
                as_blob,
            )
            .await
    }

    /// Mounts an existing volume. `crypt` will be used to unlock the volume if provided.
    /// If `as_blob` is `true`, the volume will be mounted as a blob filesystem, otherwise
    /// it will be treated as a regular fxfs volume.
    pub async fn mount_volume(
        self: &Arc<Self>,
        name: &str,
        crypt: Option<Arc<dyn Crypt>>,
        as_blob: bool,
    ) -> Result<FxVolumeAndRoot, Error> {
        self.lock().await.create_or_mount_volume(name, crypt, Mode::Mount, as_blob).await
    }

    /// Checks the integrity of volume `name`, or succeeds silently if the volume doesn't exist.
    pub async fn check_volume(
        self: &Arc<Self>,
        name: &str,
        crypt: Option<Arc<dyn Crypt>>,
    ) -> Result<(), Error> {
        let fs = self.root_volume.volume_directory().store().filesystem();
        let store = self.lock().await.check_volume(fs.as_ref(), name, crypt).await?;
        if let Some(store) = store {
            if store.is_unlocked() {
                let _ = store.lock().await;
            }
        }
        Ok(())
    }

    /// Removes a volume. The volume must exist but encrypted volume keys are not required.
    pub async fn remove_volume(self: &Arc<Self>, name: &str) -> Result<(), Error> {
        self.lock().await.remove_volume(name).await
    }

    /// Terminates all opened volumes.  This will not cancel any profiling that might be taking
    /// place.
    pub async fn terminate(self: &Arc<Self>) {
        // Abort the profiling timer task.
        let profiling_state = self.profiling_state.lock().await.take();
        if let Some(state) = profiling_state {
            state.task.abort().await;
        }
        self.lock().await.terminate().await;
        // TODO(https://fxbug.dev/452935329): Turn this into a real assert.
        debug_assert!(
            self.pager_dirty_bytes_count.load() == 0,
            "Leaked {} dirty bytes.",
            self.pager_dirty_bytes_count.load()
        );
    }

    /// Serves the given volume on `outgoing_dir_server_end`.
    pub fn serve_volume(
        self: &Arc<Self>,
        volume: &FxVolumeAndRoot,
        outgoing_dir_server_end: ServerEnd<fio::DirectoryMarker>,
        as_blob: bool,
    ) -> Result<(), Error> {
        // A note regarding strong references to `FxVolume`: connections to services here are all on
        // the admin scope.  When we force lock a volume, we want to keep the admin scope running
        // but terminate the volume.  When a volume is terminated in this way, we want to ensure
        // that there are no outstanding strong references to the volume.  With that in mind, the
        // services here should not hold strong references.  They can hold a strong reference to
        // VolumesDirectory and find the volume via the mount list, or they can hold a weak
        // reference to the FxVolume.  Before upgrading the weak reference, an active guard must be
        // acquired on the volume's scope first.  This ensures that no new strong references are
        // taken once the volume has commenced termination.  The "root" entry just below is an
        // exception that does hold a strong reference.  We handle that by making sure we remove
        // that entry before we call `FxVolume::terminate`.

        let outgoing_dir = volume.outgoing_dir();
        outgoing_dir.add_entry("root", volume.root().clone().as_directory_entry())?;
        let svc_dir = vfs::directory::immutable::simple();
        outgoing_dir.add_entry("svc", svc_dir.clone())?;

        let store_id = volume.volume().store().store_object_id();
        let me = self.clone();
        svc_dir.add_entry(
            AdminMarker::PROTOCOL_NAME,
            vfs::service::host(move |requests| {
                let me = me.clone();
                async move {
                    let _ = me.handle_admin_requests(requests, store_id).await;
                }
            }),
        )?;
        let vol_scope = volume.volume().scope().clone();
        let weak_vol = Arc::downgrade(volume.volume());
        {
            let vol_scope = vol_scope.clone();
            let weak_vol = weak_vol.clone();
            svc_dir.add_entry(
                ProjectIdMarker::PROTOCOL_NAME,
                vfs::service::host(move |requests| {
                    let weak_vol = weak_vol.clone();
                    let scope = vol_scope.clone();
                    async move {
                        let _ =
                            FxVolume::handle_project_id_requests(weak_vol, scope, requests).await;
                    }
                }),
            )?;
        }
        svc_dir.add_entry(
            FileBackedVolumeProviderMarker::PROTOCOL_NAME,
            vfs::service::host(move |requests| {
                let weak_vol = weak_vol.clone();
                let scope = vol_scope.clone();
                async move {
                    let _ = FxVolume::handle_file_backed_volume_provider_requests(
                        weak_vol, scope, requests,
                    )
                    .await;
                }
            }),
        )?;
        {
            let vol_scope = volume.volume().scope().clone();
            let weak_vol = Arc::downgrade(volume.volume());
            svc_dir.add_entry(
                DebugMarker::PROTOCOL_NAME,
                vfs::service::host(move |requests| {
                    let weak_vol = weak_vol.clone();
                    let scope = vol_scope.clone();
                    async move {
                        let _ = FxVolume::handle_debug_requests(weak_vol, scope, requests).await;
                    }
                }),
            )?;
        }
        volume.root().clone().register_additional_volume_services(&svc_dir)?;

        let scope = volume.admin_scope().clone();
        let mut flags = fio::PERM_READABLE | fio::PERM_WRITABLE;
        if as_blob {
            flags |= fio::PERM_EXECUTABLE;
        }
        vfs::directory::serve_on(Arc::clone(outgoing_dir), flags, scope, outgoing_dir_server_end);

        info!(
            store_id;
            "Serving volume, pager port koid={}",
            fasync::EHandle::local().port().koid().unwrap().raw_koid()
        );
        Ok(())
    }

    /// Creates and serves the volume with the given name.
    pub async fn create_and_serve_volume(
        self: &Arc<Self>,
        name: &str,
        outgoing_directory_server_end: ServerEnd<fio::DirectoryMarker>,
        mount_options: MountOptions,
        create_options: CreateOptions,
    ) -> Result<(), Error> {
        let mut guard = self.lock().await;
        let crypt =
            mount_options.crypt.map(|crypt| Arc::new(RemoteCrypt::new(crypt)) as Arc<dyn Crypt>);
        let as_blob = mount_options.as_blob.unwrap_or(false);
        let guid = create_options.guid;
        let low_32_bit_object_ids = create_options.restrict_inode_ids_to_32_bit.unwrap_or(false);
        let volume = guard
            .create_or_mount_volume(
                name,
                crypt,
                Mode::Create { guid, low_32_bit_object_ids },
                as_blob,
            )
            .await?;
        self.serve_volume(&volume, outgoing_directory_server_end, as_blob)
            .context("failed to serve volume")?;
        guard.auto_unmount(volume.volume().store().store_object_id());
        Ok(())
    }

    async fn handle_volume_requests(
        self: &Arc<Self>,
        name: &str,
        mut requests: VolumeRequestStream,
        store_id: u64,
    ) -> Result<(), Error> {
        while let Some(request) = requests.try_next().await? {
            match request {
                VolumeRequest::Check { responder, options } => {
                    async move {
                        responder.send(self.handle_check(store_id, options).await.map_err(
                            |error| {
                                error!(error:?, store_id; "Failed to check volume");
                                map_to_raw_status(error)
                            },
                        ))
                    }
                    .trace(trace_future_args!("Volume::Check"))
                    .await?;
                }
                VolumeRequest::Mount { responder, outgoing_directory, options } => {
                    async move {
                        responder.send(
                            self.handle_mount(name, store_id, outgoing_directory, options)
                                .await
                                .map_err(|error| {
                                    error!(error:?, name, store_id; "Failed to mount volume");
                                    map_to_raw_status(error)
                                }),
                        )
                    }
                    .trace(trace_future_args!("Volume::Mount"))
                    .await?;
                }
                VolumeRequest::SetLimit { responder, bytes } => {
                    async move {
                        responder.send(self.handle_set_limit(store_id, bytes).await.map_err(
                            |error| {
                                error!(error:?, store_id; "Failed to set volume limit");
                                map_to_raw_status(error)
                            },
                        ))
                    }
                    .trace(trace_future_args!("Volume::SetLimit"))
                    .await?;
                }
                VolumeRequest::GetLimit { responder } => {
                    fxfs_trace::duration!("Volume::GetLimit");
                    responder.send(Ok(self.handle_get_limit(store_id)))?
                }
                VolumeRequest::GetInfo { responder } => {
                    async move {
                        let result = self.handle_get_info(store_id).await.map(|guid| {
                            fidl_fuchsia_fs_startup::VolumeInfo {
                                guid: Some(guid),
                                ..Default::default()
                            }
                        });
                        match result {
                            Ok(response) => responder.send(Ok(&response)),
                            Err(error) => {
                                error!(error:?, store_id; "Failed to get volume info");
                                responder.send(Err(map_to_raw_status(error)))
                            }
                        }
                    }
                    .trace(trace_future_args!("Volume::GetInfo"))
                    .await?;
                }
            }
        }
        Ok(())
    }

    pub fn memory_pressure_config(&self) -> &MemoryPressureConfig {
        &self.memory_pressure_config
    }

    fn is_flush_required_to_dirty(&self, byte_count: u64) -> bool {
        let mem_pressure = self
            .mem_monitor
            .as_ref()
            .map(|mem_monitor| mem_monitor.level())
            .unwrap_or(MemoryPressureLevel::Normal);
        if !matches!(mem_pressure, MemoryPressureLevel::Critical) {
            return false;
        }

        let total_dirty = self.pager_dirty_bytes_count.load();
        total_dirty + byte_count >= self.max_dirty_bytes_when_critical.load(Ordering::Relaxed)
    }

    /// Reports that a certain number of bytes will be dirtied in a pager-backed VMO. If the memory
    /// pressure level is critical and fxfs has lots of dirty pages then a new task will be spawned
    /// in `volume` to flush the dirty pages before `mark_dirty` is called. If the memory pressure
    /// level is not critical then `mark_dirty` will be synchronously called.
    pub fn report_pager_dirty(
        self: Arc<Self>,
        byte_count: u64,
        volume: Arc<FxVolume>,
        mark_dirty: impl FnOnce() + Send + 'static,
    ) {
        if !self.is_flush_required_to_dirty(byte_count) {
            self.pager_dirty_bytes_count.fetch_add(byte_count);
            mark_dirty();
        } else {
            volume.spawn(
                async move {
                    let volumes = self.mounted_volumes.lock().await;

                    // Re-check the number of outstanding pager dirty bytes because another thread
                    // could have raced and flushed the volumes first.
                    if self.is_flush_required_to_dirty(byte_count) {
                        debug!(
                            "Flushing all volumes. Memory pressure is critical & dirty pager bytes \
                            ({} MiB) >= limit ({} MiB)",
                            self.pager_dirty_bytes_count.load() / MEBIBYTE,
                            self.max_dirty_bytes_when_critical.load(Ordering::Relaxed) / MEBIBYTE
                        );

                        let flushes = FuturesUnordered::new();
                        for MountedVolume { volume, .. } in volumes.values() {
                            let vol = volume.volume().clone();
                            flushes.push(async move {
                                vol.minimize_memory().await;
                            });
                        }

                        flushes.collect::<()>().await;
                    }
                    self.pager_dirty_bytes_count.fetch_add(byte_count);
                    mark_dirty();
                }
                .trace(trace_future_args!("flush-before-mark-dirty")),
            )
        }
    }

    /// Reports that a certain number of bytes were cleaned in a pager-backed VMO.
    pub fn report_pager_clean(&self, byte_count: u64) {
        let prev_dirty = self.pager_dirty_bytes_count.fetch_sub(byte_count);
        // TODO(https://fxbug.dev/452935329): Turn this into a real assert.
        debug_assert!(prev_dirty >= byte_count, "Underflowed dirty bytes.");

        if prev_dirty < byte_count {
            // An unlikely scenario, but if there was an underflow, reset the pager dirty bytes to
            // zero.
            self.pager_dirty_bytes_count.store(0);
        }
    }

    async fn handle_check(
        self: &Arc<Self>,
        store_id: u64,
        options: CheckOptions,
    ) -> Result<(), Error> {
        let fs = self.root_volume.volume_directory().store().filesystem();
        let crypt = if let Some(crypt) = options.crypt {
            Some(Arc::new(RemoteCrypt::new(crypt)) as Arc<dyn Crypt>)
        } else {
            None
        };
        let result = fsck::fsck_volume(fs.as_ref(), store_id, crypt).await?;
        // TODO(b/311550633): Stash result in inspect.
        info!(store_id:%; "{result:?}");
        Ok(())
    }

    async fn handle_set_limit(self: &Arc<Self>, store_id: u64, bytes: u64) -> Result<(), Error> {
        let store = self.root_volume.volume_directory().store();
        let mut transaction = store.new_transaction(lock_keys![], Options::default()).await?;
        store.filesystem().allocator().set_bytes_limit(&mut transaction, store_id, bytes)?;
        transaction.commit().await?;
        Ok(())
    }

    fn handle_get_limit(self: &Arc<Self>, store_id: u64) -> u64 {
        let fs = self.root_volume.volume_directory().store().filesystem();
        fs.allocator().get_owner_bytes_limit(store_id).unwrap_or_default()
    }

    async fn handle_get_info(self: &Arc<Self>, store_id: u64) -> Result<[u8; 16], Error> {
        let fs = self.root_volume.volume_directory().store().filesystem();
        let store =
            fs.object_manager().store(store_id).ok_or_else(|| anyhow!("Store not found"))?;
        Ok(store.guid())
    }

    async fn handle_mount(
        self: &Arc<Self>,
        name: &str,
        store_id: u64,
        outgoing_directory_server_end: ServerEnd<fio::DirectoryMarker>,
        options: MountOptions,
    ) -> Result<(), Error> {
        info!(name:%, store_id:%, options:?; "Received mount request");
        let crypt = options.crypt.map(|crypt| Arc::new(RemoteCrypt::new(crypt)) as Arc<dyn Crypt>);
        let as_blob = options.as_blob.unwrap_or(false);
        let mut guard = self.lock().await;
        let volume = guard
            .create_or_mount_volume(name, crypt, Mode::Mount, as_blob)
            .await
            .context("failed to mount volume")?;
        self.serve_volume(&volume, outgoing_directory_server_end, as_blob)
            .context("failed to serve volume")?;
        guard.auto_unmount(volume.volume().store().store_object_id());
        Ok(())
    }

    async fn handle_admin_requests(
        self: &Arc<Self>,
        mut stream: AdminRequestStream,
        store_id: u64,
    ) -> Result<(), Error> {
        // If the Admin protocol ever supports more methods, this should change to a while.
        if let Some(request) = stream.try_next().await.context("Reading request")? {
            match request {
                AdminRequest::Shutdown { responder } => {
                    info!(store_id; "Received shutdown request for volume");

                    let root_store = self.root_volume.volume_directory().store();
                    let fs = root_store.filesystem();
                    let _guard = fs
                        .lock_manager()
                        .txn_lock(lock_keys![LockKey::object(
                            root_store.store_object_id(),
                            self.root_volume.volume_directory().object_id(),
                        )])
                        .await;

                    let maybe_volume = self.lock().await.unmount(store_id).await;
                    responder
                        .send()
                        .unwrap_or_else(|e| warn!("Failed to send shutdown response: {}", e));

                    if let Ok(volume) = maybe_volume {
                        // NOTE: After calling this, this task might be dropped at the next await
                        // point.
                        volume.admin_scope().shutdown();
                    }

                    return Ok(());
                }
            }
        }
        Ok(())
    }

    /// Sets a callback which is invoked when a volume is added.  When the volume is removed, this
    /// is called again with `None` as the second parameter. The root store is included along with
    /// volume so the volume's layers can be accessed. Note that this can only be set once per
    /// VolumesDirectory; repeated calls will panic.
    pub fn set_on_mount_callback<
        F: Fn(&str, Option<(Arc<FxVolume>, Arc<ObjectStore>)>) + Send + Sync + 'static,
    >(
        &self,
        callback: F,
    ) {
        self.on_volume_added.set(Box::new(callback)).ok().unwrap();
    }

    pub async fn install_volume(
        self: &Arc<Self>,
        src: &str,
        image_file: &str,
        dst: &str,
    ) -> Result<(), Error> {
        let guard = self.lock().await;
        info!("installing {src}/{image_file} -> {dst}");
        for MountedVolume { volume, .. } in guard.mounted_volumes.values() {
            if volume.volume().name() == src {
                return Err(zx::Status::ALREADY_BOUND)
                    .with_context(|| format!("volume {src} is already mounted"));
            }
            if volume.volume().name() == dst {
                return Err(zx::Status::ALREADY_BOUND)
                    .with_context(|| format!("volume {dst} is already mounted"));
            }
        }
        guard.volumes_directory.root_volume.install_volume(&src, &image_file, &dst).await?;

        // The above function ensures that we've deleted `src` and `dst` now exists. Before we
        // release `guard`, we need to update the entries in the volumes directory accordingly.
        guard
            .volumes_directory
            .directory_node()
            .remove_entry(src, /* must_be_directory: */ false)
            .unwrap();
        guard
            .volumes_directory
            .directory_node()
            .remove_entry(dst, /* must_be_directory: */ false)
            .unwrap();
        let new_dst_object_id =
            match guard.volumes_directory.root_volume.volume_directory().lookup(dst).await? {
                Some((object_id, ObjectDescriptor::Volume, _)) => Ok(object_id),
                Some(_) => Err(FxfsError::Inconsistent),
                None => Err(FxfsError::NotFound),
            }?;
        self.add_directory_entry(dst, new_dst_object_id);

        info!("install complete");
        Ok(())
    }
}

#[cfg(test)]
pub(crate) fn serve_startup_volume_proxy(
    volumes_directory: &Arc<VolumesDirectory>,
    volume_name: &str,
) -> (fidl_fuchsia_fs_startup::VolumeProxy, vfs::ExecutionScope) {
    use vfs::ToObjectRequest;
    use vfs::service::ServiceLike;
    let scope = vfs::ExecutionScope::new();
    let entry = volumes_directory.directory_node().get_entry(volume_name).unwrap();
    let service = entry.into_any().downcast::<vfs::service::Service>().unwrap();
    let (proxy, server) = fidl::endpoints::create_proxy::<fidl_fuchsia_fs_startup::VolumeMarker>();
    service
        .connect(
            scope.clone(),
            Default::default(),
            &mut fio::Flags::PROTOCOL_SERVICE.to_object_request(server),
        )
        .unwrap();
    (proxy, scope)
}

struct PagerDirtyByteCount(AtomicU64);

impl PagerDirtyByteCount {
    pub fn new() -> Self {
        Self(AtomicU64::new(0))
    }

    pub fn fetch_add(&self, value: u64) -> u64 {
        let prev = self.0.fetch_add(value, Ordering::Relaxed);
        fxfs_trace::counter!("dirty-bytes", 0, "total" => prev.saturating_add(value));
        prev
    }

    pub fn fetch_sub(&self, value: u64) -> u64 {
        let prev = self.0.fetch_sub(value, Ordering::Relaxed);
        fxfs_trace::counter!("dirty-bytes", 0, "total" => prev.saturating_sub(value));
        prev
    }

    pub fn load(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }

    pub fn store(&self, value: u64) {
        self.0.store(value, Ordering::Relaxed);
        fxfs_trace::counter!("dirty-bytes", 0, "total" => value);
    }
}

#[cfg(test)]
mod tests {
    use super::{Mode, serve_startup_volume_proxy};
    use crate::fuchsia::RemoteCrypt;
    use crate::fuchsia::memory_pressure::MemoryPressureLevel;
    use crate::fuchsia::testing::{self, TestFixture, open_dir_checked, open_file_checked};
    use crate::fuchsia::volume::MemoryPressureConfig;
    use crate::fuchsia::volumes_directory::VolumesDirectory;
    use crate::testing::TestFixtureOptions;
    use fidl::endpoints::{DiscoverableProtocolMarker, create_proxy, create_request_stream};
    use fidl_fuchsia_fs::AdminMarker;
    use fidl_fuchsia_fs_startup::{CreateOptions, MountOptions, VolumeProxy};
    use fidl_fuchsia_fxfs::{CryptRequest, DebugMarker, FxfsKey, KeyPurpose, WrappedKey};
    use fidl_fuchsia_io as fio;
    use fuchsia_async as fasync;
    use fuchsia_component_client::connect_to_protocol_at_dir_svc;
    use fuchsia_fs::file;
    use futures::{TryStreamExt, join};
    use fxfs::errors::FxfsError;
    use fxfs::filesystem::{FlushReason, ForceMajor, FxFilesystem};
    use fxfs::fsck::{FsckOptions, fsck, fsck_volume_with_options, fsck_with_options};
    use fxfs::lock_keys;
    use fxfs::object_handle::ObjectHandle;
    use fxfs::object_store::allocator::Allocator;
    use fxfs::object_store::transaction::{LockKey, Options};
    use fxfs::object_store::volume::root_volume;
    use fxfs_crypto::Crypt;
    use fxfs_insecure_crypto::new_insecure_crypt;
    use refaults_vmo::PageRefaultCounter;
    use std::sync::atomic::Ordering;
    use std::sync::{Arc, Weak};
    use std::time::Duration;
    use storage_device::DeviceHolder;
    use storage_device::fake_device::FakeDevice;
    use storage_units::{BlockSize, PAGE_SIZE};
    use vfs::execution_scope::ExecutionScope;
    use vfs::temp_clone::{TempClonable, unblock};
    use zx::Status;
    async fn write_image_to_file(image: DeviceHolder, file: fio::FileProxy) {
        file.resize(image.size()).await.unwrap().expect("resize failed");
        let vmo = TempClonable::new(
            file.get_backing_memory(fio::VmoFlags::SHARED_BUFFER | fio::VmoFlags::WRITE)
                .await
                .unwrap()
                .expect("get backing memory failed"),
        );

        const CHUNK_READ_SIZE: usize = 131_072; /* 128 KiB */
        let mut buff = image.allocate_buffer(CHUNK_READ_SIZE).await;
        let total = image.size();
        let mut offset = 0;
        while offset < total {
            let amount = std::cmp::min(total - offset, CHUNK_READ_SIZE as u64);
            image.read(offset, buff.as_mut()).await.expect("image read failed");
            {
                // *NOTE*: We have to unblock our write to the VMO since it's pager backed and could
                // be running on the same thread as the filesystem is.
                let vmo = vmo.temp_clone();
                let data = buff.subslice(0..amount as usize).to_vec();
                let offset = offset;
                unblock(move || vmo.write(&data, offset)).await.expect("vmo write failed");
            }
            offset += amount;
        }
        assert_eq!(offset, total);
        file.sync().await.unwrap().expect("sync failed");
        file.close().await.unwrap().expect("close failed");
    }

    #[fuchsia::test]
    async fn test_volume_creation() {
        let device = DeviceHolder::new(FakeDevice::new(8192, 512));
        let filesystem = FxFilesystem::new_empty(device).await.unwrap();
        let blob_resupplied_count =
            Arc::new(PageRefaultCounter::new().expect("Failed to create PageRefaultCounter"));
        let volumes_directory = VolumesDirectory::new(
            root_volume(filesystem.clone()).await.unwrap(),
            Weak::new(),
            None,
            blob_resupplied_count,
            MemoryPressureConfig::default(),
        )
        .await
        .unwrap();

        let crypt = Arc::new(new_insecure_crypt()) as Arc<dyn Crypt>;
        {
            let vol = volumes_directory
                .create_and_mount_volume(
                    "encrypted",
                    Some(crypt.clone()),
                    false,
                    CreateOptions::default(),
                )
                .await
                .expect("create encrypted volume failed");
            vol.volume().store().store_object_id()
        };

        volumes_directory.terminate().await;
        std::mem::drop(volumes_directory);
        filesystem.close().await.expect("close filesystem failed");
        let device = filesystem.take_device().await;
        device.reopen(false);
        let filesystem = FxFilesystem::open(device).await.unwrap();
        let blob_resupplied_count =
            Arc::new(PageRefaultCounter::new().expect("Failed to create PageRefaultCounter"));
        let volumes_directory = VolumesDirectory::new(
            root_volume(filesystem.clone()).await.unwrap(),
            Weak::new(),
            None,
            blob_resupplied_count,
            MemoryPressureConfig::default(),
        )
        .await
        .unwrap();

        let error = volumes_directory
            .create_and_mount_volume(
                "encrypted",
                Some(crypt.clone()),
                false,
                CreateOptions::default(),
            )
            .await
            .err()
            .expect("Creating existing encrypted volume should fail");
        assert!(FxfsError::AlreadyExists.matches(&error));
    }

    #[fuchsia::test]
    async fn test_dirty_pages_accumulate_in_parent() {
        let device = DeviceHolder::new(FakeDevice::new(8192, 512));
        let filesystem = FxFilesystem::new_empty(device).await.unwrap();
        let blob_resupplied_count =
            Arc::new(PageRefaultCounter::new().expect("Failed to create PageRefaultCounter"));
        let volumes_directory = VolumesDirectory::new(
            root_volume(filesystem.clone()).await.unwrap(),
            Weak::new(),
            None,
            blob_resupplied_count,
            MemoryPressureConfig::default(),
        )
        .await
        .unwrap();

        let crypt = Arc::new(new_insecure_crypt()) as Arc<dyn Crypt>;
        let vol = volumes_directory
            .create_and_mount_volume(
                "encrypted",
                Some(crypt.clone()),
                false,
                CreateOptions::default(),
            )
            .await
            .expect("create encrypted volume failed");
        let old_dirty = volumes_directory.pager_dirty_bytes_count.load();

        let new_dirty = {
            let (root, server_end) = create_proxy::<fio::DirectoryMarker>();
            vol.root().clone().serve(fio::PERM_READABLE | fio::PERM_WRITABLE, server_end);
            let f = open_file_checked(
                &root,
                "foo",
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE
                    | fio::Flags::PROTOCOL_FILE,
                &Default::default(),
            )
            .await;
            let buf = vec![0xaa as u8; 8192];
            file::write(&f, buf.as_slice()).await.expect("Write");
            // It's important to check the dirty bytes before closing the file, as closing can
            // trigger a flush.
            volumes_directory.pager_dirty_bytes_count.load()
        };
        assert_ne!(old_dirty, new_dirty);

        volumes_directory.terminate().await;
        std::mem::drop(volumes_directory);
        filesystem.close().await.expect("close filesystem failed");
    }

    #[fuchsia::test]
    async fn test_volume_reopen() {
        let device = DeviceHolder::new(FakeDevice::new(8192, 512));
        let filesystem = FxFilesystem::new_empty(device).await.unwrap();
        let blob_resupplied_count =
            Arc::new(PageRefaultCounter::new().expect("Failed to create PageRefaultCounter"));
        let volumes_directory = VolumesDirectory::new(
            root_volume(filesystem.clone()).await.unwrap(),
            Weak::new(),
            None,
            blob_resupplied_count,
            MemoryPressureConfig::default(),
        )
        .await
        .unwrap();

        let crypt = Arc::new(new_insecure_crypt()) as Arc<dyn Crypt>;
        let volume_id = {
            let vol = volumes_directory
                .create_and_mount_volume(
                    "encrypted",
                    Some(crypt.clone()),
                    false,
                    CreateOptions::default(),
                )
                .await
                .expect("create encrypted volume failed");
            vol.volume().store().store_object_id()
        };

        volumes_directory.terminate().await;
        std::mem::drop(volumes_directory);
        filesystem.close().await.expect("close filesystem failed");
        let device = filesystem.take_device().await;
        device.reopen(false);
        let filesystem = FxFilesystem::open(device).await.unwrap();
        let blob_resupplied_count =
            Arc::new(PageRefaultCounter::new().expect("Failed to create PageRefaultCounter"));
        let volumes_directory = VolumesDirectory::new(
            root_volume(filesystem.clone()).await.unwrap(),
            Weak::new(),
            None,
            blob_resupplied_count,
            MemoryPressureConfig::default(),
        )
        .await
        .unwrap();

        {
            let vol = volumes_directory
                .mount_volume("encrypted", Some(crypt.clone()), false)
                .await
                .expect("open existing encrypted volume failed");
            assert_eq!(vol.volume().store().store_object_id(), volume_id);
        }

        volumes_directory.terminate().await;
        std::mem::drop(volumes_directory);
        filesystem.close().await.expect("close filesystem failed");
    }

    #[fuchsia::test]
    async fn test_volume_creation_unencrypted() {
        let device = DeviceHolder::new(FakeDevice::new(8192, 512));
        let filesystem = FxFilesystem::new_empty(device).await.unwrap();
        let blob_resupplied_count =
            Arc::new(PageRefaultCounter::new().expect("Failed to create PageRefaultCounter"));
        let volumes_directory = VolumesDirectory::new(
            root_volume(filesystem.clone()).await.unwrap(),
            Weak::new(),
            None,
            blob_resupplied_count,
            MemoryPressureConfig::default(),
        )
        .await
        .unwrap();

        {
            let vol = volumes_directory
                .create_and_mount_volume("unencrypted", None, false, CreateOptions::default())
                .await
                .expect("create unencrypted volume failed");
            vol.volume().store().store_object_id()
        };

        volumes_directory.terminate().await;
        std::mem::drop(volumes_directory);
        filesystem.close().await.expect("close filesystem failed");
        let device = filesystem.take_device().await;
        device.reopen(false);
        let filesystem = FxFilesystem::open(device).await.unwrap();
        let blob_resupplied_count =
            Arc::new(PageRefaultCounter::new().expect("Failed to create PageRefaultCounter"));
        let volumes_directory = VolumesDirectory::new(
            root_volume(filesystem.clone()).await.unwrap(),
            Weak::new(),
            None,
            blob_resupplied_count,
            MemoryPressureConfig::default(),
        )
        .await
        .unwrap();

        let error = volumes_directory
            .create_and_mount_volume("unencrypted", None, false, CreateOptions::default())
            .await
            .err()
            .expect("Creating existing unencrypted volume should fail");
        assert!(FxfsError::AlreadyExists.matches(&error));

        volumes_directory.terminate().await;
        std::mem::drop(volumes_directory);
        filesystem.close().await.expect("close filesystem failed");
    }

    #[fuchsia::test]
    async fn test_volume_reopen_unencrypted() {
        let device = DeviceHolder::new(FakeDevice::new(8192, 512));
        let filesystem = FxFilesystem::new_empty(device).await.unwrap();
        let blob_resupplied_count =
            Arc::new(PageRefaultCounter::new().expect("Failed to create PageRefaultCounter"));
        let volumes_directory = VolumesDirectory::new(
            root_volume(filesystem.clone()).await.unwrap(),
            Weak::new(),
            None,
            blob_resupplied_count,
            MemoryPressureConfig::default(),
        )
        .await
        .unwrap();

        let volume_id = {
            let vol = volumes_directory
                .create_and_mount_volume("unencrypted", None, false, CreateOptions::default())
                .await
                .expect("create unencrypted volume failed");
            vol.volume().store().store_object_id()
        };

        volumes_directory.terminate().await;
        std::mem::drop(volumes_directory);
        filesystem.close().await.expect("close filesystem failed");
        let device = filesystem.take_device().await;
        device.reopen(false);
        let filesystem = FxFilesystem::open(device).await.unwrap();
        let blob_resupplied_count =
            Arc::new(PageRefaultCounter::new().expect("Failed to create PageRefaultCounter"));
        let volumes_directory = VolumesDirectory::new(
            root_volume(filesystem.clone()).await.unwrap(),
            Weak::new(),
            None,
            blob_resupplied_count,
            MemoryPressureConfig::default(),
        )
        .await
        .unwrap();

        {
            let vol = volumes_directory
                .mount_volume("unencrypted", None, false)
                .await
                .expect("open existing unencrypted volume failed");
            assert_eq!(vol.volume().store().store_object_id(), volume_id);
        }

        volumes_directory.terminate().await;
        std::mem::drop(volumes_directory);
        filesystem.close().await.expect("close filesystem failed");
    }

    #[fuchsia::test]
    async fn test_volume_enumeration() {
        let device = DeviceHolder::new(FakeDevice::new(8192, 512));
        let filesystem = FxFilesystem::new_empty(device).await.unwrap();
        let blob_resupplied_count =
            Arc::new(PageRefaultCounter::new().expect("Failed to create PageRefaultCounter"));
        let volumes_directory = VolumesDirectory::new(
            root_volume(filesystem.clone()).await.unwrap(),
            Weak::new(),
            None,
            blob_resupplied_count,
            MemoryPressureConfig::default(),
        )
        .await
        .unwrap();

        // Add an encrypted volume...
        let crypt = Arc::new(new_insecure_crypt()) as Arc<dyn Crypt>;
        {
            volumes_directory
                .create_and_mount_volume(
                    "encrypted",
                    Some(crypt.clone()),
                    false,
                    CreateOptions::default(),
                )
                .await
                .expect("create encrypted volume failed");
        };
        // And an unencrypted volume.
        {
            volumes_directory
                .create_and_mount_volume("unencrypted", None, false, CreateOptions::default())
                .await
                .expect("create unencrypted volume failed");
        };

        // Restart, so that we can test enumeration of unopened volumes.
        volumes_directory.terminate().await;
        std::mem::drop(volumes_directory);
        filesystem.close().await.expect("close filesystem failed");
        let device = filesystem.take_device().await;
        device.reopen(false);
        let filesystem = FxFilesystem::open(device).await.unwrap();
        let blob_resupplied_count =
            Arc::new(PageRefaultCounter::new().expect("Failed to create PageRefaultCounter"));
        let volumes_directory = VolumesDirectory::new(
            root_volume(filesystem.clone()).await.unwrap(),
            Weak::new(),
            None,
            blob_resupplied_count,
            MemoryPressureConfig::default(),
        )
        .await
        .unwrap();

        let readdir = |dir: Arc<fio::DirectoryProxy>| async move {
            let status = dir.rewind().await.expect("FIDL call failed");
            Status::ok(status).expect("rewind failed");
            let (status, buf) = dir.read_dirents(fio::MAX_BUF).await.expect("FIDL call failed");
            Status::ok(status).expect("read_dirents failed");
            let mut entries = vec![];
            for res in fuchsia_fs::directory::parse_dir_entries(&buf) {
                entries.push(res.expect("Failed to parse entry").name);
            }
            entries
        };

        let dir_proxy = Arc::new(vfs::directory::serve_read_only(
            volumes_directory.directory_node().clone(),
            ExecutionScope::new(),
        ));
        let entries = readdir(dir_proxy.clone()).await;
        assert_eq!(entries, [".", "encrypted", "unencrypted"]);

        let _vol = volumes_directory
            .mount_volume("encrypted", Some(crypt.clone()), false)
            .await
            .expect("Open encrypted volume failed");

        // Ensure that the behaviour is the same after we've opened a volume.
        let entries = readdir(dir_proxy).await;
        assert_eq!(entries, [".", "encrypted", "unencrypted"]);

        volumes_directory.terminate().await;
        std::mem::drop(volumes_directory);
        filesystem.close().await.expect("close filesystem failed");
    }

    #[fuchsia::test]
    async fn test_get_info() {
        let device = DeviceHolder::new(FakeDevice::new(8192, 512));
        let filesystem = FxFilesystem::new_empty(device).await.unwrap();
        let root_volume = root_volume(filesystem.clone()).await.unwrap();
        let blob_resupplied_count =
            Arc::new(PageRefaultCounter::new().expect("Failed to create PageRefaultCounter"));
        let volumes_directory = VolumesDirectory::new(
            root_volume,
            Weak::new(),
            None,
            blob_resupplied_count,
            MemoryPressureConfig::default(),
        )
        .await
        .unwrap();

        let vol = volumes_directory
            .create_and_mount_volume("vol", None, false, CreateOptions::default())
            .await
            .expect("create_and_mount_volume failed");
        let guid = vol.volume().store().guid();

        let (volume_proxy, _scope) = serve_startup_volume_proxy(&volumes_directory, "vol");

        let info: fidl_fuchsia_fs_startup::VolumeInfo = volume_proxy
            .get_info()
            .await
            .expect("get_info failed")
            .expect("get_info returned error");
        assert_eq!(info.guid, Some(guid));

        volumes_directory.terminate().await;
    }

    #[fuchsia::test]
    async fn test_deleted_encrypted_volume_while_mounted() {
        const VOLUME_NAME: &str = "encrypted";

        let device = DeviceHolder::new(FakeDevice::new(8192, 512));
        let filesystem = FxFilesystem::new_empty(device).await.unwrap();
        let crypt = Arc::new(new_insecure_crypt()) as Arc<dyn Crypt>;
        let blob_resupplied_count =
            Arc::new(PageRefaultCounter::new().expect("Failed to create PageRefaultCounter"));
        let volumes_directory = VolumesDirectory::new(
            root_volume(filesystem.clone()).await.unwrap(),
            Weak::new(),
            None,
            blob_resupplied_count,
            MemoryPressureConfig::default(),
        )
        .await
        .unwrap();
        volumes_directory
            .create_and_mount_volume(
                VOLUME_NAME,
                Some(crypt.clone()),
                false,
                CreateOptions::default(),
            )
            .await
            .expect("create encrypted volume failed");
        // We have the volume mounted so delete attempts should fail.
        assert!(
            FxfsError::AlreadyBound.matches(
                &volumes_directory
                    .remove_volume(VOLUME_NAME)
                    .await
                    .err()
                    .expect("Deleting volume should fail")
            )
        );
        volumes_directory.terminate().await;
        std::mem::drop(volumes_directory);
        filesystem.close().await.expect("close filesystem failed");
    }

    #[fuchsia::test]
    async fn test_mount_volume_using_volume_protocol() {
        let device = DeviceHolder::new(FakeDevice::new(8192, 512));
        let filesystem = FxFilesystem::new_empty(device).await.unwrap();
        let blob_resupplied_count =
            Arc::new(PageRefaultCounter::new().expect("Failed to create PageRefaultCounter"));
        let volumes_directory = VolumesDirectory::new(
            root_volume(filesystem.clone()).await.unwrap(),
            Weak::new(),
            None,
            blob_resupplied_count,
            MemoryPressureConfig::default(),
        )
        .await
        .unwrap();

        let crypt = Arc::new(new_insecure_crypt()) as Arc<dyn Crypt>;
        let store_id = {
            let vol = volumes_directory
                .create_and_mount_volume(
                    "encrypted",
                    Some(crypt.clone()),
                    false,
                    CreateOptions::default(),
                )
                .await
                .expect("create encrypted volume failed");
            vol.volume().store().store_object_id()
        };
        volumes_directory.lock().await.unmount(store_id).await.expect("unmount failed");

        let (volume_proxy, _scope) = serve_startup_volume_proxy(&volumes_directory, "encrypted");

        let (dir_proxy, dir_server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();

        let crypt_service = fxfs_crypt::CryptService::new();
        crypt_service
            .add_wrapping_key(0, fxfs_insecure_crypto::DATA_KEY.to_vec())
            .expect("add_wrapping_key failed");
        crypt_service
            .add_wrapping_key(1, fxfs_insecure_crypto::METADATA_KEY.to_vec())
            .expect("add_wrapping_key failed");
        crypt_service.set_active_key(KeyPurpose::Data, 0).expect("set_active_key failed");
        crypt_service.set_active_key(KeyPurpose::Metadata, 1).expect("set_active_key failed");
        let (client1, stream1) = create_request_stream();
        let (client2, stream2) = create_request_stream();

        join!(
            async {
                volume_proxy
                    .mount(
                        dir_server_end,
                        MountOptions { crypt: Some(client1), ..MountOptions::default() },
                    )
                    .await
                    .expect("mount (fidl) failed")
                    .expect("mount failed");

                open_file_checked(
                    &dir_proxy,
                    "root/test",
                    fio::PERM_READABLE | fio::PERM_WRITABLE | fio::Flags::FLAG_MAYBE_CREATE,
                    &Default::default(),
                )
                .await;

                // Attempting to mount again should fail with ALREADY_BOUND.
                let (_dir_proxy, dir_server_end) =
                    fidl::endpoints::create_proxy::<fio::DirectoryMarker>();

                assert_eq!(
                    Status::ok(
                        volume_proxy
                            .mount(
                                dir_server_end,
                                MountOptions { crypt: Some(client2), ..MountOptions::default() },
                            )
                            .await
                            .expect("mount (fidl) failed")
                            .expect_err("mount succeeded")
                    ),
                    Err(Status::ALREADY_BOUND)
                );

                std::mem::drop(dir_proxy);

                // The volume should get unmounted a short time later.
                let mut count = 0;
                loop {
                    if volumes_directory.mounted_volumes.lock().await.is_empty() {
                        break;
                    }
                    count += 1;
                    assert!(count <= 100);
                    fasync::Timer::new(Duration::from_millis(100)).await;
                }
            },
            async {
                crypt_service
                    .handle_request(fxfs_crypt::Services::Crypt(stream1))
                    .await
                    .expect("handle_request failed");
                crypt_service
                    .handle_request(fxfs_crypt::Services::Crypt(stream2))
                    .await
                    .expect("handle_request failed");
            }
        );
        // Make sure the background thread that actually calls terminate() on the volume finishes
        // before exiting the test. terminate() should be a no-op since we already verified
        // mounted_directories is empty, but the volume's terminate() future in the background task
        // may still be outstanding. As both the background task and VolumesDirectory::terminate()
        // hold the write lock, we use that to block until the background task has completed.
        volumes_directory.terminate().await;
    }

    #[fuchsia::test]
    #[ignore] // TODO(b/293917849) re-enable this test when de-flaked

    async fn test_volume_dir_races() {
        let device = DeviceHolder::new(FakeDevice::new(8192, 512));
        let filesystem = FxFilesystem::new_empty(device).await.unwrap();
        let blob_resupplied_count =
            Arc::new(PageRefaultCounter::new().expect("Failed to create PageRefaultCounter"));
        let volumes_directory = VolumesDirectory::new(
            root_volume(filesystem.clone()).await.unwrap(),
            Weak::new(),
            None,
            blob_resupplied_count,
            MemoryPressureConfig::default(),
        )
        .await
        .unwrap();

        let crypt = Arc::new(new_insecure_crypt()) as Arc<dyn Crypt>;
        let store_id = {
            let vol = volumes_directory
                .create_and_mount_volume(
                    "encrypted",
                    Some(crypt.clone()),
                    false,
                    CreateOptions::default(),
                )
                .await
                .expect("create encrypted volume failed");
            vol.volume().store().store_object_id()
        };
        volumes_directory.lock().await.unmount(store_id).await.expect("unmount failed");

        let (volume_proxy, _scope) = serve_startup_volume_proxy(&volumes_directory, "encrypted");

        let crypt_service = Arc::new(fxfs_crypt::CryptService::new());
        crypt_service
            .add_wrapping_key(0, fxfs_insecure_crypto::DATA_KEY.to_vec())
            .expect("add_wrapping_key failed");
        crypt_service
            .add_wrapping_key(1, fxfs_insecure_crypto::METADATA_KEY.to_vec())
            .expect("add_wrapping_key failed");
        crypt_service.set_active_key(KeyPurpose::Data, 0).expect("set_active_key failed");
        crypt_service.set_active_key(KeyPurpose::Metadata, 1).expect("set_active_key failed");
        let (client1, stream1) = create_request_stream();
        let (client2, stream2) = create_request_stream();
        let crypt_service_clone = crypt_service.clone();
        let crypt_task1 = fasync::Task::spawn(async move {
            crypt_service_clone
                .handle_request(fxfs_crypt::Services::Crypt(stream1))
                .await
                .expect("handle_request failed");
        });
        let crypt_task2 = fasync::Task::spawn(async move {
            crypt_service
                .handle_request(fxfs_crypt::Services::Crypt(stream2))
                .await
                .expect("handle_request failed");
        });

        // Create two tasks each of mount and remove, and one to recreate the volume, so that we get
        // to exercise a wide variety of concurrent actions.
        // Delay remove and create a bit, since mount is slower due to FIDL.
        join!(
            async {
                let (_dir_proxy, dir_server_end) =
                    fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
                if let Err(status) = volume_proxy
                    .mount(
                        dir_server_end,
                        MountOptions { crypt: Some(client1), ..MountOptions::default() },
                    )
                    .await
                    .expect("mount (fidl) failed")
                {
                    let status = Status::err_from_raw(status);
                    if status != Status::NOT_FOUND && status != Status::ALREADY_BOUND {
                        assert!(false, "Unexpected status {:}", status);
                    }
                }
            },
            async {
                let (_dir_proxy, dir_server_end) =
                    fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
                if let Err(status) = volume_proxy
                    .mount(
                        dir_server_end,
                        MountOptions { crypt: Some(client2), ..MountOptions::default() },
                    )
                    .await
                    .expect("mount (fidl) failed")
                {
                    let status = Status::err_from_raw(status);
                    if status != Status::NOT_FOUND && status != Status::ALREADY_BOUND {
                        assert!(false, "Unexpected status {:}", status);
                    }
                }
            },
            async {
                let volumes_directory = volumes_directory.clone();
                let wait_time = rand::random_range(0..5);
                fasync::Timer::new(Duration::from_millis(wait_time)).await;
                if let Err(err) = volumes_directory.remove_volume("encrypted").await {
                    assert!(
                        FxfsError::NotFound.matches(&err) || FxfsError::AlreadyBound.matches(&err),
                        "Unexpected error {:?}",
                        err
                    );
                }
            },
            async {
                let volumes_directory = volumes_directory.clone();
                let wait_time = rand::random_range(0..5);
                fasync::Timer::new(Duration::from_millis(wait_time)).await;
                if let Err(err) = volumes_directory.remove_volume("encrypted").await {
                    assert!(
                        FxfsError::NotFound.matches(&err) || FxfsError::AlreadyBound.matches(&err),
                        "Unexpected error {:?}",
                        err
                    );
                }
            },
            async {
                let volumes_directory = volumes_directory.clone();
                let wait_time = rand::random_range(0..5);
                fasync::Timer::new(Duration::from_millis(wait_time)).await;
                let mut guard = volumes_directory.lock().await;
                match guard
                    .create_or_mount_volume(
                        "encrypted",
                        Some(crypt.clone()),
                        Mode::Create { guid: None, low_32_bit_object_ids: false },
                        false,
                    )
                    .await
                {
                    Ok(vol) => {
                        let store_id = vol.volume().store().store_object_id();
                        std::mem::drop(vol);
                        guard.unmount(store_id).await.expect("unmount failed");
                    }
                    Err(err) => {
                        assert!(
                            FxfsError::AlreadyExists.matches(&err)
                                || FxfsError::AlreadyBound.matches(&err),
                            "Unexpected error {:?}",
                            err
                        );
                    }
                }
            }
        );
        std::mem::drop(crypt_task1);
        std::mem::drop(crypt_task2);
        // Make sure the background thread that actually calls terminate() on the volume finishes
        // before exiting the test. terminate() should be a no-op since we already verified
        // mounted_directories is empty, but the volume's terminate() future in the background task
        // may still be outstanding. As both the background task and VolumesDirectory::terminate()
        // hold the write lock, we use that to block until the background task has completed.
        volumes_directory.terminate().await;
    }

    #[fuchsia::test]
    async fn test_shutdown_volume() {
        let device = DeviceHolder::new(FakeDevice::new(8192, 512));
        let filesystem = FxFilesystem::new_empty(device).await.unwrap();
        let blob_resupplied_count =
            Arc::new(PageRefaultCounter::new().expect("Failed to create PageRefaultCounter"));
        let volumes_directory = VolumesDirectory::new(
            root_volume(filesystem.clone()).await.unwrap(),
            Weak::new(),
            None,
            blob_resupplied_count,
            MemoryPressureConfig::default(),
        )
        .await
        .unwrap();

        let crypt = Arc::new(new_insecure_crypt()) as Arc<dyn Crypt>;
        let vol = volumes_directory
            .create_and_mount_volume(
                "encrypted",
                Some(crypt.clone()),
                false,
                CreateOptions::default(),
            )
            .await
            .expect("create encrypted volume failed");

        let (dir_proxy, dir_server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();

        volumes_directory.serve_volume(&vol, dir_server_end, false).expect("serve_volume failed");

        let admin_proxy = connect_to_protocol_at_dir_svc::<AdminMarker>(&dir_proxy)
            .expect("Unable to connect to admin service");

        admin_proxy.shutdown().await.expect("shutdown failed");

        assert!(volumes_directory.mounted_volumes.lock().await.is_empty());
    }

    #[fuchsia::test]
    async fn test_volume_debug_clear_caches() {
        let device = DeviceHolder::new(FakeDevice::new(8192, 512));
        let filesystem = FxFilesystem::new_empty(device).await.unwrap();
        let blob_resupplied_count =
            Arc::new(PageRefaultCounter::new().expect("Failed to create PageRefaultCounter"));
        let volumes_directory = VolumesDirectory::new(
            root_volume(filesystem.clone()).await.unwrap(),
            Weak::new(),
            None,
            blob_resupplied_count,
            MemoryPressureConfig::default(),
        )
        .await
        .unwrap();

        let crypt = Arc::new(new_insecure_crypt()) as Arc<dyn Crypt>;
        let vol = volumes_directory
            .create_and_mount_volume("test_vol", Some(crypt), false, CreateOptions::default())
            .await
            .expect("create encrypted volume failed");

        let (dir_proxy, dir_server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        volumes_directory.serve_volume(&vol, dir_server_end, false).expect("serve_volume failed");

        let (root, root_server_end) = create_proxy::<fio::DirectoryMarker>();
        vol.root().clone().serve(fio::PERM_READABLE | fio::PERM_WRITABLE, root_server_end);
        let file = open_file_checked(
            &root,
            "foo",
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;

        file.write(b"Hello, world!").await.expect("write fidl failed").expect("write failed");
        file.sync()
            .await
            .expect("sync fidl failed")
            .map_err(Status::err_from_raw)
            .expect("sync failed");

        let (_, attrs) = file
            .get_attributes(fio::NodeAttributesQuery::ID)
            .await
            .expect("get_attributes fidl failed")
            .expect("get_attributes failed");
        let file_id = attrs.id.expect("missing file id");

        let store = vol.volume().store();
        // Flush the store so that records are compacted into a PersistentLayer.
        store.flush().await.expect("flush failed");

        // Clear the tree cache so the lookup below misses TreeCache and reads from the
        // PersistentLayer on disk, populating both the layer chunk cache and TreeCache.
        store.tree().clear_cache();
        let _ = store
            .tree()
            .find(&fxfs::object_store::ObjectKey::object(file_id))
            .await
            .expect("tree find failed");

        let layer_object_id =
            store.tree().immutable_layer_set().layers[0].handle().unwrap().object_id();

        // Verify all caches are populated prior to ClearCaches.
        assert!(vol.volume().dirent_cache().len() > 0);
        assert!(store.tree().cache_len() > 0);
        assert!(store.key_manager().get(file_id).await.unwrap().is_some());
        assert!(
            filesystem.root_store().key_manager().get(layer_object_id).await.unwrap().is_some()
        );

        let debug_proxy = connect_to_protocol_at_dir_svc::<DebugMarker>(&dir_proxy)
            .expect("Unable to connect to debug service");

        debug_proxy
            .clear_caches()
            .await
            .expect("clear_caches fidl failed")
            .expect("clear_caches failed");

        // Verify dirent cache and TreeCache are cleared.
        assert_eq!(vol.volume().dirent_cache().len(), 0);
        assert_eq!(store.tree().cache_len(), 0);
        // Verify non-permanent file key is evicted from the volume store.
        assert!(store.key_manager().get(file_id).await.unwrap().is_none());

        // Also clear caches on the top-level VolumesDirectory and verify permanent layer keys in
        // root_store are preserved so reading from the encrypted volume continues to work.
        volumes_directory.clear_caches().await;
        assert!(
            filesystem.root_store().key_manager().get(layer_object_id).await.unwrap().is_some()
        );
        let _ = store
            .tree()
            .find(&fxfs::object_store::ObjectKey::object(file_id))
            .await
            .expect("tree find after clear_caches failed");
    }

    #[fuchsia::test]
    async fn test_byte_limit_persistence() {
        const BYTES_LIMIT_1: u64 = 123456;
        const BYTES_LIMIT_2: u64 = 456789;
        const VOLUME_NAME: &str = "A";
        let mut device = DeviceHolder::new(FakeDevice::new(8192, 512));
        {
            let filesystem = FxFilesystem::new_empty(device).await.unwrap();
            let blob_resupplied_count =
                Arc::new(PageRefaultCounter::new().expect("Failed to create PageRefaultCounter"));
            let volumes_directory = VolumesDirectory::new(
                root_volume(filesystem.clone()).await.unwrap(),
                Weak::new(),
                None,
                blob_resupplied_count,
                MemoryPressureConfig::default(),
            )
            .await
            .unwrap();

            volumes_directory
                .create_and_mount_volume(VOLUME_NAME, None, false, CreateOptions::default())
                .await
                .expect("create unencrypted volume failed");

            let (volume_proxy, _scope) =
                serve_startup_volume_proxy(&volumes_directory, VOLUME_NAME);

            volume_proxy.set_limit(BYTES_LIMIT_1).await.unwrap().expect("To set limits");
            {
                let limits = (filesystem.allocator() as Arc<Allocator>).owner_byte_limits();
                assert_eq!(limits.len(), 1);
                assert_eq!(limits[0].1, BYTES_LIMIT_1);
            }

            volume_proxy.set_limit(BYTES_LIMIT_2).await.unwrap().expect("To set limits");
            {
                let limits = (filesystem.allocator() as Arc<Allocator>).owner_byte_limits();
                assert_eq!(limits.len(), 1);
                assert_eq!(limits[0].1, BYTES_LIMIT_2);
            }
            std::mem::drop(volume_proxy);
            volumes_directory.terminate().await;
            std::mem::drop(volumes_directory);
            filesystem.close().await.expect("close filesystem failed");
            device = filesystem.take_device().await;
        }
        device.ensure_unique();
        device.reopen(false);
        {
            let filesystem = FxFilesystem::open(device as DeviceHolder).await.unwrap();
            fsck(filesystem.clone()).await.expect("Fsck");
            let blob_resupplied_count =
                Arc::new(PageRefaultCounter::new().expect("Failed to create PageRefaultCounter"));
            let volumes_directory = VolumesDirectory::new(
                root_volume(filesystem.clone()).await.unwrap(),
                Weak::new(),
                None,
                blob_resupplied_count,
                MemoryPressureConfig::default(),
            )
            .await
            .unwrap();
            {
                let limits = (filesystem.allocator() as Arc<Allocator>).owner_byte_limits();
                assert_eq!(limits.len(), 1);
                assert_eq!(limits[0].1, BYTES_LIMIT_2);
            }
            volumes_directory.remove_volume(VOLUME_NAME).await.expect("Volume deletion failed");
            {
                let limits = (filesystem.allocator() as Arc<Allocator>).owner_byte_limits();
                assert_eq!(limits.len(), 0);
            }
            volumes_directory.terminate().await;
            std::mem::drop(volumes_directory);
            filesystem.close().await.expect("close filesystem failed");
            device = filesystem.take_device().await;
        }
        device.ensure_unique();
        device.reopen(false);
        let filesystem = FxFilesystem::open(device as DeviceHolder).await.unwrap();
        fsck(filesystem.clone()).await.expect("Fsck");
        let limits = (filesystem.allocator() as Arc<Allocator>).owner_byte_limits();
        assert_eq!(limits.len(), 0);
    }

    struct VolumeInfo {
        _scope: vfs::ExecutionScope,
        volume_proxy: VolumeProxy,
        file_proxy: fio::FileProxy,
    }

    impl VolumeInfo {
        async fn new(volumes_directory: &Arc<VolumesDirectory>, name: &'static str) -> Self {
            let volume = volumes_directory
                .create_and_mount_volume(name, None, false, CreateOptions::default())
                .await
                .expect("create unencrypted volume failed");

            let (volume_dir_proxy, dir_server_end) =
                fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
            volumes_directory
                .serve_volume(&volume, dir_server_end, false)
                .expect("serve_volume failed");

            let (volume_proxy, _scope) = serve_startup_volume_proxy(&volumes_directory, name);

            let (root_proxy, root_server_end) =
                fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
            volume_dir_proxy
                .open(
                    "root",
                    fio::PERM_READABLE | fio::PERM_WRITABLE | fio::Flags::PROTOCOL_DIRECTORY,
                    &Default::default(),
                    root_server_end.into_channel(),
                )
                .expect("Failed to open volume root");

            let file_proxy = open_file_checked(
                &root_proxy,
                "foo",
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE
                    | fio::Flags::PROTOCOL_FILE,
                &Default::default(),
            )
            .await;
            VolumeInfo { _scope, volume_proxy, file_proxy }
        }
    }

    #[fuchsia::test]
    async fn test_limit_bytes() {
        const BYTES_LIMIT: u64 = 262_144; // 256KiB
        const BLOCK_SIZE: BlockSize = BlockSize::SIZE_512B;
        let device = DeviceHolder::new(FakeDevice::new(8192, BLOCK_SIZE.get() as u32));
        let filesystem = FxFilesystem::new_empty(device).await.unwrap();
        let blob_resupplied_count =
            Arc::new(PageRefaultCounter::new().expect("Failed to create PageRefaultCounter"));
        let volumes_directory = VolumesDirectory::new(
            root_volume(filesystem.clone()).await.unwrap(),
            Weak::new(),
            None,
            blob_resupplied_count,
            MemoryPressureConfig::default(),
        )
        .await
        .unwrap();

        let vol = VolumeInfo::new(&volumes_directory, "foo").await;
        let old_info = {
            let (status, info) = vol.file_proxy.query_filesystem().await.expect("Getting fs info");
            assert_eq!(status, zx::sys::ZX_OK);
            let info = info.unwrap();
            // With no limit set, the total filesystem size should be returned.
            assert!(info.total_bytes > BYTES_LIMIT);
            info
        };

        vol.volume_proxy.set_limit(BYTES_LIMIT).await.unwrap().expect("To set limits");
        {
            let (status, info) = vol.file_proxy.query_filesystem().await.expect("Getting fs info");
            assert_eq!(status, zx::sys::ZX_OK);
            let new_info = info.unwrap();
            assert_eq!(new_info.total_bytes, BYTES_LIMIT);
            // Now since the limit is the volume limit, the space used should be the volume usage,
            // which should be strictly less than the filesystem.
            assert!(new_info.used_bytes < old_info.used_bytes);
        }

        let zeros = vec![0u8; BLOCK_SIZE.get() as usize];
        // First write should succeed.
        assert_eq!(
            vol.file_proxy
                .write(&zeros)
                .await
                .expect("Failed Write message")
                .expect("Failed write"),
            BLOCK_SIZE
        );
        // Likely to run out of space before writing the full limit due to overheads.
        for _ in (BLOCK_SIZE.get()..BYTES_LIMIT).step_by(BLOCK_SIZE.get() as usize) {
            match vol.file_proxy.write(&zeros).await.expect("Failed Write message") {
                Err(_) => break,
                Ok(b) if b < BLOCK_SIZE => break,
                _ => (),
            };
        }

        // Any further writes should fail with out of space.
        assert_eq!(
            vol.file_proxy
                .write(&zeros)
                .await
                .expect("Failed write message")
                .expect_err("Write should have been limited"),
            Status::NO_SPACE.into_raw()
        );

        // Double the limit and try again. We should have write space again.
        vol.volume_proxy.set_limit(BYTES_LIMIT * 2).await.unwrap().expect("To set limits");
        assert_eq!(
            vol.file_proxy
                .write(&zeros)
                .await
                .expect("Failed Write message")
                .expect("Failed write"),
            BLOCK_SIZE
        );

        vol.file_proxy.close().await.unwrap().expect("Failed to close file");
        volumes_directory.terminate().await;
        std::mem::drop(volumes_directory);
        filesystem.close().await.expect("close filesystem failed");
    }

    #[fuchsia::test]
    async fn test_limit_bytes_two_hit_device_limit() {
        const BYTES_LIMIT: u64 = 3_145_728; // 3MiB
        const BLOCK_SIZE: BlockSize = BlockSize::SIZE_512B;
        const BLOCK_COUNT: u64 = 8192;
        let device = DeviceHolder::new(FakeDevice::new(BLOCK_COUNT, BLOCK_SIZE.get() as u32));
        let filesystem = FxFilesystem::new_empty(device).await.unwrap();
        let blob_resupplied_count =
            Arc::new(PageRefaultCounter::new().expect("Failed to create PageRefaultCounter"));
        let volumes_directory = VolumesDirectory::new(
            root_volume(filesystem.clone()).await.unwrap(),
            Weak::new(),
            None,
            blob_resupplied_count,
            MemoryPressureConfig::default(),
        )
        .await
        .unwrap();

        let a = VolumeInfo::new(&volumes_directory, "foo").await;
        let b = VolumeInfo::new(&volumes_directory, "bar").await;
        a.volume_proxy.set_limit(BYTES_LIMIT).await.unwrap().expect("To set limits");
        b.volume_proxy.set_limit(BYTES_LIMIT).await.unwrap().expect("To set limits");
        let mut a_written: u64 = 0;
        let mut b_written: u64 = 0;

        // Write chunks of BLOCK_SIZE.
        let zeros = vec![0u8; BLOCK_SIZE.get() as usize];

        // First write should succeed for both.
        assert_eq!(
            a.file_proxy.write(&zeros).await.expect("Failed Write message").expect("Failed write"),
            BLOCK_SIZE
        );
        a_written += BLOCK_SIZE;
        assert_eq!(
            b.file_proxy.write(&zeros).await.expect("Failed Write message").expect("Failed write"),
            BLOCK_SIZE
        );
        b_written += BLOCK_SIZE;

        // Likely to run out of space before writing the full limit due to overheads.
        for _ in (BLOCK_SIZE.get()..BYTES_LIMIT).step_by(BLOCK_SIZE.get() as usize) {
            match a.file_proxy.write(&zeros).await.expect("Failed Write message") {
                Err(_) => break,
                Ok(bytes) => {
                    a_written += bytes;
                    if bytes < BLOCK_SIZE {
                        break;
                    }
                }
            };
        }
        // Any further writes should fail with out of space.
        assert_eq!(
            a.file_proxy
                .write(&zeros)
                .await
                .expect("Failed write message")
                .expect_err("Write should have been limited"),
            Status::NO_SPACE.into_raw()
        );

        // Now write to the second volume. Likely to run out of space before writing the full limit
        // due to overheads.
        for _ in (BLOCK_SIZE.get()..BYTES_LIMIT).step_by(BLOCK_SIZE.get() as usize) {
            match b.file_proxy.write(&zeros).await.expect("Failed Write message") {
                Err(_) => break,
                Ok(bytes) => {
                    b_written += bytes;
                    if bytes < BLOCK_SIZE {
                        break;
                    }
                }
            };
        }
        // Any further writes should fail with out of space.
        assert_eq!(
            b.file_proxy
                .write(&zeros)
                .await
                .expect("Failed write message")
                .expect_err("Write should have been limited"),
            Status::NO_SPACE.into_raw()
        );

        // Second volume should have failed very early.
        assert!(BLOCK_SIZE * BLOCK_COUNT - BYTES_LIMIT >= b_written);
        // First volume should have gotten further.
        assert!(BLOCK_SIZE * BLOCK_COUNT - BYTES_LIMIT <= a_written);

        a.file_proxy.close().await.unwrap().expect("Failed to close file");
        b.file_proxy.close().await.unwrap().expect("Failed to close file");
        volumes_directory.terminate().await;
        std::mem::drop(volumes_directory);
        filesystem.close().await.expect("close filesystem failed");
    }

    #[fuchsia::test(threads = 10)]
    async fn test_profile_start() {
        const PREMOUNT_BLOB: &str = "premount_blob";
        const PREMOUNT_NOBLOB: &str = "premount_noblob";
        const LIVE_BLOB: &str = "live_blob";
        const LIVE_NOBLOB: &str = "live_noblob";

        const RECORDING_NAME: &str = "foo";

        let device = {
            let device = DeviceHolder::new(FakeDevice::new(8192, 512));
            let filesystem = FxFilesystem::new_empty(device).await.unwrap();
            let blob_resupplied_count =
                Arc::new(PageRefaultCounter::new().expect("Failed to create PageRefaultCounter"));
            let volumes_directory = VolumesDirectory::new(
                root_volume(filesystem.clone()).await.unwrap(),
                Weak::new(),
                None,
                blob_resupplied_count,
                MemoryPressureConfig::default(),
            )
            .await
            .unwrap();
            volumes_directory
                .create_and_mount_volume(PREMOUNT_BLOB, None, true, CreateOptions::default())
                .await
                .unwrap();
            volumes_directory
                .create_and_mount_volume(PREMOUNT_NOBLOB, None, false, CreateOptions::default())
                .await
                .unwrap();
            volumes_directory
                .create_and_mount_volume(LIVE_BLOB, None, true, CreateOptions::default())
                .await
                .unwrap();
            volumes_directory
                .create_and_mount_volume(LIVE_NOBLOB, None, false, CreateOptions::default())
                .await
                .unwrap();

            volumes_directory.terminate().await;
            std::mem::drop(volumes_directory);
            filesystem.close().await.expect("Filesystem close");
            filesystem.take_device().await
        };

        device.ensure_unique();
        device.reopen(false);
        let device = {
            let filesystem = FxFilesystem::open(device as DeviceHolder).await.unwrap();
            let blob_resupplied_count =
                Arc::new(PageRefaultCounter::new().expect("Failed to create PageRefaultCounter"));
            let volumes_directory = VolumesDirectory::new(
                root_volume(filesystem.clone()).await.unwrap(),
                Weak::new(),
                None,
                blob_resupplied_count,
                MemoryPressureConfig::default(),
            )
            .await
            .unwrap();

            // Premount two volumes.
            let _premount_blob = volumes_directory
                .mount_volume(PREMOUNT_BLOB, None, true)
                .await
                .expect("Reopen volume");
            let _premount_noblob = volumes_directory
                .mount_volume(PREMOUNT_NOBLOB, None, false)
                .await
                .expect("Reopen volume");

            // Start the recording, let it run a really long time, it doesn't need to end for this
            // test. If it does wait this long then it should trigger test timeouts.
            volumes_directory
                .clone()
                .record_and_replay_profile(None, RECORDING_NAME.to_owned(), 600)
                .await
                .expect("Recording");

            // Live mount two volumes.
            let _live_blob =
                volumes_directory.mount_volume(LIVE_BLOB, None, true).await.expect("Reopen volume");
            let _live_noblob = volumes_directory
                .mount_volume(LIVE_NOBLOB, None, false)
                .await
                .expect("Reopen volume");

            // Wait for the recordings to finish.
            volumes_directory.stop_profile_tasks().await;

            volumes_directory.terminate().await;
            std::mem::drop(volumes_directory);
            filesystem.close().await.expect("Filesystem close");
            filesystem.take_device().await
        };

        device.ensure_unique();
        device.reopen(false);
        let filesystem = FxFilesystem::open(device as DeviceHolder).await.unwrap();
        {
            let blob_resupplied_count =
                Arc::new(PageRefaultCounter::new().expect("Failed to create PageRefaultCounter"));
            let volumes_directory = VolumesDirectory::new(
                root_volume(filesystem.clone()).await.unwrap(),
                Weak::new(),
                None,
                blob_resupplied_count,
                MemoryPressureConfig::default(),
            )
            .await
            .unwrap();

            let _premount_blob = volumes_directory
                .mount_volume(PREMOUNT_BLOB, None, true)
                .await
                .expect("Reopen volume");
            let _premount_noblob = volumes_directory
                .mount_volume(PREMOUNT_NOBLOB, None, false)
                .await
                .expect("Reopen volume");
            let _live_blob =
                volumes_directory.mount_volume(LIVE_BLOB, None, true).await.expect("Reopen volume");
            let _live_noblob = volumes_directory
                .mount_volume(LIVE_NOBLOB, None, false)
                .await
                .expect("Reopen volume");

            // Verify which recordings ran based on the saved recordings.
            volumes_directory
                .delete_profile(PREMOUNT_BLOB, RECORDING_NAME)
                .await
                .expect("Finding profile to delete.");
            volumes_directory
                .delete_profile(PREMOUNT_NOBLOB, RECORDING_NAME)
                .await
                .expect("Finding profile to delete.");
            volumes_directory
                .delete_profile(LIVE_BLOB, RECORDING_NAME)
                .await
                .expect("Finding profile to delete.");
            volumes_directory
                .delete_profile(LIVE_NOBLOB, RECORDING_NAME)
                .await
                .expect("Finding profile to delete.");

            volumes_directory.terminate().await;
        }

        filesystem.close().await.expect("Filesystem close");
    }

    #[fuchsia::test(threads = 10)]
    async fn test_profile_stop() {
        let device = DeviceHolder::new(FakeDevice::new(8192, 512));
        let filesystem = FxFilesystem::new_empty(device).await.unwrap();
        let blob_resupplied_count =
            Arc::new(PageRefaultCounter::new().expect("Failed to create PageRefaultCounter"));
        let volumes_directory = VolumesDirectory::new(
            root_volume(filesystem.clone()).await.unwrap(),
            Weak::new(),
            None,
            blob_resupplied_count,
            MemoryPressureConfig::default(),
        )
        .await
        .unwrap();
        let volume = volumes_directory
            .create_and_mount_volume("foo", None, true, CreateOptions::default())
            .await
            .unwrap();

        // Run the recording with no time at all and ensure that it still shuts down properly.
        volumes_directory
            .clone()
            .record_and_replay_profile(None, "foo".to_owned(), 0)
            .await
            .expect("Recording");

        // Delete will succeed once the profile is completed.
        while volumes_directory.delete_profile("foo", "foo").await.is_err() {
            fasync::Timer::new(Duration::from_millis(10)).await;
        }

        std::mem::drop(volume);
        volumes_directory.terminate().await;
        std::mem::drop(volumes_directory);
        filesystem.close().await.expect("Filesystem close");
    }

    #[fuchsia::test(threads = 10)]
    async fn test_delete_profile() {
        let device = DeviceHolder::new(FakeDevice::new(8192, 512));
        let filesystem = FxFilesystem::new_empty(device).await.unwrap();
        let blob_resupplied_count =
            Arc::new(PageRefaultCounter::new().expect("Failed to create PageRefaultCounter"));
        let volumes_directory = VolumesDirectory::new(
            root_volume(filesystem.clone()).await.unwrap(),
            Weak::new(),
            None,
            blob_resupplied_count,
            MemoryPressureConfig::default(),
        )
        .await
        .unwrap();
        let volume = volumes_directory
            .create_and_mount_volume("foo", None, true, CreateOptions::default())
            .await
            .unwrap();

        volumes_directory
            .clone()
            .record_and_replay_profile(None, "foo".to_owned(), 600)
            .await
            .expect("Recording");

        // Deletion fails during in-flight recording.
        assert_eq!(
            volumes_directory.delete_profile("foo", "foo").await.expect_err("File shouldn't exist"),
            Status::SHOULD_WAIT
        );

        volumes_directory.stop_profile_tasks().await;

        // Missing volume name.
        assert_eq!(
            volumes_directory.delete_profile("bar", "foo").await.expect_err("File shouldn't exist"),
            Status::NOT_FOUND
        );

        // Missing Profile name.
        assert_eq!(
            volumes_directory.delete_profile("foo", "bar").await.expect_err("File shouldn't exist"),
            Status::NOT_FOUND
        );

        // Deletion should now succeed as the profile will be placed as part of `finish_profiling()`
        volumes_directory.delete_profile("foo", "foo").await.expect("Deleting");

        // Deletion fails as the file shouldn't exist anymore.
        assert_eq!(
            volumes_directory.delete_profile("foo", "foo").await.expect_err("File shouldn't exist"),
            Status::NOT_FOUND
        );

        std::mem::drop(volume);
        volumes_directory.terminate().await;
        std::mem::drop(volumes_directory);
        filesystem.close().await.expect("Filesystem close");
    }

    #[fuchsia::test(threads = 10)]
    async fn test_profile_start_single_volume() {
        const TEST_VOLUME: &str = "test_1234";
        const TEST_RECORDING: &str = "test_5678";
        let crypt = Arc::new(new_insecure_crypt()) as Arc<dyn Crypt>;

        let fixture = TestFixture::new().await;
        {
            let volumes_directory = fixture.volumes_directory();
            // Start recording for volume that doesn't exist.
            assert_eq!(
                volumes_directory
                    .record_and_replay_profile(
                        Some(TEST_VOLUME.to_owned()),
                        TEST_RECORDING.to_owned(),
                        1
                    )
                    .await
                    .expect_err("Volumes doesn't exist yet"),
                Status::NOT_FOUND
            );

            // Create the volume unmounted.
            {
                let volume = volumes_directory
                    .create_and_mount_volume(
                        TEST_VOLUME,
                        Some(crypt.clone()),
                        false,
                        CreateOptions::default(),
                    )
                    .await
                    .unwrap();
                volumes_directory
                    .lock()
                    .await
                    .unmount(volume.volume().store().store_object_id())
                    .await
                    .expect("unmount failed");
            }

            // Start recording for volume that is not mounted.
            assert_eq!(
                volumes_directory
                    .record_and_replay_profile(
                        Some(TEST_VOLUME.to_owned()),
                        TEST_RECORDING.to_owned(),
                        1
                    )
                    .await
                    .expect_err("Volumes doesn't exist yet"),
                Status::UNAVAILABLE
            );

            // Remount the volume and try again.
            let volume = volumes_directory
                .mount_volume(TEST_VOLUME, Some(crypt.clone()), false)
                .await
                .expect("Remount volume");
            volumes_directory
                .record_and_replay_profile(
                    Some(TEST_VOLUME.to_owned()),
                    TEST_RECORDING.to_owned(),
                    1,
                )
                .await
                .expect("Starting recording");

            // Stop the recording and check that it was created on the new volume.
            volumes_directory.stop_profile_tasks().await;
            {
                let profile_dir = volume.volume().get_profile_directory().await.unwrap();
                assert!(profile_dir.lookup(TEST_RECORDING).await.unwrap().is_some());
            }

            // Should be no recording for the other volume.
            {
                let profile_dir = fixture.volume().volume().get_profile_directory().await.unwrap();
                assert!(profile_dir.lookup(TEST_RECORDING).await.unwrap().is_none());
            }
        }
        fixture.close().await;
    }

    #[fuchsia::test(threads = 10)]
    async fn test_delete_volume_while_flushing() {
        let device = DeviceHolder::new(FakeDevice::new(8192, 512));
        let filesystem = FxFilesystem::new_empty(device).await.unwrap();
        let blob_resupplied_count =
            Arc::new(PageRefaultCounter::new().expect("Failed to create PageRefaultCounter"));
        let volumes_directory = VolumesDirectory::new(
            root_volume(filesystem.clone()).await.unwrap(),
            Weak::new(),
            None,
            blob_resupplied_count,
            MemoryPressureConfig::default(),
        )
        .await
        .unwrap();
        let name = "vol";
        let volume = volumes_directory
            .create_and_mount_volume(name, None, false, CreateOptions::default())
            .await
            .unwrap();
        let mut transaction = filesystem
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(
                    volume.volume().store().store_object_id(),
                    volume.root_dir().directory().object_id()
                )],
                Options::default(),
            )
            .await
            .unwrap();
        volume
            .root_dir()
            .directory()
            .create_child_file(&mut transaction, "foo")
            .await
            .expect("create_child_file failed");
        transaction.commit().await.expect("commit failed");
        volumes_directory
            .lock()
            .await
            .unmount(volume.volume().store().store_object_id())
            .await
            .expect("unmount failed");

        let filesystem_clone = filesystem.clone();
        let filesystem_clone2 = filesystem.clone();
        let volumes_directory_clone1 = volumes_directory.clone();
        let volumes_directory_clone2 = volumes_directory.clone();
        let root_store_object_id = filesystem.root_store().store_object_id();
        let store_info_object_id = volume.volume().store().store_info_handle_object_id().unwrap();
        join!(
            async move {
                // Take a lock that the EndFlush transaction requires, so we interleave removing
                // the volume between StartFlush and EndFlush.  Release it on a timer since once the
                // volume is deleted, `remove_volume` needs to take this lock as well to tombstone
                // the store info.
                let _guard = filesystem_clone2
                    .lock_manager()
                    .read_lock(lock_keys![LockKey::object(
                        root_store_object_id,
                        store_info_object_id,
                    )])
                    .await;
                fasync::Timer::new(Duration::from_millis(200)).await;
            },
            async move {
                filesystem_clone.journal().force_compact().await.expect("Compact failed");
            },
            async move {
                if let Err(e) = volumes_directory_clone1.remove_volume(name).await {
                    if !FxfsError::NotFound.matches(&e) {
                        panic!("remove_volume failed: {e:?}");
                    }
                }
            },
            async move {
                if let Err(e) = volumes_directory_clone2.remove_volume(name).await {
                    if !FxfsError::NotFound.matches(&e) {
                        panic!("remove_volume failed: {e:?}");
                    }
                }
            },
        );
        volumes_directory.terminate().await;
        std::mem::drop(volumes_directory);
        filesystem.close().await.expect("Filesystem close");
    }

    // This mostly just ensures that we exercise the code path. It was added because the path at
    // one point contained a deadlock.
    #[fuchsia::test(threads = 10)]
    async fn test_flush_before_mark_dirty_under_critical_memory_pressure() {
        let fixture = TestFixture::new().await;
        // Memory is critical, and it's always our fault.
        let _ = fixture
            .memory_pressure_proxy()
            .on_level_changed(MemoryPressureLevel::Critical)
            .await
            .expect("memory pressure FIDL");
        fixture.volumes_directory().max_dirty_bytes_when_critical.store(1, Ordering::Relaxed);

        let root = fixture.root();
        let file = open_file_checked(
            &root,
            "foo",
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;

        file.resize((PAGE_SIZE * 2).into()).await.expect("resize (FIDL)").expect("resize failed");
        // The above resize creates zero pages which aren't dirty pages and don't contribute to the
        // dirty bytes count but still need to be flushed. The first write below will cross the
        // `max_dirty_bytes_when_critical` threshold but `minimize_memory` won't flush the file
        // because the zero pages aren't dirty pages. The second write below will also cross the
        // `max_dirty_bytes_when_critical` threshold and since the first write dirtied a page, a
        // flush will occur. The flush cleans the 2nd page which is a zero page and the kernel is
        // actively trying to dirty. The kernel doesn't end up dirtying the 2nd page because it's
        // concerned that the clean and dirty raced so it issues another dirty request for the same
        // page. The duplicate dirty request again crosses the `max_dirty_bytes_when_critical`
        // threshold. The file thinks that it has a dirty page so `minimize_memory` flushes the
        // file. `zx_pager_query_dirty_ranges` doesn't return any pages because the kernel didn't
        // actually dirty the page that fxfs thought it did. This causes only the file metadata to
        // be flushed and the dirty page count to not be reduced. The 2nd page finally gets dirtied
        // and now fxfs thinks it has 2 dirty pages instead of 1. `PagedObjectHandle` knows that
        // this can happen and will fix up the counts on the next flush. This test doesn't want to
        // deal with all of that so it syncs here to clean the zero pages that would cause that
        // mess.
        file.sync().await.expect("Failed to make sync call").expect("sync failed");

        let vmo = file
            .get_backing_memory(fio::VmoFlags::READ | fio::VmoFlags::WRITE)
            .await
            .expect("get_backing_memory (FIDL)")
            .expect("get_backing_memory");

        let buf = [0xAAu8];
        // One call to get dirty bytes over 0, the second to force a flush during mark_dirty.
        vmo.write(&buf, 0).expect("Writing to create dirty bytes");
        let before = fixture.volumes_directory().pager_dirty_bytes_count.load();
        vmo.write(&buf, PAGE_SIZE.get()).expect("Writing to force a flush during mark_dirty");
        // This is still the page size because we forced a flush of the first write during the
        // second write.
        assert_eq!(fixture.volumes_directory().pager_dirty_bytes_count.load(), before,);

        fixture.close().await;
    }

    #[fuchsia::test(threads = 10)]
    async fn test_delete_crypt_for_volume() {
        let device = DeviceHolder::new(FakeDevice::new(8192, 512));
        let filesystem = FxFilesystem::new_empty(device).await.unwrap();
        let store_id;
        {
            let blob_resupplied_count =
                Arc::new(PageRefaultCounter::new().expect("Failed to create PageRefaultCounter"));
            let volumes_directory = VolumesDirectory::new(
                root_volume(filesystem.clone()).await.unwrap(),
                Weak::new(),
                None,
                blob_resupplied_count,
                MemoryPressureConfig::default(),
            )
            .await
            .unwrap();
            let name = "vol";
            let crypt = Arc::new(new_insecure_crypt());
            let volume = volumes_directory
                .create_and_mount_volume(name, Some(crypt.clone()), false, CreateOptions::default())
                .await
                .unwrap();
            store_id = volume.volume().store().store_object_id();
            // Make sure the volume has some journaled mutations.
            let mut transaction = filesystem
                .root_store()
                .new_transaction(
                    lock_keys![LockKey::object(
                        volume.volume().store().store_object_id(),
                        volume.root_dir().directory().object_id()
                    )],
                    Options::default(),
                )
                .await
                .unwrap();
            volume
                .root_dir()
                .directory()
                .create_child_file(&mut transaction, "foo")
                .await
                .expect("create_child_file failed");
            transaction.commit().await.expect("commit failed");

            let (volume_dir_proxy, dir_server_end) =
                fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
            volumes_directory
                .serve_volume(&volume, dir_server_end, false)
                .expect("serve_volume failed");
            let (root_dir, root_server_end) =
                fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
            volume_dir_proxy
                .open(
                    "root",
                    fio::PERM_READABLE | fio::PERM_WRITABLE | fio::Flags::PROTOCOL_DIRECTORY,
                    &Default::default(),
                    root_server_end.into_channel(),
                )
                .expect("Failed to open volume root");

            let filesystem_clone = filesystem.clone();
            join!(
                async move {
                    filesystem_clone.journal().force_compact().await.expect("Compact failed");
                },
                async move {
                    let mut i = 0;
                    while let Ok(_) = fuchsia_fs::directory::open_file(
                        &root_dir,
                        &format!("foo{i}"),
                        fio::Flags::FLAG_MAYBE_CREATE | fio::PERM_READABLE,
                    )
                    .await
                    {
                        i += 1;
                    }
                },
                async move {
                    crypt.shutdown();
                },
            );

            // Make sure we can still ask the volume to shutdown.
            let (admin_proxy, server_end) =
                fidl::endpoints::create_proxy::<fidl_fuchsia_fs::AdminMarker>();
            volume_dir_proxy
                .open(
                    &format!("svc/{}", fidl_fuchsia_fs::AdminMarker::PROTOCOL_NAME),
                    fio::Flags::PROTOCOL_SERVICE,
                    &Default::default(),
                    server_end.into(),
                )
                .expect("Failed to open Admin connection");
            admin_proxy.shutdown().await.expect("shutdown failed");

            volumes_directory.terminate().await;
        }
        filesystem.close().await.expect("Filesystem close");
        let device = filesystem.take_device().await;
        device.reopen(false);
        let filesystem = FxFilesystem::open(device).await.expect("open failed");
        let options = FsckOptions { fail_on_warning: true, ..Default::default() };
        fsck_with_options(filesystem.clone(), &options).await.expect("fsck failed");
        fsck_volume_with_options(
            filesystem.as_ref(),
            &options,
            store_id,
            Some(Arc::new(new_insecure_crypt())),
        )
        .await
        .expect("fsck_volume failed");
        filesystem.close().await.expect("Filesystem close");
    }

    /// Tests writing a partition image and installing a volume contained within.
    #[fuchsia::test(threads = 10)]
    async fn test_volume_installation() {
        let fixture = TestFixture::open(
            DeviceHolder::new(FakeDevice::new(1024, 4096)),
            TestFixtureOptions { format: true, encrypted: false, ..Default::default() },
        )
        .await;

        // Create a file "foo" in the existing volume "vol". This should be gone after installation.
        {
            let file = open_file_checked(
                fixture.root(),
                "foo",
                fio::Flags::PROTOCOL_FILE | fio::Flags::FLAG_MUST_CREATE | fio::PERM_WRITABLE,
                &Default::default(),
            )
            .await;
            file.write("Hello, world!".as_bytes()).await.unwrap().expect("write failed");
        };

        // Create another in-memory partition image with a different set of files.
        let image = {
            let inner_fixture = TestFixture::open(
                DeviceHolder::new(FakeDevice::new(512, 4096)),
                TestFixtureOptions { format: true, encrypted: false, ..Default::default() },
            )
            .await;
            let file = open_file_checked(
                inner_fixture.root(),
                "bar",
                fio::Flags::PROTOCOL_FILE | fio::Flags::FLAG_MUST_CREATE | fio::PERM_WRITABLE,
                &Default::default(),
            )
            .await;
            file.write("Well, this is new...".as_bytes()).await.unwrap().expect("write failed");
            file.close().await.unwrap().expect("close error");
            inner_fixture.close().await
        };

        // Write the partition image to a file "install" in a new volume called "src".
        {
            let (src_out_dir, server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
            fixture
                .volumes_directory()
                .create_and_serve_volume("src", server_end, Default::default(), Default::default())
                .await
                .unwrap();
            let src_root = open_dir_checked(
                &src_out_dir,
                "root",
                fio::PERM_READABLE | fio::PERM_WRITABLE,
                Default::default(),
            )
            .await;
            let file = open_file_checked(
                &src_root,
                "image",
                fio::Flags::PROTOCOL_FILE | fio::Flags::FLAG_MUST_CREATE | fio::PERM_WRITABLE,
                &Default::default(),
            )
            .await;
            write_image_to_file(image, file).await;
        };

        // Installation should not be possible yet since both volumes are still mounted.
        assert!(
            fixture.volumes_directory().install_volume("src", "image", "vol").await.is_err(),
            "volume installation should fail while either src/dst is still mounted"
        );

        // Now let's re-mount the filesystem manually without a fixture, install the volumes, and
        // spin up a new fixture to verify the result.
        let device = fixture.close().await;
        let fs = FxFilesystem::open(device).await.unwrap();
        {
            let root = root_volume(fs.clone()).await.unwrap();
            root.install_volume("src", "image", "vol").await.unwrap();
        }
        fs.close().await.unwrap();
        let device = fs.take_device().await;
        device.reopen(/*read_only*/ true);
        let fixture = TestFixture::open(
            device,
            TestFixtureOptions { encrypted: false, format: false, ..Default::default() },
        )
        .await;

        // Ensure that the "src" volume is now gone, and the old file "foo" is gone from "vol".
        assert!(
            fixture.volumes_directory().mount_volume("src", None, false).await.is_err(),
            "src volume should be deleted after installation"
        );
        assert!(
            testing::open_file(
                fixture.volume_out_dir(),
                "foo",
                fio::PERM_READABLE,
                &Default::default()
            )
            .await
            .is_err(),
            "foo should be deleted after installation"
        );

        // Check that we can find the contents of "vol" that we installed from the image.
        let file =
            open_file_checked(fixture.root(), "bar", fio::PERM_READABLE, &Default::default()).await;
        let data = file.read(fio::MAX_TRANSFER_SIZE).await.unwrap().expect("read failed");
        assert_eq!(String::from_utf8(data).unwrap(), "Well, this is new...");
        file.close().await.unwrap().unwrap();

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_create_with_low_32_bit_ids() {
        let device = DeviceHolder::new(FakeDevice::new(8192, 512));
        let filesystem = FxFilesystem::new_empty(device).await.unwrap();
        let blob_resupplied_count =
            Arc::new(PageRefaultCounter::new().expect("Failed to create PageRefaultCounter"));

        {
            let volumes_directory = VolumesDirectory::new(
                root_volume(filesystem.clone()).await.unwrap(),
                Weak::new(),
                None,
                blob_resupplied_count,
                MemoryPressureConfig::default(),
            )
            .await
            .unwrap();

            let mut guard = volumes_directory.lock().await;

            let vol = guard
                .create_or_mount_volume(
                    "low_32",
                    None,
                    Mode::Create { guid: None, low_32_bit_object_ids: true },
                    false,
                )
                .await
                .expect("create volume failed");

            let root_dir = vol.volume().store().root_directory_object_id();
            let root_dir = fxfs::object_store::Directory::open(vol.volume().store(), root_dir)
                .await
                .expect("open failed");

            let mut transaction = filesystem
                .root_store()
                .new_transaction(
                    lock_keys![LockKey::object(
                        vol.volume().store().store_object_id(),
                        root_dir.object_id()
                    )],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");

            let object = root_dir
                .create_child_file(&mut transaction, "test")
                .await
                .expect("create_child_file failed");

            // We can't check LastObjectIdInfo as it is private, but we can verify behavior.
            assert!(object.object_id() < 1 << 32);
            transaction.commit().await.expect("commit failed");
        };

        filesystem.close().await.expect("close filesystem failed");

        // Reopen and verify persistence
        let device = filesystem.take_device().await;
        device.reopen(false);
        let filesystem = FxFilesystem::open(device).await.unwrap();
        let blob_resupplied_count =
            Arc::new(PageRefaultCounter::new().expect("Failed to create PageRefaultCounter"));
        let volumes_directory = VolumesDirectory::new(
            root_volume(filesystem.clone()).await.unwrap(),
            Weak::new(),
            None,
            blob_resupplied_count,
            MemoryPressureConfig::default(),
        )
        .await
        .unwrap();

        // Mount the volume again, and check that new files are still created with expected IDs.
        {
            let mut guard = volumes_directory.lock().await;

            let vol = guard
                .create_or_mount_volume("low_32", None, Mode::Mount, false)
                .await
                .expect("mount volume failed");

            let root_dir = vol.volume().store().root_directory_object_id();
            let root_dir = fxfs::object_store::Directory::open(vol.volume().store(), root_dir)
                .await
                .expect("open failed");

            let mut transaction = filesystem
                .root_store()
                .new_transaction(
                    lock_keys![LockKey::object(
                        vol.volume().store().store_object_id(),
                        root_dir.object_id()
                    )],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");

            let object = root_dir
                .create_child_file(&mut transaction, "test2")
                .await
                .expect("create_child_file failed");

            assert!(object.object_id() < 1 << 32);
            transaction.commit().await.expect("commit failed");
        }

        filesystem.close().await.expect("close filesystem failed");
    }

    #[fuchsia::test(threads = 10)]
    async fn test_race_unmount_and_flush_with_crypt_error() {
        let device = DeviceHolder::new(FakeDevice::new(8192, 512));
        let filesystem = FxFilesystem::new_empty(device).await.unwrap();
        let blob_resupplied_count = Arc::new(PageRefaultCounter::new().unwrap());
        let volumes_directory = VolumesDirectory::new(
            root_volume(filesystem.clone()).await.unwrap(),
            Weak::new(),
            None,
            blob_resupplied_count,
            MemoryPressureConfig::default(),
        )
        .await
        .unwrap();

        let crypt_service = Arc::new(fxfs_crypt::CryptService::new());
        crypt_service
            .add_wrapping_key(0, fxfs_insecure_crypto::DATA_KEY.to_vec())
            .expect("add_wrapping_key failed");
        crypt_service
            .add_wrapping_key(1, fxfs_insecure_crypto::METADATA_KEY.to_vec())
            .expect("add_wrapping_key failed");
        crypt_service.set_active_key(KeyPurpose::Data, 0).expect("set_active_key failed");
        crypt_service.set_active_key(KeyPurpose::Metadata, 1).expect("set_active_key failed");

        for _ in 0..20 {
            let (client, mut stream) = create_request_stream::<fidl_fuchsia_fxfs::CryptMarker>();
            let (close_tx, mut close_rx) = futures::channel::oneshot::channel::<()>();

            let crypt_task = fasync::Task::spawn(async move {
                loop {
                    futures::select! {
                        _ = close_rx => return, // Close signal
                        request = stream.try_next() => {
                            match request {
                                Ok(Some(CryptRequest::CreateKey { responder, .. })) => {
                                    responder.send(Ok((&[0; 16], &[0; 48], &[0; 32]))).unwrap();
                                }
                                Ok(Some(CryptRequest::CreateKeyWithId {
                                        wrapping_key_id, responder, .. })) => {
                                    let key = WrappedKey::Fxfs(FxfsKey {
                                        wrapping_key_id,
                                        wrapped_key: [0u8; 48],
                                    });
                                    responder.send(Ok((&key, &[0; 32]))).unwrap();
                                }
                                Ok(Some(CryptRequest::UnwrapKey { responder, .. })) => {
                                    responder.send(Ok(&vec![0; 32])).unwrap();
                                }
                                _ => return,
                            }
                        }
                    }
                }
            });

            let volume = volumes_directory
                .create_and_mount_volume(
                    "encrypted",
                    Some(Arc::new(RemoteCrypt::new(client))),
                    false,
                    CreateOptions::default(),
                )
                .await
                .unwrap();

            // Write some data to dirty the journal.
            {
                let mut transaction = filesystem
                    .root_store()
                    .new_transaction(
                        lock_keys![LockKey::object(
                            volume.volume().store().store_object_id(),
                            volume.root_dir().directory().object_id()
                        )],
                        Options::default(),
                    )
                    .await
                    .unwrap();
                volume
                    .root_dir()
                    .directory()
                    .create_child_file(&mut transaction, "foo")
                    .await
                    .expect("create_child_file failed");
                transaction.commit().await.expect("commit failed");
            }

            let (dir_proxy, dir_server_end) =
                fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
            volumes_directory.serve_volume(&volume, dir_server_end, false).unwrap();
            volumes_directory.lock().await.auto_unmount(volume.volume().store().store_object_id());

            // Kill crypt.  The next usage should be in flush.
            let _ = close_tx.send(());

            let filesystem_clone = filesystem.clone();
            let compact_task = fasync::Task::spawn(async move {
                // Flush should not fail, because that would close the journal for the rest of the
                // filesystem.  Instead, the volume should be force-locked and flushed (which
                // doesn't depend on crypt).
                filesystem_clone
                    .object_manager()
                    .flush(FlushReason::Journal(ForceMajor::False))
                    .await
                    .expect("flush failed");
            });

            // Trigger unmount.
            std::mem::drop(dir_proxy);

            // Wait for volume to be unmounted, so we can clean up for the next iteration.
            let store_id = volume.volume().store().store_object_id();
            loop {
                {
                    let guard = volumes_directory.lock().await;
                    if !guard.mounted_volumes.contains_key(&store_id) {
                        break;
                    }
                }
                fasync::Timer::new(Duration::from_millis(10)).await;
            }

            join!(compact_task, crypt_task);
            volumes_directory.remove_volume("encrypted").await.expect("remove_volume failed");
        }
        volumes_directory.terminate().await;
        filesystem.close().await.expect("close filesystem failed");
    }
}
