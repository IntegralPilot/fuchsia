// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::component::map_to_raw_status;
use crate::directory::FxDirectory;
use crate::fuchsia::errors::map_to_status;
use crate::volume::FxVolume;
use crate::volumes_directory::VolumesDirectory;
use anyhow::{Context, Error};
use fidl_fuchsia_fxfs::DebugRequest;
use fidl_fuchsia_io as fio;
use fxfs::filesystem::FxFilesystem;
use fxfs::lsm_tree::Query;
use fxfs::lsm_tree::types::LayerIterator;
use fxfs::object_handle::{INVALID_OBJECT_ID, ObjectHandle, ReadObjectHandle};
use fxfs::object_store::{
    AttributeId, AttributeKey, DataObjectHandle, HandleOptions, ObjectDescriptor, ObjectKey,
    ObjectKeyData, ObjectStore,
};
use pseudo_fs::{LazyPseudoDirectory, LazyPseudoDirectoryState, PseudoDirectory};
use std::collections::BTreeMap;
use std::sync::{Arc, Weak};
use vfs::directory::dirents_sink::{self, AppendResult};
use vfs::directory::entry::{DirectoryEntry, GetEntryInfo, OpenRequest};
use vfs::directory::entry_container::Directory;
use vfs::directory::helper::DirectlyMutable;
use vfs::directory::immutable::connection::ImmutableConnection;
use vfs::directory::traversal_position::TraversalPosition;
use vfs::execution_scope::ExecutionScope;
use vfs::file::{FidlIoConnection, File, FileIo, FileLike, FileOptions, SyncMode};
use vfs::node::Node;
use vfs::{ObjectRequestRef, ToObjectRequest, attributes, immutable_attributes};
use zx::{self as zx, Status};

// To avoid dependency cycles, FxfsDebug stores weak references back to internal structures.  This
// convenience method returns an appropriate error when these internal structures are dropped.
fn upgrade_weak<T>(weak: &Weak<T>) -> Result<Arc<T>, Status> {
    weak.upgrade().ok_or(Status::CANCELED)
}

/// Immutable read-only access to internal Fxfs objects (attribute 0).
/// We open this as-required to avoid dealing with data that is otherwise cached in the handle
/// (specifically file size).
struct InternalFile {
    object_id: u64,
    store: Weak<ObjectStore>,
}

impl InternalFile {
    fn new(object_id: u64, store: Weak<ObjectStore>) -> Arc<Self> {
        Arc::new(Self { object_id, store })
    }

    /// Opens the file and returns a handle
    async fn handle(&self) -> Result<DataObjectHandle<ObjectStore>, zx::Status> {
        ObjectStore::open_object(
            &upgrade_weak(&self.store)?,
            self.object_id,
            HandleOptions::default(),
            None,
        )
        .await
        .map_err(map_to_status)
    }
}

impl DirectoryEntry for InternalFile {
    fn open_entry(self: Arc<Self>, request: OpenRequest<'_>) -> Result<(), Status> {
        request.open_file(self)
    }
}

impl GetEntryInfo for InternalFile {
    fn entry_info(&self) -> vfs::directory::entry::EntryInfo {
        vfs::directory::entry::EntryInfo::new(self.object_id, fio::DirentType::File)
    }
}

impl vfs::node::Node for InternalFile {
    async fn get_attributes(
        &self,
        requested_attributes: fio::NodeAttributesQuery,
    ) -> Result<fio::NodeAttributes2, zx::Status> {
        let props = self.handle().await?.get_properties().await.map_err(map_to_status)?;
        Ok(attributes!(
            requested_attributes,
            Mutable {
                creation_time: props.creation_time.as_nanos(),
                modification_time: props.modification_time.as_nanos()
            },
            Immutable {
                protocols: fio::NodeProtocolKinds::FILE,
                abilities: fio::Operations::GET_ATTRIBUTES | fio::Operations::READ_BYTES,
                id: self.object_id,
                content_size: props.data_attribute_size,
                storage_size: props.allocated_size,
                link_count: props.refs,
            }
        ))
    }

    fn query_filesystem(&self) -> Result<fio::FilesystemInfo, Status> {
        // Nb: self.handle() is async so we can't call it here.
        Err(zx::Status::NOT_SUPPORTED)
    }
}

impl File for InternalFile {
    async fn open_file(&self, _options: &FileOptions) -> Result<(), Status> {
        Ok(())
    }

    async fn truncate(&self, _length: u64) -> Result<(), Status> {
        Err(Status::NOT_SUPPORTED)
    }

    async fn get_size(&self) -> Result<u64, Status> {
        // TODO(ripper): Look up size in LSMTree on every request.
        Ok(self.handle().await?.get_size())
    }

    async fn update_attributes(
        &self,
        _attributes: fio::MutableNodeAttributes,
    ) -> Result<(), Status> {
        Err(zx::Status::NOT_SUPPORTED)
    }

    async fn sync(&self, _mode: SyncMode) -> Result<(), Status> {
        Ok(())
    }
}

impl FileIo for InternalFile {
    async fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Result<u64, Status> {
        // Deal with alignment. Handle requires aligned reads.
        let handle = self.handle().await?;
        let aligned = handle
            .block_size()
            .align_range_outwards(&(offset..offset + buffer.len() as u64))
            .unwrap();
        let mut buf = handle.allocate_buffer((aligned.end - aligned.start) as usize).await;
        let bytes = handle.read(aligned.start, buf.as_mut()).await.map_err(map_to_status)?;
        let end = std::cmp::min(offset + buffer.len() as u64, aligned.start + bytes as u64);
        if end > offset {
            let target_range = (offset - aligned.start) as usize..(end - aligned.start) as usize;
            buf.subslice(target_range).copy_to_slice(&mut buffer[..(end - offset) as usize]);
            Ok(end - offset)
        } else {
            Ok(0)
        }
    }

    async fn write_at(&self, _offset: u64, _content: &[u8]) -> Result<u64, Status> {
        Err(zx::Status::NOT_SUPPORTED)
    }

    async fn append(&self, _content: &[u8]) -> Result<(u64, u64), Status> {
        Err(zx::Status::NOT_SUPPORTED)
    }
}

impl FileLike for InternalFile {
    fn open(
        self: Arc<Self>,
        scope: ExecutionScope,
        options: FileOptions,
        object_request: ObjectRequestRef<'_>,
    ) -> Result<(), Status> {
        scope.clone().spawn(object_request.take().handle_async(async move |object_request| {
            FidlIoConnection::create(scope, self, options, object_request).await
        }));
        Ok(())
    }
}

/// A way to list an `FxDirectory` without holding any strong references when idle.
struct LazyInternalDirectory {
    volume: Weak<FxVolume>,
    object_id: u64,
}

impl LazyInternalDirectory {
    async fn upgrade(&self) -> Result<Arc<FxDirectory>, Status> {
        let volume = upgrade_weak(&self.volume)?;
        let node = volume
            .get_or_load_node(self.object_id, ObjectDescriptor::Directory, None)
            .await
            .map_err(|_| Status::INTERNAL)?;
        node.into_any().downcast::<FxDirectory>().map_err(|_| Status::IO_DATA_INTEGRITY)
    }
}

impl DirectoryEntry for LazyInternalDirectory {
    fn open_entry(self: Arc<Self>, request: OpenRequest<'_>) -> Result<(), Status> {
        request.open_dir(self)
    }
}

impl GetEntryInfo for LazyInternalDirectory {
    fn entry_info(&self) -> vfs::directory::entry::EntryInfo {
        vfs::directory::entry::EntryInfo::new(self.object_id, fio::DirentType::Directory)
    }
}

impl Node for LazyInternalDirectory {
    async fn get_attributes(
        &self,
        requested_attributes: fio::NodeAttributesQuery,
    ) -> Result<fio::NodeAttributes2, Status> {
        self.upgrade().await?.get_attributes(requested_attributes).await
    }
}

impl Directory for LazyInternalDirectory {
    fn deprecated_open(
        self: Arc<Self>,
        scope: ExecutionScope,
        flags: fio::OpenFlags,
        path: vfs::path::Path,
        server_end: fidl::endpoints::ServerEnd<fio::NodeMarker>,
    ) {
        scope.clone().spawn(async move {
            match self.upgrade().await {
                Ok(dir) => dir.deprecated_open(scope, flags, path, server_end),
                Err(status) => {
                    let _ = server_end.close_with_epitaph(status);
                }
            }
        });
    }

    fn open(
        self: Arc<Self>,
        scope: ExecutionScope,
        path: vfs::path::Path,
        flags: fio::Flags,
        object_request: ObjectRequestRef<'_>,
    ) -> Result<(), Status> {
        scope.clone().spawn(object_request.take().handle_async(async move |object_request| {
            match self.upgrade().await {
                Ok(dir) => {
                    let _ = dir.open(scope, path, flags, object_request);
                }
                Err(s) => object_request.take().shutdown(s),
            }
            Ok(())
        }));
        Ok(())
    }

    async fn read_dirents(
        &self,
        pos: &TraversalPosition,
        sink: Box<dyn dirents_sink::Sink>,
    ) -> Result<(TraversalPosition, Box<dyn dirents_sink::Sealed>), Status> {
        self.upgrade().await?.read_dirents(pos, sink).await
    }

    fn register_watcher(
        self: Arc<Self>,
        _scope: ExecutionScope,
        _mask: fio::WatchMask,
        _watcher: vfs::directory::entry_container::DirectoryWatcher,
    ) -> Result<(), Status> {
        Err(zx::Status::NOT_SUPPORTED)
    }

    fn unregister_watcher(self: Arc<Self>, _key: usize) {}
}

/// Exposes a VFS directory containing debug entries for a given object store.
struct ObjectStoreDirectory {
    vfs_root: Arc<vfs::directory::immutable::Simple>,
    parent_store: Option<Weak<ObjectStore>>,
    // Only set if `parent_store` is, since we only track persistent layers here.
    layers: Option<Arc<vfs::directory::immutable::Simple>>,
}

impl ObjectStoreDirectory {
    /// Used when creating from an object store, for the root and root_parent stores.
    fn new_from_object_store(
        store: Arc<ObjectStore>,
        parent_store: Option<Arc<ObjectStore>>,
    ) -> Result<Arc<Self>, Status> {
        Self::new(store, None, parent_store)
    }

    /// Used for creating from a normal volume.
    fn new_from_volume(
        volume: Arc<FxVolume>,
        parent_store: Arc<ObjectStore>,
    ) -> Result<Arc<Self>, Status> {
        Self::new(volume.store().clone(), Some(volume), Some(parent_store))
    }

    fn new(
        store: Arc<ObjectStore>,
        volume: Option<Arc<FxVolume>>,
        parent_store: Option<Arc<ObjectStore>>,
    ) -> Result<Arc<Self>, Status> {
        let vfs_root = PseudoDirectory::new();
        let layers_dir = if parent_store.is_some() {
            let layers_dir = PseudoDirectory::new();
            vfs_root.add_entry("layers", layers_dir.clone())?;
            Some(layers_dir)
        } else {
            None
        };

        vfs_root.add_entry(
            "objects",
            Arc::new(ObjectDirectory {
                store: Arc::downgrade(&store),
                store_object_id: store.store_object_id(),
            }),
        )?;

        if let Some(volume) = volume {
            if let Ok(object_id) = store.get_internal_directory_id() {
                vfs_root.add_entry(
                    "internal_storage",
                    Arc::new(LazyInternalDirectory { volume: Arc::downgrade(&volume), object_id })
                        as Arc<dyn DirectoryEntry>,
                )?;
            }
        }

        // TODO(b/313524454):
        //  * graveyard_dir
        //  * root_dir
        //  * '/lsm_tree' with full contents of the merged lsm_tree.

        let this = Arc::new(Self {
            vfs_root,
            parent_store: parent_store.as_ref().map(Arc::downgrade),
            layers: layers_dir,
        });
        this.update_from_store(store.as_ref())?;

        let this_clone = this.clone();
        store.set_flush_callback(move |store| {
            if let Err(e) = this_clone.update_from_store(store) {
                log::warn!(e:?; "debug: Failed to update store; debug info may be stale");
            }
        });

        Ok(this)
    }

    fn update_from_store(&self, store: &ObjectStore) -> Result<(), Status> {
        // If the store is flushed by the journal while it's still locked, store_info won't be
        // available yet.
        self.vfs_root.remove_entry("store_info.txt", false)?;
        if let Some(store_info) = store.store_info() {
            let store_info_txt = format!("{:?}", store_info);
            self.vfs_root.add_entry("store_info.txt", vfs::file::vmo::read_only(store_info_txt))?;
        }

        let (layers_dir, parent_store) = if let Some(layers) = self.layers.as_ref() {
            (layers, upgrade_weak(self.parent_store.as_ref().unwrap())?)
        } else {
            return Ok(());
        };
        layers_dir.remove_all_entries();
        let layers = store
            .tree()
            .immutable_layer_set()
            .layers
            .iter()
            // NB: some layers in the immutable set might not be persistent layers yet (i.e. they
            // are sealed in-memory layers).  We still want to track the layer file indexes
            // correctly, so plumb them through as None here.
            .map(|layer| {
                layer
                    .handle()
                    .map(|h| InternalFile::new(h.object_id(), Arc::downgrade(&parent_store)))
            })
            .collect::<Vec<_>>();
        for (idx, layer) in layers.into_iter().enumerate() {
            if let Some(layer) = layer {
                layers_dir.add_entry(format!("{}", idx), layer)?;
            }
        }
        Ok(())
    }
}

/// Exposes a VFS directory containing all objects in a store with a data attribute.
/// Objects are named by their object_id in decimal.
struct ObjectDirectory {
    store: Weak<ObjectStore>,
    store_object_id: u64,
}

impl DirectoryEntry for ObjectDirectory {
    fn open_entry(self: Arc<Self>, request: OpenRequest<'_>) -> Result<(), Status> {
        request.open_dir(self)
    }
}

impl GetEntryInfo for ObjectDirectory {
    fn entry_info(&self) -> vfs::directory::entry::EntryInfo {
        vfs::directory::entry::EntryInfo::new(self.store_object_id, fio::DirentType::Directory)
    }
}

impl Node for ObjectDirectory {
    async fn get_attributes(
        &self,
        requested_attributes: fio::NodeAttributesQuery,
    ) -> Result<fio::NodeAttributes2, Status> {
        let id = upgrade_weak(&self.store)?.store_object_id();
        Ok(immutable_attributes!(
            requested_attributes,
            Immutable {
                protocols: fio::NodeProtocolKinds::DIRECTORY,
                abilities: fio::Operations::GET_ATTRIBUTES
                    | fio::Operations::UPDATE_ATTRIBUTES
                    | fio::Operations::ENUMERATE
                    | fio::Operations::TRAVERSE
                    | fio::Operations::MODIFY_DIRECTORY,
                id: id
            }
        ))
    }
}

impl Directory for ObjectDirectory {
    fn deprecated_open(
        self: Arc<Self>,
        scope: ExecutionScope,
        flags: fio::OpenFlags,
        mut path: vfs::path::Path,
        server_end: fidl::endpoints::ServerEnd<fio::NodeMarker>,
    ) {
        flags.to_object_request(server_end).handle(|object_request| {
            match path.next_with_ref() {
                (_, Some(name)) => {
                    // Lookup an object by id and return it.
                    let name = name.to_owned();
                    let object_id = name.parse().unwrap_or(INVALID_OBJECT_ID);
                    vfs::file::serve(
                        InternalFile::new(object_id, self.store.clone()),
                        scope,
                        &flags,
                        object_request,
                    )
                }
                (_, None) => {
                    object_request
                        .take()
                        .create_connection_sync::<ImmutableConnection<_>, _>(scope, self, flags);
                    Ok(())
                }
            }
        });
    }

    fn open(
        self: Arc<Self>,
        scope: ExecutionScope,
        mut path: vfs::path::Path,
        flags: fio::Flags,
        object_request: ObjectRequestRef<'_>,
    ) -> Result<(), Status> {
        match path.next_with_ref() {
            (_, Some(name)) => {
                // Lookup an object by id and return it.
                let name = name.to_owned();
                let object_id = name.parse().unwrap_or(INVALID_OBJECT_ID);
                vfs::file::serve(
                    InternalFile::new(object_id, self.store.clone()),
                    scope,
                    &flags,
                    object_request,
                )
            }
            (_, None) => {
                object_request
                    .take()
                    .create_connection_sync::<ImmutableConnection<_>, _>(scope, self, flags);
                Ok(())
            }
        }
    }

    /// Reads directory entries starting from `pos` by adding them to `sink`.
    /// Once finished, should return a sealed sink.
    async fn read_dirents(
        &self,
        pos: &TraversalPosition,
        mut sink: Box<dyn dirents_sink::Sink>,
    ) -> Result<(TraversalPosition, Box<dyn dirents_sink::Sealed>), Status> {
        let object_id = match pos {
            TraversalPosition::Start => 0,
            TraversalPosition::Index(object_id) => *object_id,
            TraversalPosition::End => u64::MAX,
            _ => return Err(zx::Status::BAD_STATE),
        };
        let store = upgrade_weak(&self.store)?;
        let layer_set = store.tree().layer_set();
        let mut merger = layer_set.merger();
        let mut iter = merger
            .query(Query::FullRange(&ObjectKey::object(object_id)))
            .await
            .map_err(map_to_status)?;
        while let Some(data) = iter.get() {
            match data.key {
                ObjectKey {
                    object_id,
                    data: ObjectKeyData::Attribute(AttributeId::DATA, AttributeKey::Attribute),
                } => {
                    sink = match sink.append(
                        &vfs::directory::entry::EntryInfo::new(*object_id, fio::DirentType::File),
                        &object_id.to_string(),
                    ) {
                        AppendResult::Ok(sink) => sink,
                        AppendResult::Sealed(sink) => {
                            return Ok((TraversalPosition::Index(*object_id), sink));
                        }
                    };
                }
                _ => {}
            }
            iter.advance().await.map_err(map_to_status)?;
        }

        Ok((TraversalPosition::End, sink.seal()))
    }

    fn register_watcher(
        self: Arc<Self>,
        _scope: ExecutionScope,
        _mask: fio::WatchMask,
        _watcher: vfs::directory::entry_container::DirectoryWatcher,
    ) -> Result<(), Status> {
        Err(zx::Status::NOT_SUPPORTED)
    }

    fn unregister_watcher(self: Arc<Self>, _key: usize) {}
}

/// Stores the information to construct the debug directory until it's accessed.
struct FxfsDebugInfo {
    fs: Weak<FxFilesystem>,
    volumes: BTreeMap<String, Weak<FxVolume>>,
}

impl FxfsDebugInfo {
    fn build_directory(self, root: &PseudoDirectory) -> Result<(), Error> {
        let fs = self.fs.upgrade().context("Failed to upgrade FxFilesystem")?;

        let root_parent_store =
            ObjectStoreDirectory::new_from_object_store(fs.root_parent_store(), None)
                .context("Failed to create root_parent_store directory")?;
        root.add_entry("root_parent_store", root_parent_store.vfs_root.clone())
            .context("Failed to add root_parent_store directory")?;

        let root_store = ObjectStoreDirectory::new_from_object_store(
            fs.root_store(),
            Some(fs.root_parent_store()),
        )
        .context("Failed to create root_store directory")?;
        root_parent_store
            .vfs_root
            .add_entry("root_store", root_store.vfs_root.clone())
            .context("Failed to add root_store directory")?;

        root_parent_store
            .vfs_root
            .add_entry(
                "journal",
                InternalFile::new(
                    fs.super_block_header().journal_object_id,
                    Arc::downgrade(&fs.root_parent_store()),
                ),
            )
            .context("Failed to add journal file")?;

        // TODO(b/313524454): This should update dynamically.
        let superblock_header_txt = format!("{:?}", fs.super_block_header());
        root_store
            .vfs_root
            .add_entry("superblock_header.txt", vfs::file::vmo::read_only(superblock_header_txt))
            .context("Failed to add superblock_header.txt file")?;

        // TODO(b/313524454): Enumerate SuperBlockInstance::A and B.

        // TODO(b/313524454): Enumerate fs.object_manager().volumes_directory() under root_store to
        // find volumes which are not currently open.

        // TODO(b/313524454): Export Allocator info under root_store.

        let volumes_dir = PseudoDirectory::new();
        root_store
            .vfs_root
            .add_entry("volumes", volumes_dir.clone())
            .context("Failed to add volumes directory")?;
        for (name, volume) in self.volumes {
            let volume =
                volume.upgrade().with_context(|| format!("Failed to upgrade volume \"{name}\""))?;
            let object_store_dir =
                ObjectStoreDirectory::new_from_volume(volume, fs.root_store())
                    .with_context(|| format!("Failed to create volume directory for \"{name}\""))?;
            volumes_dir
                .add_entry(&name, object_store_dir.vfs_root.clone())
                .with_context(|| format!("Failed to add volume directory for \"{name}\""))?;
        }
        Ok(())
    }
}

impl pseudo_fs::ToPseudoDirectory for FxfsDebugInfo {
    fn to_pseudo_directory(self) -> Arc<pseudo_fs::PseudoDirectory> {
        let root = PseudoDirectory::new();
        if let Err(e) = self.build_directory(&root) {
            log::error!("Failed to build debug directory: {e:?}");
        }
        root
    }
}

pub async fn handle_debug_request(
    fs: Arc<FxFilesystem>,
    volumes: Arc<VolumesDirectory>,
    request: DebugRequest,
) -> Result<(), fidl::Error> {
    match request {
        DebugRequest::Compact { responder } => {
            responder.send(fs.journal().force_compact().await.map_err(map_to_raw_status))
        }
        DebugRequest::DeleteProfile { responder, volume, profile } => responder
            .send(volumes.delete_profile(&volume, &profile).await.map_err(Status::into_raw)),
        DebugRequest::RecordAndReplayProfile { responder, volume, profile, duration_secs } => {
            responder.send(
                volumes
                    .record_and_replay_profile(volume, profile, duration_secs)
                    .await
                    .map_err(Status::into_raw),
            )
        }
        DebugRequest::ReplayXorRecordProfile { responder, volume, profile, duration_secs } => {
            responder.send(
                volumes
                    .replay_xor_record_profile(volume, profile, duration_secs)
                    .await
                    .map_err(Status::into_raw),
            )
        }
        DebugRequest::StopProfileTasks { responder } => {
            volumes.stop_profile_tasks().await;
            responder.send(Ok(()))
        }
        DebugRequest::ClearCaches { responder } => {
            volumes.clear_caches().await;
            responder.send(Ok(()))
        }
    }
}

/// Creates fxfs' debug directory.
pub fn create_debug_directory(
    fs: &Arc<FxFilesystem>,
    volumes: &Arc<VolumesDirectory>,
) -> Arc<dyn DirectoryEntry> {
    let debug_dir = LazyPseudoDirectory::new(FxfsDebugInfo {
        fs: Arc::downgrade(fs),
        volumes: BTreeMap::new(),
    });
    let dir_clone = debug_dir.clone();
    volumes.set_on_mount_callback(move |name, volume_and_parent_store| {
        let result = match volume_and_parent_store {
            Some((volume, parent_store)) => add_volume(&debug_dir, name, volume, parent_store),
            None => remove_volume(&debug_dir, name),
        };
        if let Err(e) = result {
            log::error!("debug directory failure for volume \"{name}\": {e:?}");
        }
    });
    dir_clone
}

/// Returns the directory at `"root_parent_store/root_store/volumes"` within `debug_dir`.
fn get_volumes_dir(debug_dir: &Arc<PseudoDirectory>) -> Result<Arc<PseudoDirectory>, Error> {
    let root_parent_store = debug_dir
        .get_entry("root_parent_store")
        .context("Failed to get root_parent_store")?
        .into_any()
        .downcast::<PseudoDirectory>()
        .map_err(|_| anyhow::anyhow!("Failed to downcast root_parent_store to PseudoDirectory"))?;
    let root_store = root_parent_store
        .get_entry("root_store")
        .context("Failed to get root_store")?
        .into_any()
        .downcast::<PseudoDirectory>()
        .map_err(|_| anyhow::anyhow!("Failed to downcast root_store to PseudoDirectory"))?;
    let volumes_dir = root_store
        .get_entry("volumes")
        .context("Failed to get volumes")?
        .into_any()
        .downcast::<PseudoDirectory>()
        .map_err(|_| anyhow::anyhow!("Failed to downcast volumes to PseudoDirectory"))?;
    Ok(volumes_dir)
}

/// Adds a volume to the debug directory.
fn add_volume(
    debug_dir: &LazyPseudoDirectory<FxfsDebugInfo>,
    name: &str,
    volume: Arc<FxVolume>,
    parent_store: Arc<ObjectStore>,
) -> Result<(), Error> {
    match debug_dir.state() {
        LazyPseudoDirectoryState::Data(mut debug_info) => {
            debug_info.volumes.insert(name.to_string(), Arc::downgrade(&volume));
        }
        LazyPseudoDirectoryState::Directory(debug_dir) => {
            let object_store_dir = ObjectStoreDirectory::new_from_volume(volume, parent_store)
                .with_context(|| format!("Failed to create volume directory for \"{name}\""))?;
            get_volumes_dir(&debug_dir)?
                .add_entry(name, object_store_dir.vfs_root.clone())
                .with_context(|| format!("Failed to add volume entry for \"{name}\""))?;
        }
    }
    Ok(())
}

/// Remove a volume from the debug directory.
fn remove_volume(debug_dir: &LazyPseudoDirectory<FxfsDebugInfo>, name: &str) -> Result<(), Error> {
    match debug_dir.state() {
        LazyPseudoDirectoryState::Data(mut debug_info) => {
            debug_info.volumes.remove(name);
        }
        LazyPseudoDirectoryState::Directory(debug_dir) => {
            get_volumes_dir(&debug_dir)?
                .remove_entry(name, false)
                .with_context(|| format!("Failed to remove volume entry for \"{name}\""))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fuchsia::testing::{TestFixture, open_dir_checked};
    use fidl::endpoints::create_proxy;
    use fidl_fuchsia_fs_startup::CreateOptions;
    use fidl_fuchsia_io as fio;
    use fuchsia_fs::directory::{read_file, readdir};
    use fxfs::object_store::transaction::{LockKey, Options, lock_keys};
    use vfs::ToObjectRequest;
    use vfs::directory::entry::OpenRequest;
    use vfs::execution_scope::ExecutionScope;

    fn serve_debug_dir(
        debug_dir: Arc<dyn DirectoryEntry>,
    ) -> (fio::DirectoryProxy, ExecutionScope) {
        let (client, server) = create_proxy::<fio::DirectoryMarker>();
        let scope = ExecutionScope::new();
        let flags = fio::PERM_READABLE;
        flags
            .to_object_request(server)
            .handle(|object_request| {
                debug_dir.open_entry(OpenRequest::new(
                    scope.clone(),
                    flags,
                    vfs::path::Path::dot(),
                    object_request,
                ))?;
                Ok(())
            })
            .unwrap();
        (client, scope)
    }

    async fn open_dir(parent: &fio::DirectoryProxy, path: &str) -> fio::DirectoryProxy {
        open_dir_checked(
            parent,
            path,
            fio::PERM_READABLE | fio::Flags::PROTOCOL_DIRECTORY,
            Default::default(),
        )
        .await
    }

    async fn expect_dir_entries(dir: &fio::DirectoryProxy, expected: &[&str]) {
        let entries = readdir(dir).await.unwrap();
        let mut names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        names.sort();
        assert_eq!(names, expected);
    }

    async fn verify_debug_directory_entries(
        debug_dir_proxy: &fio::DirectoryProxy,
        expected_volumes: &[&str],
    ) {
        expect_dir_entries(debug_dir_proxy, &["root_parent_store"]).await;

        let parent_store_dir = open_dir(debug_dir_proxy, "root_parent_store").await;
        expect_dir_entries(
            &parent_store_dir,
            &["journal", "objects", "root_store", "store_info.txt"],
        )
        .await;

        let root_store_dir = open_dir(&parent_store_dir, "root_store").await;
        expect_dir_entries(
            &root_store_dir,
            &["layers", "objects", "store_info.txt", "superblock_header.txt", "volumes"],
        )
        .await;

        let volumes_dir = open_dir(&root_store_dir, "volumes").await;
        expect_dir_entries(&volumes_dir, expected_volumes).await;

        for volume_name in expected_volumes {
            let vol_dir = open_dir(&volumes_dir, volume_name).await;
            expect_dir_entries(&vol_dir, &["layers", "objects", "store_info.txt"]).await;
        }
    }

    #[fuchsia::test]
    async fn test_debug_directory_lazy_materialization() {
        let fixture = TestFixture::new().await;

        let debug_dir = create_debug_directory(fixture.fs(), fixture.volumes_directory())
            .into_any()
            .downcast::<LazyPseudoDirectory<FxfsDebugInfo>>()
            .unwrap();
        assert!(debug_dir.state().is_data());

        // Connecting to the directory doesn't cause it to materialize.
        let (proxy, _scope) = serve_debug_dir(debug_dir.clone());
        assert!(debug_dir.state().is_data());

        // Making a request should cause the directory to materialize.
        proxy.get_flags().await.unwrap().unwrap();
        assert!(debug_dir.state().is_directory());

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_debug_directory_entries_volume_added_before_materialization() {
        let fixture = TestFixture::new().await;
        {
            let debug_dir = create_debug_directory(fixture.fs(), fixture.volumes_directory());

            let _volume_and_root = fixture
                .volumes_directory()
                .create_and_mount_volume("vol2", None, false, CreateOptions::default())
                .await
                .expect("failed to create volume");

            // Open/materialize the debug directory.
            let (debug_dir_proxy, _scope) = serve_debug_dir(debug_dir);
            debug_dir_proxy.get_flags().await.unwrap().unwrap();

            verify_debug_directory_entries(&debug_dir_proxy, &["vol2"]).await;
        }
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_debug_directory_entries_volume_added_after_materialization() {
        let fixture = TestFixture::new().await;
        {
            let debug_dir = create_debug_directory(fixture.fs(), fixture.volumes_directory());

            // Open/materialize the debug directory.
            let (debug_dir_proxy, _scope) = serve_debug_dir(debug_dir);
            let _ = debug_dir_proxy.get_flags().await.unwrap().unwrap();

            let _volume_and_root = fixture
                .volumes_directory()
                .create_and_mount_volume("vol2", None, false, CreateOptions::default())
                .await
                .expect("failed to create volume");

            verify_debug_directory_entries(&debug_dir_proxy, &["vol2"]).await;
        }
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_volume_removed_before_materialization() {
        let fixture = TestFixture::new().await;
        {
            let debug_dir = create_debug_directory(fixture.fs(), fixture.volumes_directory());

            // Create and remove a volume when the debug directory hasn't been materialized.
            let store_id = {
                let volume_and_root = fixture
                    .volumes_directory()
                    .create_and_mount_volume("vol2", None, false, CreateOptions::default())
                    .await
                    .expect("failed to create volume");
                volume_and_root.volume().store().store_object_id()
            };
            fixture.volumes_directory().lock().await.unmount(store_id).await.unwrap();
            fixture.volumes_directory().remove_volume("vol2").await.unwrap();

            // Open/materialize the debug directory.
            let (debug_dir_proxy, _scope) = serve_debug_dir(debug_dir);
            debug_dir_proxy.get_flags().await.unwrap().unwrap();

            // The volumes directory should be empty.
            verify_debug_directory_entries(&debug_dir_proxy, &[]).await;
        }
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_volume_removed_after_materialization() {
        let fixture = TestFixture::new().await;
        {
            let debug_dir = create_debug_directory(fixture.fs(), fixture.volumes_directory());

            // Open and materialize the debug directory first.
            let (debug_dir_proxy, _scope) = serve_debug_dir(debug_dir);
            debug_dir_proxy.get_flags().await.unwrap().unwrap();

            // Initially the volumes directory is empty.
            verify_debug_directory_entries(&debug_dir_proxy, &[]).await;

            let store_id = {
                let volume_and_root = fixture
                    .volumes_directory()
                    .create_and_mount_volume("vol2", None, false, CreateOptions::default())
                    .await
                    .expect("failed to create volume");
                volume_and_root.volume().store().store_object_id()
            };

            // "vol2" is now present.
            verify_debug_directory_entries(&debug_dir_proxy, &["vol2"]).await;

            // Unmount and remove "vol2".
            fixture.volumes_directory().lock().await.unmount(store_id).await.unwrap();
            fixture.volumes_directory().remove_volume("vol2").await.unwrap();

            // The volumes directory is empty again.
            verify_debug_directory_entries(&debug_dir_proxy, &[]).await;
        }
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_mutable_layer_skipped_in_immutable_set() {
        let fixture = TestFixture::new().await;
        {
            let debug_dir = create_debug_directory(fixture.fs(), fixture.volumes_directory());
            let volume_and_root = fixture
                .volumes_directory()
                .create_and_mount_volume("vol2", None, false, CreateOptions::default())
                .await
                .expect("failed to create volume");
            let volume = volume_and_root.volume();
            let store = volume.store();

            // Flush the store to create a persistent layer.
            store.flush().await.expect("flush failed");

            // Write a new file to populate the mutable layer with something.
            let root_dir = volume_and_root.root_dir();
            let mut transaction = store
                .new_transaction(
                    lock_keys![LockKey::object(
                        store.store_object_id(),
                        root_dir.directory().object_id()
                    )],
                    Options::default(),
                )
                .await
                .unwrap();
            root_dir.directory().create_child_file(&mut transaction, "temp_file").await.unwrap();
            transaction.commit().await.unwrap();

            // Seal the tree to move the mutable layer to the immutable layer set.
            store.tree().seal();

            let (client, _scope) = serve_debug_dir(debug_dir.clone());
            let layers_dir =
                open_dir(&client, "root_parent_store/root_store/volumes/vol2/layers").await;

            // Verify that entry "0" is skipped, but entry "1" is present.
            expect_dir_entries(&layers_dir, &["1"]).await;
        }
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_volume_store_info_updated_on_flush() {
        let fixture = TestFixture::new().await;
        {
            let debug_dir = create_debug_directory(fixture.fs(), fixture.volumes_directory());
            let (debug_dir_proxy, _scope) = serve_debug_dir(debug_dir);

            let volume_and_root = fixture
                .volumes_directory()
                .create_and_mount_volume("vol2", None, false, CreateOptions::default())
                .await
                .expect("failed to create volume");
            let volume = volume_and_root.volume();
            let store = volume.store();

            let initial_vol_store_info = read_file(
                &debug_dir_proxy,
                "root_parent_store/root_store/volumes/vol2/store_info.txt",
            )
            .await
            .unwrap();

            // Make some mutations in vol2 to make sure a flush will actually build a new layer.
            let root_dir = volume_and_root.root_dir();
            let mut transaction = store
                .new_transaction(
                    lock_keys![LockKey::object(
                        store.store_object_id(),
                        root_dir.directory().object_id()
                    )],
                    Options::default(),
                )
                .await
                .unwrap();
            root_dir.directory().create_child_file(&mut transaction, "temp_file").await.unwrap();
            transaction.commit().await.unwrap();

            // Flush the store to commit the changes and trigger the update callback.
            store.flush().await.expect("flush failed");

            let updated_vol_store_info = read_file(
                &debug_dir_proxy,
                "root_parent_store/root_store/volumes/vol2/store_info.txt",
            )
            .await
            .unwrap();
            assert_ne!(initial_vol_store_info, updated_vol_store_info);
        }
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_root_store_info_updated_on_flush() {
        let fixture = TestFixture::new().await;
        {
            let debug_dir = create_debug_directory(fixture.fs(), fixture.volumes_directory());
            let (debug_dir_proxy, _scope) = serve_debug_dir(debug_dir);

            let initial_root_store_info =
                read_file(&debug_dir_proxy, "root_parent_store/root_store/store_info.txt")
                    .await
                    .unwrap();

            // Create a volume to generate mutations in root_store.
            let _vol2 = fixture
                .volumes_directory()
                .create_and_mount_volume("vol2", None, false, CreateOptions::default())
                .await
                .expect("failed to create volume");

            fixture.fs().root_store().flush().await.expect("flush failed");

            let updated_root_store_info =
                read_file(&debug_dir_proxy, "root_parent_store/root_store/store_info.txt")
                    .await
                    .unwrap();
            assert_ne!(initial_root_store_info, updated_root_store_info);
        }
        fixture.close().await;
    }
}
