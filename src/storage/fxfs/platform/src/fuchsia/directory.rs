// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::fuchsia::device::BlockServer;
use crate::fuchsia::dirent_cache::DirentCacheKey;
use crate::fuchsia::errors::map_to_status;
use crate::fuchsia::file::FxFile;
use crate::fuchsia::node::{FxNode, GetResult, OpenedNode};
use crate::fuchsia::symlink::FxSymlink;
use crate::fuchsia::volume::{FxVolume, RootDir};
use anyhow::{Error, bail};
use either::{Left, Right};
use fidl::endpoints::ServerEnd;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_storage_block::BlockMarker;
use fuchsia_sync::Mutex;
use futures::future::BoxFuture;
use fxfs::errors::FxfsError;
use fxfs::filesystem::{SyncOptions, TruncateGuard};
use fxfs::log::*;
use fxfs::object_store::directory::{self, ReplacedChild};
use fxfs::object_store::transaction::{LockKey, Options, Transaction, lock_keys};
use fxfs::object_store::{self, Directory, ObjectDescriptor, ObjectStore, Timestamp};
use fxfs_crypto::WrappingKeyId;
use fxfs_macros::ToWeakNode;
use std::any::Any;
use std::sync::Arc;
use vfs::directory::dirents_sink::{self, AppendResult};
use vfs::directory::entry::{DirectoryEntry, EntryInfo, GetEntryInfo, OpenRequest};
use vfs::directory::entry_container::{
    Directory as VfsDirectory, DirectoryWatcher, MutableDirectory,
};
use vfs::directory::mutable::connection::MutableConnection;
use vfs::directory::traversal_position::TraversalPosition;
use vfs::directory::watchers::Watchers;
use vfs::directory::watchers::event_producers::SingleNameEventProducer;
use vfs::execution_scope::ExecutionScope;
use vfs::path::Path;
use vfs::{ObjectRequest, ObjectRequestRef, ProtocolsExt, ToObjectRequest, attributes, symlink};

struct ReplaceWithPurgeResult<'a> {
    transaction: Transaction<'a>,
    truncate_guard: Option<TruncateGuard<'static>>,
    replace_result: ReplacedChild,
    moved_node: Option<Arc<dyn FxNode>>,
    actual_src_name: String,
    actual_dst_name: String,
}

#[derive(ToWeakNode)]
pub struct FxDirectory {
    // The root directory is the only directory which has no parent, and its parent can never
    // change, hence the Option can go on the outside.
    parent: Option<Mutex<Arc<FxDirectory>>>,
    directory: object_store::Directory<FxVolume>,
    watchers: Mutex<Watchers>,
}

impl RootDir for FxDirectory {
    fn as_directory_entry(self: Arc<Self>) -> Arc<dyn DirectoryEntry> {
        self
    }

    fn serve(self: Arc<Self>, flags: fio::Flags, server_end: ServerEnd<fio::DirectoryMarker>) {
        let scope = self.volume().scope().clone();
        vfs::directory::serve_on(self, flags, scope, server_end);
    }

    fn as_node(self: Arc<Self>) -> Arc<dyn FxNode> {
        self as Arc<dyn FxNode>
    }
}

impl FxDirectory {
    pub(super) fn new(
        parent: Option<Arc<FxDirectory>>,
        directory: object_store::Directory<FxVolume>,
    ) -> Self {
        Self {
            parent: parent.map(|p| Mutex::new(p)),
            directory,
            watchers: Mutex::new(Watchers::new()),
        }
    }

    pub fn directory(&self) -> &object_store::Directory<FxVolume> {
        &self.directory
    }

    pub fn volume(&self) -> &Arc<FxVolume> {
        self.directory.owner()
    }

    pub fn store(&self) -> &ObjectStore {
        self.directory.store()
    }

    pub fn is_deleted(&self) -> bool {
        self.directory.is_deleted()
    }

    pub fn set_deleted(&self) {
        self.directory.set_deleted();
        self.watchers.lock().send_event(&mut SingleNameEventProducer::deleted());
    }

    async fn lookup(
        self: &Arc<Self>,
        protocols: &dyn ProtocolsExt,
        mut path: Path,
        request: &ObjectRequest,
    ) -> Result<OpenedNode<dyn FxNode>, Error> {
        if path.is_empty() {
            return if protocols.create_unnamed_temporary_in_directory_path() {
                self.create_unnamed_temporary_file(request.create_attributes()).await
            } else {
                Ok(OpenedNode::new(self.clone()))
            };
        }
        let store = self.store();
        let fs = store.filesystem();
        let mut current_node = self.clone() as Arc<dyn FxNode>;
        loop {
            let last_segment = path.is_single_component();
            let current_dir =
                current_node.into_any().downcast::<FxDirectory>().map_err(|_| FxfsError::NotDir)?;
            let name = path.next().unwrap();

            // Create the transaction here if we might need to create the object so that we have a
            // lock in place.
            let keys = lock_keys![LockKey::object(
                store.store_object_id(),
                current_dir.directory.object_id()
            )];
            let create_object = last_segment
                && matches!(
                    protocols.creation_mode(),
                    vfs::CreationMode::AllowExisting | vfs::CreationMode::Always
                );
            let transaction_or_guard = if create_object {
                Left(store.new_transaction(keys, Options::default()).await?)
            } else {
                // When child objects are created, the object is created along with the
                // directory entry in the same transaction, and so we need to hold a read lock
                // over the lookup and open calls.
                Right(fs.lock_manager().read_lock(keys).await)
            };

            let is_casefold = current_dir.directory.dir_type().is_casefold();
            let child_descriptor = {
                match self.directory.owner().dirent_cache().lookup(&(
                    current_dir.object_id(),
                    name,
                    is_casefold,
                )) {
                    Some(node) => {
                        let desc = node.object_descriptor();
                        Some((node, desc))
                    }
                    None => {
                        if let Some((object_id, object_descriptor, locked)) =
                            current_dir.directory.lookup(name).await?
                        {
                            let child_node = self
                                .volume()
                                .get_or_load_node(
                                    object_id,
                                    object_descriptor.clone(),
                                    Some(current_dir.clone()),
                                )
                                .await?;
                            // Do not add a locked encrypted child to the dirent cache. That way, if
                            // a user opens a locked directory, unlocks the directory, and then
                            // reopens the directory, fxfs does not return the cached locked node.
                            if !locked {
                                self.directory.owner().dirent_cache().insert(
                                    DirentCacheKey::new(
                                        current_dir.object_id(),
                                        name.to_owned(),
                                        is_casefold,
                                    ),
                                    child_node.clone(),
                                );
                            }
                            Some((child_node, object_descriptor))
                        } else {
                            None
                        }
                    }
                }
            };

            match child_descriptor {
                Some((child_node, object_descriptor)) => {
                    if transaction_or_guard.is_left()
                        && protocols.creation_mode() == vfs::CreationMode::Always
                    {
                        bail!(FxfsError::AlreadyExists);
                    }
                    if last_segment {
                        if protocols.create_unnamed_temporary_in_directory_path() {
                            if !matches!(object_descriptor, ObjectDescriptor::Directory) {
                                bail!(FxfsError::WrongType);
                            }
                            let dir = child_node
                                .into_any()
                                .downcast::<FxDirectory>()
                                .map_err(|_| FxfsError::Inconsistent)?;
                            return dir
                                .create_unnamed_temporary_file(request.create_attributes())
                                .await;
                        }

                        match object_descriptor {
                            ObjectDescriptor::Directory => {
                                if !protocols.is_node() && !protocols.is_dir_allowed() {
                                    if protocols.is_file_allowed() {
                                        bail!(FxfsError::NotFile)
                                    } else {
                                        bail!(FxfsError::WrongType)
                                    }
                                }
                            }
                            ObjectDescriptor::File => {
                                if !protocols.is_node() && !protocols.is_file_allowed() {
                                    if protocols.is_dir_allowed() {
                                        bail!(FxfsError::NotDir)
                                    } else {
                                        bail!(FxfsError::WrongType)
                                    }
                                }
                            }
                            ObjectDescriptor::Symlink => {
                                if !protocols.is_node() && !protocols.is_symlink_allowed() {
                                    bail!(FxfsError::WrongType)
                                }
                            }
                            ObjectDescriptor::Volume => bail!(FxfsError::Inconsistent),
                        }
                    }
                    current_node = child_node;
                    if last_segment {
                        // We must make sure to take an open-count whilst we are holding a read
                        // lock.
                        return Ok(OpenedNode::new(current_node));
                    }
                }
                None => {
                    if let Left(mut transaction) = transaction_or_guard {
                        let new_node = current_dir
                            .create_child(
                                &mut transaction,
                                name,
                                protocols.create_directory(),
                                request.create_attributes(),
                            )
                            .await?;
                        if let GetResult::Placeholder(p) =
                            self.volume().cache().get_or_reserve(new_node.object_id()).await
                        {
                            return transaction
                                .commit_with_callback(|_| {
                                    p.commit(&new_node);
                                    current_dir.did_add(name, Some(new_node.clone()));
                                    // NOTE: We don't take the open count until here in case the
                                    // transaction fails.
                                    OpenedNode::new(new_node)
                                })
                                .await;
                        } else {
                            // We created a node, but the object ID was already used in the cache,
                            // which suggests a object ID was reused (which would either be a bug or
                            // corruption).
                            bail!(FxfsError::Inconsistent);
                        }
                    } else {
                        bail!(FxfsError::NotFound);
                    }
                }
            };
        }
    }

    async fn create_child(
        self: &Arc<Self>,
        transaction: &mut Transaction<'_>,
        name: &str,
        create_dir: bool, // If false, creates a file.
        create_attributes: Option<&fio::MutableNodeAttributes>,
    ) -> Result<Arc<dyn FxNode>, Error> {
        if create_dir {
            let dir = Arc::new(FxDirectory::new(
                Some(self.clone()),
                self.directory.create_child_dir(transaction, name).await?,
            ));
            if let Some(attrs) = create_attributes {
                dir.directory().handle().update_attributes(transaction, Some(&attrs), None).await?;
            }
            Ok(dir as Arc<dyn FxNode>)
        } else {
            let file = FxFile::new(self.directory.create_child_file(transaction, name).await?);
            if let Some(attrs) = create_attributes {
                file.handle()
                    .uncached_handle()
                    .update_attributes(transaction, Some(&attrs), None)
                    .await?;
            }
            Ok(file as Arc<dyn FxNode>)
        }
    }

    pub(crate) async fn create_unnamed_temporary_file(
        self: &Arc<Self>,
        create_attributes: Option<&fio::MutableNodeAttributes>,
    ) -> Result<OpenedNode<dyn FxNode>, Error> {
        let store = self.store();
        let keys = lock_keys![LockKey::object(store.store_object_id(), self.directory.object_id())];
        let mut transaction = store.new_transaction(keys, Options::default()).await?;
        let file = FxFile::new(
            self.directory.create_child_unnamed_temporary_file(&mut transaction).await?,
        );
        if let Some(attrs) = create_attributes {
            file.handle()
                .uncached_handle()
                .update_attributes(&mut transaction, Some(&attrs), None)
                .await?;
        }
        let GetResult::Placeholder(p) =
            self.volume().cache().get_or_reserve(file.object_id()).await
        else {
            bail!(FxfsError::Inconsistent);
        };
        transaction
            .commit_with_callback(|_| {
                let file = file.open_as_temporary();
                p.commit(&file);
                file
            })
            .await
    }

    fn remove_from_dirent_cache(&self, name: &str) {
        let is_casefold = self.directory.dir_type().is_casefold();
        self.directory.owner().dirent_cache().remove(&(
            self.directory.object_id(),
            name,
            is_casefold,
        ));
    }

    /// Called to indicate a file or directory was removed from this directory.
    pub(crate) fn did_remove(&self, name: &str) {
        self.remove_from_dirent_cache(name);
        self.watchers.lock().send_event(&mut SingleNameEventProducer::removed(name));
    }

    /// Called to indicate a file or directory was added to this directory.
    pub(crate) fn did_add(&self, name: &str, node: Option<Arc<dyn FxNode>>) {
        if let Some(node) = node {
            let is_casefold = self.directory.dir_type().is_casefold();
            self.directory.owner().dirent_cache().insert(
                DirentCacheKey::new(self.directory.object_id(), name.to_owned(), is_casefold),
                node,
            );
        }
        self.watchers.lock().send_event(&mut SingleNameEventProducer::added(name));
    }

    /// As per fscrypt, files cannot be moved or linked across different encryption policies.
    /// Appropriate locks must be held by the caller.
    pub fn check_fscrypt_policy_equivalence(
        &self,
        source_wrapping_key_id: Option<WrappingKeyId>,
    ) -> Result<(), zx::Status> {
        if let Some(target_id) = self.directory().dir_type().wrapping_key_id() {
            if Some(target_id) != source_wrapping_key_id {
                return Err(zx::Status::BAD_STATE);
            }
        }
        Ok(())
    }

    pub(crate) async fn link_object(
        &self,
        mut transaction: Transaction<'_>,
        name: &str,
        source_id: u64,
        kind: ObjectDescriptor,
    ) -> Result<(), zx::Status> {
        let store = self.store();
        if self.is_deleted() {
            return Err(zx::Status::ACCESS_DENIED);
        }
        if self.directory.lookup(&name).await.map_err(map_to_status)?.is_some() {
            return Err(zx::Status::ALREADY_EXISTS);
        }
        self.directory
            .insert_child(&mut transaction, &name, source_id, kind.clone())
            .await
            .map_err(map_to_status)?;
        store.adjust_refs(&mut transaction, source_id, 1).await.map_err(map_to_status)?;
        transaction
            .commit_with_callback(|_| self.did_add(&name, None))
            .await
            .map_err(map_to_status)?;
        Ok(())
    }

    // Move graveyard object out from the graveyard and link it to this path. We only expect to do
    // this when linking an unnamed temporary file for the first time.
    pub(crate) async fn link_graveyard_object<F>(
        &self,
        mut transaction: Transaction<'_>,
        name: &str,
        source_id: u64,
        kind: ObjectDescriptor,
        transaction_callback: F,
    ) -> Result<(), zx::Status>
    where
        F: FnOnce() + Send,
    {
        let store = self.store();
        if self.is_deleted() {
            return Err(zx::Status::ACCESS_DENIED);
        }
        if self.directory.lookup(&name).await.map_err(map_to_status)?.is_some() {
            return Err(zx::Status::ALREADY_EXISTS);
        }
        // Move object out from the graveyard and place into record. As we are moving the object
        // from one record to the other, the reference count should stay the same.
        store.remove_from_graveyard(&mut transaction, source_id);
        self.directory
            .insert_child(&mut transaction, &name, source_id, kind.clone())
            .await
            .map_err(map_to_status)?;
        transaction
            .commit_with_callback(|_| {
                transaction_callback();
                self.did_add(&name, None);
            })
            .await
            .map_err(map_to_status)?;
        Ok(())
    }

    async fn link_impl(
        self: Arc<Self>,
        name: String,
        source_dir: Arc<dyn Any + Send + Sync>,
        source_name: &str,
    ) -> Result<(), zx::Status> {
        let source_dir = source_dir.downcast::<Self>().unwrap();
        let store = self.store();
        let mut source_id =
            match source_dir.directory.lookup(source_name).await.map_err(map_to_status)? {
                Some((object_id, ObjectDescriptor::File, _)) => object_id,
                None => return Err(zx::Status::NOT_FOUND),
                _ => return Err(zx::Status::NOT_SUPPORTED),
            };
        loop {
            // We don't need a lock on the source directory, as it will be unchanged (unless it is
            // the same as the destination directory). We just need a lock on the source object to
            // ensure that it hasn't been simultaneously unlinked. This may race with a rename of
            // the source file to somewhere else but that shouldn't matter. We need that lock anyway
            // to update the ref count. Note, fscrypt does not require the source directory to be
            // locked because a directory's wrapping key cannot change once the directory has
            // entries.
            let transaction = store
                .new_transaction(
                    lock_keys![
                        LockKey::object(store.store_object_id(), self.object_id()),
                        LockKey::object(store.store_object_id(), source_id),
                    ],
                    Options::default(),
                )
                .await
                .map_err(map_to_status)?;
            self.check_fscrypt_policy_equivalence(source_dir.directory().wrapping_key_id())?;
            // Ensure under lock that the file still exists there.
            match source_dir.directory.lookup(source_name).await.map_err(map_to_status)? {
                Some((new_id, ObjectDescriptor::File, _)) => {
                    if new_id == source_id {
                        // We found the object that we got a lock on, it is still valid.
                        return self
                            .link_object(transaction, &name, source_id, ObjectDescriptor::File)
                            .await;
                    } else {
                        source_id = new_id
                    }
                }
                None => return Err(zx::Status::NOT_FOUND),
                _ => return Err(zx::Status::NOT_SUPPORTED),
            }
        }
    }

    /// Acquires the transaction and executes `directory::replace_child_with_purge`.
    ///
    /// Returns `Ok(None)` if this is a trivial no-op rename (`src_dir == self` and `src == dst`).
    ///
    /// To avoid holding a directory `WriteLock` across extent-trimming disk I/O (which would
    /// block concurrent lookups), this method optimistically attempts a single-transaction purge
    /// while holding normal transaction locks, and retries once falling back to the graveyard if
    /// a concurrent lookup opens the target file during the I/O window.
    async fn replace_child_with_purge<'a>(
        self: &'a Arc<Self>,
        src: Option<(&'a Arc<FxDirectory>, &'a str)>,
        dst: &'a str,
        must_be_directory: bool,
    ) -> Result<Option<ReplaceWithPurgeResult<'a>>, zx::Status> {
        let mut allow_purge = true;
        loop {
            let borrow_metadata_space = src.is_none();
            // Acquire the transaction that locks |src_dir|, |src_name|, |self|, and |dst_name| if
            // they exist, and also the ID and type of dst and src.
            let replace_context = self
                .directory
                .acquire_context_for_replace(
                    src.map(|(dir, name)| (dir.directory(), name)),
                    dst,
                    borrow_metadata_space,
                )
                .await
                .map_err(map_to_status)?;
            let mut transaction = replace_context.transaction;

            let (moved_node, actual_src_name) = if let Some((src_dir, src_name)) = src {
                if self.is_deleted() {
                    return Err(zx::Status::NOT_FOUND);
                }

                let (moved_id, moved_descriptor) =
                    replace_context.src_id_and_descriptor.clone().ok_or(zx::Status::NOT_FOUND)?;

                // Make sure the dst path is compatible with the moved node.
                if let ObjectDescriptor::File = moved_descriptor {
                    if must_be_directory {
                        return Err(zx::Status::NOT_DIR);
                    }
                }

                // Now that we've ensured that the dst path is compatible with the moved node, we
                // can check for the trivial case.
                if src_dir.object_id() == self.object_id() && src_name == dst {
                    return Ok(None);
                }

                if let Some((_, dst_descriptor)) = replace_context.dst_id_and_descriptor.as_ref() {
                    // dst is being overwritten; make sure it's a file iff src is.
                    match (&moved_descriptor, dst_descriptor) {
                        (ObjectDescriptor::Directory, ObjectDescriptor::Directory) => {}
                        (
                            ObjectDescriptor::File | ObjectDescriptor::Symlink,
                            ObjectDescriptor::File | ObjectDescriptor::Symlink,
                        ) => {}
                        (ObjectDescriptor::Directory, _) => return Err(zx::Status::NOT_DIR),
                        (ObjectDescriptor::File | ObjectDescriptor::Symlink, _) => {
                            return Err(zx::Status::NOT_FILE);
                        }
                        _ => return Err(zx::Status::IO_DATA_INTEGRITY),
                    }
                }

                let moved_node = src_dir
                    .volume()
                    .get_or_load_node(moved_id, moved_descriptor.clone(), Some(src_dir.clone()))
                    .await
                    .map_err(map_to_status)?;

                if let ObjectDescriptor::Directory = moved_descriptor {
                    // Lastly, ensure that self isn't a (transitive) child of the moved node.
                    let mut node_opt = Some(self.clone());
                    while let Some(node) = node_opt {
                        if node.object_id() == moved_node.object_id() {
                            return Err(zx::Status::INVALID_ARGS);
                        }
                        node_opt = node.parent();
                    }
                }

                // Use name from the replace_context if available (which preserves case-folding
                // info). `src_name` comes from user supplied name which may have different case.
                let actual_src_name =
                    replace_context.src_name.as_deref().unwrap_or(src_name).to_owned();
                (Some(moved_node), actual_src_name)
            } else {
                let (_child_id, object_descriptor) =
                    replace_context.dst_id_and_descriptor.clone().ok_or(zx::Status::NOT_FOUND)?;
                if let ObjectDescriptor::Directory = object_descriptor {
                } else if must_be_directory {
                    return Err(zx::Status::NOT_DIR);
                }
                (None, String::new())
            };

            // Use name from the replace_context if available (which preserves case-folding info).
            // `dst` is user supplied name and may have different case.
            let actual_dst_name = replace_context.dst_name.as_deref().unwrap_or(dst).to_owned();

            // Closed files are flushed on close (`SyncMode::PreClose`), so `dirent_cache`
            // typically holds the last strong `Arc<dyn FxNode>`. Evicting it here synchronously
            // removes the node from `NodeCache`.
            if allow_purge && replace_context.dst_id_and_descriptor.is_some() {
                self.remove_from_dirent_cache(&actual_dst_name);
            }
            let can_purge = allow_purge
                && match replace_context.dst_id_and_descriptor.as_ref() {
                    Some((dst_id, ObjectDescriptor::File | ObjectDescriptor::Symlink)) => {
                        !self.volume().cache().contains_key(*dst_id)
                    }
                    _ => false,
                };

            let replace_result = directory::replace_child_with_purge(
                &mut transaction,
                src.map(|(dir, name)| (dir.directory(), name)),
                (self.directory(), dst),
                can_purge,
            )
            .await
            .map_err(map_to_status)?;

            if let ReplacedChild::Purged(id) = replace_result {
                // Now that extent-trimming disk I/O is complete, upgrade locks to `WriteLock` and
                // wait for active readers to drain so no new lookups can start. If a concurrent
                // lookup loaded the node into `NodeCache` (or repopulated `dirent_cache`) during
                // the I/O window, drop the uncommitted transaction and retry once via the
                // graveyard.
                transaction.commit_prepare().await;
                if self.volume().cache().contains_key(id) {
                    allow_purge = false;
                    continue;
                }
            }
            return Ok(Some(ReplaceWithPurgeResult {
                transaction,
                truncate_guard: replace_context.truncate_guard,
                replace_result,
                moved_node,
                actual_src_name,
                actual_dst_name,
            }));
        }
    }

    async fn rename_impl(
        self: Arc<Self>,
        src_dir: Arc<dyn MutableDirectory>,
        src_name: Path,
        dst_name: Path,
    ) -> Result<(), zx::Status> {
        if !src_name.is_single_component() || !dst_name.is_single_component() {
            return Err(zx::Status::INVALID_ARGS);
        }
        let (src, dst) = (src_name.peek().unwrap(), dst_name.peek().unwrap());
        let src_dir =
            src_dir.into_any().downcast::<FxDirectory>().map_err(|_| zx::Status::NOT_DIR)?;

        let Some(ReplaceWithPurgeResult {
            transaction,
            truncate_guard,
            replace_result,
            moved_node,
            actual_src_name,
            actual_dst_name,
        }) = self
            .replace_child_with_purge(
                Some((&src_dir, src)),
                dst,
                src_name.is_dir() || dst_name.is_dir(),
            )
            .await?
        else {
            return Ok(());
        };
        let moved_node = moved_node.unwrap();

        transaction
            .commit_with_callback(|_| {
                moved_node.set_parent(self.clone());
                src_dir.did_remove(&actual_src_name);

                match replace_result {
                    ReplacedChild::None => {}
                    ReplacedChild::ObjectWithRemainingLinks(..)
                    | ReplacedChild::Object(_)
                    | ReplacedChild::Purged(_) => {
                        self.did_remove(&actual_dst_name);
                    }
                    ReplacedChild::Directory(id) => {
                        let store = self.store();
                        store
                            .filesystem()
                            .graveyard()
                            .queue_tombstone_object(store.store_object_id(), id);
                        self.did_remove(&actual_dst_name);
                        self.volume().mark_directory_deleted(id);
                    }
                }
                self.did_add(dst, Some(moved_node));
            })
            .await
            .map_err(map_to_status)?;

        if let ReplacedChild::Object(id) = replace_result {
            self.volume()
                .maybe_purge_file(id, truncate_guard.as_ref())
                .await
                .map_err(map_to_status)?;
        }
        Ok(())
    }

    pub(crate) async fn open_block_file(
        self: &Arc<Self>,
        name: &str,
        server_end: ServerEnd<BlockMarker>,
    ) {
        let request = ObjectRequest::new(
            fio::Flags::empty(),
            &fio::Options::default(),
            server_end.into_channel(),
        );
        let scope = self.volume().scope().clone();
        let this = self.clone();
        request
            .handle_async(async move |request| {
                let path = Path::validate_and_split(name).and_then(|p| {
                    if p.is_single_component() { Ok(p) } else { Err(zx::Status::INVALID_ARGS) }
                })?;
                let node = this
                    .lookup(&fio::Flags::empty(), path, request)
                    .await
                    .map_err(map_to_status)?;
                if node.is::<FxFile>() {
                    let file = node.downcast::<FxFile>().unwrap_or_else(|_| unreachable!());
                    if file.is_verified_file() {
                        log::error!("Tried to expose a verified file as a block device.");
                        return Err(zx::Status::NOT_SUPPORTED);
                    }
                    let server = BlockServer::new(file, request.take().into_channel());
                    scope.spawn(server.run());
                    Ok(())
                } else {
                    Err(zx::Status::NOT_FILE)
                }
            })
            .await
    }
}

impl Drop for FxDirectory {
    fn drop(&mut self) {
        self.volume().cache().remove(self);
    }
}

impl FxNode for FxDirectory {
    fn object_id(&self) -> u64 {
        self.directory.object_id()
    }

    fn parent(&self) -> Option<Arc<FxDirectory>> {
        self.parent.as_ref().map(|p| p.lock().clone())
    }

    fn set_parent(&self, parent: Arc<FxDirectory>) {
        match &self.parent {
            Some(p) => *p.lock() = parent,
            None => panic!("Called set_parent on root node"),
        }
    }

    // If these ever do anything, BlobDirectory might need to be fixed.
    fn open_count_add_one(&self) {}
    fn open_count_sub_one(self: Arc<Self>) {}

    fn object_descriptor(&self) -> ObjectDescriptor {
        ObjectDescriptor::Directory
    }
}

impl MutableDirectory for FxDirectory {
    fn link<'a>(
        self: Arc<Self>,
        name: String,
        source_dir: Arc<dyn Any + Send + Sync>,
        source_name: &'a str,
    ) -> BoxFuture<'a, Result<(), zx::Status>> {
        Box::pin(self.link_impl(name, source_dir, source_name))
    }

    async fn unlink(
        self: Arc<Self>,
        name: &str,
        must_be_directory: bool,
    ) -> Result<(), zx::Status> {
        let ReplaceWithPurgeResult {
            transaction,
            truncate_guard,
            replace_result,
            actual_dst_name,
            ..
        } = self
            .replace_child_with_purge(None, name, must_be_directory)
            .await?
            .expect("unlink cannot be a trivial rename");

        match replace_result {
            ReplacedChild::None => return Err(zx::Status::NOT_FOUND),
            ReplacedChild::ObjectWithRemainingLinks(..) | ReplacedChild::Purged(..) => {
                transaction
                    .commit_with_callback(|_| self.did_remove(&actual_dst_name))
                    .await
                    .map_err(map_to_status)?;
            }
            ReplacedChild::Object(id) => {
                transaction
                    .commit_with_callback(|_| self.did_remove(&actual_dst_name))
                    .await
                    .map_err(map_to_status)?;

                // If purging fails, we should still return success, since the file will appear
                // unlinked at this point anyways.  The file should be cleaned up on a later mount.
                if let Err(e) = self.volume().maybe_purge_file(id, truncate_guard.as_ref()).await {
                    warn!(error:? = e; "Failed to purge file");
                }
            }
            ReplacedChild::Directory(id) => {
                transaction
                    .commit_with_callback(|_| {
                        let store = self.store();
                        store
                            .filesystem()
                            .graveyard()
                            .queue_tombstone_object(store.store_object_id(), id);
                        self.did_remove(&actual_dst_name);
                        self.volume().mark_directory_deleted(id);
                    })
                    .await
                    .map_err(map_to_status)?;
            }
        }
        Ok(())
    }

    async fn update_attributes(
        &self,
        attributes: fio::MutableNodeAttributes,
    ) -> Result<(), zx::Status> {
        // TODO(b/365630582): Reconsider doing this as part of the transaction below.
        if let Some(casefold) = attributes.casefold {
            self.directory.set_casefold(casefold).await.map_err(map_to_status)?;
        }
        let transaction = self
            .store()
            .new_transaction(
                lock_keys![LockKey::object(
                    self.store().store_object_id(),
                    self.directory.object_id()
                )],
                Options { borrow_metadata_space: true, ..Default::default() },
            )
            .await
            .map_err(map_to_status)?;

        self.directory
            .update_attributes(transaction, Some(&attributes), 0, Some(Timestamp::now()))
            .await
            .map_err(map_to_status)?;
        Ok(())
    }

    async fn sync(&self) -> Result<(), zx::Status> {
        // FDIO implements `syncfs` by calling sync on a directory, so replicate that behaviour.
        self.volume()
            .store()
            .filesystem()
            .sync(SyncOptions { flush_device: true, ..Default::default() })
            .await
            .map_err(map_to_status)
    }

    fn rename(
        self: Arc<Self>,
        src_dir: Arc<dyn MutableDirectory>,
        src_name: Path,
        dst_name: Path,
    ) -> BoxFuture<'static, Result<(), zx::Status>> {
        Box::pin(self.rename_impl(src_dir, src_name, dst_name))
    }

    async fn create_symlink(
        &self,
        name: String,
        target: Vec<u8>,
        connection: Option<ServerEnd<fio::SymlinkMarker>>,
    ) -> Result<(), zx::Status> {
        let store = self.store();
        let dir = &self.directory;
        let keys = lock_keys![LockKey::object(store.store_object_id(), dir.object_id())];
        let mut transaction =
            store.new_transaction(keys, Options::default()).await.map_err(map_to_status)?;
        if dir.lookup(&name).await.map_err(map_to_status)?.is_some() {
            return Err(zx::Status::ALREADY_EXISTS);
        }
        let object_id =
            dir.create_symlink(&mut transaction, &target, &name).await.map_err(map_to_status)?;
        if let Some(connection) = connection {
            if let GetResult::Placeholder(p) = self.volume().cache().get_or_reserve(object_id).await
            {
                transaction
                    .commit_with_callback(|_| {
                        let node = Arc::new(FxSymlink::new(self.volume().clone(), object_id));
                        p.commit(&(node.clone() as Arc<dyn FxNode>));
                        let scope = self.volume().scope().clone();
                        let flags =
                            fio::Flags::PROTOCOL_SYMLINK | fio::PERM_READABLE | fio::PERM_WRITABLE;
                        // Wrap in OpenedNode to set open_count to 1 for the connection.
                        let opened_node = OpenedNode::new(node);
                        // fio::Flags::FLAG_SEND_REPRESENTATION isn't specified so connection
                        // creation is synchronous.
                        symlink::Connection::create_sync(
                            scope,
                            opened_node.take(),
                            flags,
                            flags.to_object_request(connection),
                        );
                    })
                    .await
            } else {
                // The node already exists in the cache which could only happen if the filesystem is
                // corrupt.
                return Err(zx::Status::IO_DATA_INTEGRITY);
            }
        } else {
            transaction.commit().await.map(|_| ())
        }
        .map_err(map_to_status)
    }
}

impl DirectoryEntry for FxDirectory {
    fn open_entry(self: Arc<Self>, request: OpenRequest<'_>) -> Result<(), zx::Status> {
        request.open_dir(self)
    }

    fn scope(&self) -> Option<ExecutionScope> {
        Some(self.volume().scope().clone())
    }
}

impl GetEntryInfo for FxDirectory {
    fn entry_info(&self) -> EntryInfo {
        EntryInfo::new(self.object_id(), fio::DirentType::Directory)
    }
}

impl vfs::node::Node for FxDirectory {
    async fn get_attributes(
        &self,
        requested_attributes: fio::NodeAttributesQuery,
    ) -> Result<fio::NodeAttributes2, zx::Status> {
        let mut props = self.directory.get_properties().await.map_err(map_to_status)?;

        if requested_attributes.contains(fio::NodeAttributesQuery::PENDING_ACCESS_TIME_UPDATE) {
            self.store()
                .update_access_time(self.directory.object_id(), &mut props, || !self.is_deleted())
                .await
                .map_err(map_to_status)?;
        }

        Ok(attributes!(
            requested_attributes,
            Mutable {
                creation_time: props.creation_time.as_nanos(),
                modification_time: props.modification_time.as_nanos(),
                access_time: props.access_time.as_nanos(),
                mode: props.posix_attributes.map(|a| a.mode),
                uid: props.posix_attributes.map(|a| a.uid),
                gid: props.posix_attributes.map(|a| a.gid),
                rdev: props.posix_attributes.map(|a| a.rdev),
                casefold: self.directory.dir_type().is_casefold(),
                selinux_context: self
                    .directory
                    .handle()
                    .get_inline_selinux_context()
                    .await
                    .map_err(map_to_status)?,
                wrapping_key_id: props.dir_type.wrapping_key_id(),
            },
            Immutable {
                protocols: fio::NodeProtocolKinds::DIRECTORY,
                abilities: fio::Operations::GET_ATTRIBUTES
                    | fio::Operations::UPDATE_ATTRIBUTES
                    | fio::Operations::ENUMERATE
                    | fio::Operations::TRAVERSE
                    | fio::Operations::MODIFY_DIRECTORY,
                content_size: props.data_attribute_size,
                storage_size: props.allocated_size,
                link_count: props.refs + 1 + props.sub_dirs,
                id: self.directory.object_id(),
                change_time: props.change_time.as_nanos(),
                verity_enabled: false,
            }
        ))
    }

    fn query_filesystem(&self) -> Result<fio::FilesystemInfo, zx::Status> {
        Ok(self.volume().filesystem_info_for_volume())
    }

    async fn list_extended_attributes(&self) -> Result<Vec<Vec<u8>>, zx::Status> {
        self.directory.list_extended_attributes().await.map_err(map_to_status)
    }

    async fn get_extended_attribute(&self, name: Vec<u8>) -> Result<Vec<u8>, zx::Status> {
        self.directory.get_extended_attribute(name).await.map_err(map_to_status)
    }

    async fn set_extended_attribute(
        &self,
        name: Vec<u8>,
        value: Vec<u8>,
        mode: fio::SetExtendedAttributeMode,
    ) -> Result<(), zx::Status> {
        self.directory.set_extended_attribute(name, value, mode.into()).await.map_err(map_to_status)
    }

    async fn remove_extended_attribute(&self, name: Vec<u8>) -> Result<(), zx::Status> {
        self.directory.remove_extended_attribute(name).await.map_err(map_to_status)
    }
}

impl VfsDirectory for FxDirectory {
    fn deprecated_open(
        self: Arc<Self>,
        scope: ExecutionScope,
        flags: fio::OpenFlags,
        path: Path,
        server_end: ServerEnd<fio::NodeMarker>,
    ) {
        scope.clone().spawn(flags.to_object_request(server_end).handle_async(
            async move |object_request| {
                let node =
                    self.lookup(&flags, path, object_request).await.map_err(map_to_status)?;
                if node.is::<FxDirectory>() {
                    let directory =
                        node.downcast::<FxDirectory>().unwrap_or_else(|_| unreachable!()).take();
                    object_request
                        .create_connection::<MutableConnection<_>, _>(scope, directory, flags)
                        .await
                } else if node.is::<FxFile>() {
                    let node = node.downcast::<FxFile>().unwrap_or_else(|_| unreachable!());
                    if flags.contains(fio::OpenFlags::BLOCK_DEVICE) {
                        if node.is_verified_file() {
                            log::error!("Tried to expose a verified file as a block device.");
                            return Err(zx::Status::NOT_SUPPORTED);
                        }
                        if !flags.contains(fio::OpenFlags::RIGHT_READABLE) {
                            log::error!(
                                "Opening a file as block device requires at least RIGHT_READABLE."
                            );
                            return Err(zx::Status::ACCESS_DENIED);
                        }
                        let server = BlockServer::new(node, object_request.take().into_channel());
                        scope.spawn(server.run());
                        Ok(())
                    } else {
                        FxFile::create_connection_async(node, scope, flags, object_request).await
                    }
                } else if node.is::<FxSymlink>() {
                    let node = node.downcast::<FxSymlink>().unwrap_or_else(|_| unreachable!());
                    object_request
                        .create_connection::<symlink::Connection<_>, _>(
                            scope.clone(),
                            node.take(),
                            flags,
                        )
                        .await
                } else {
                    unreachable!();
                }
            },
        ));
    }

    fn open(
        self: Arc<Self>,
        scope: ExecutionScope,
        path: Path,
        flags: fio::Flags,
        object_request: ObjectRequestRef<'_>,
    ) -> Result<(), zx::Status> {
        self.volume().scope().clone().spawn(object_request.take().handle_async(
            async move |object_request| self.open_async(scope, path, flags, object_request).await,
        ));
        Ok(())
    }

    async fn open_async(
        self: Arc<Self>,
        scope: ExecutionScope,
        path: Path,
        flags: fio::Flags,
        object_request: ObjectRequestRef<'_>,
    ) -> Result<(), zx::Status> {
        let node = self.lookup(&flags, path, object_request).await.map_err(map_to_status)?;
        if node.is::<FxDirectory>() {
            let directory =
                node.downcast::<FxDirectory>().unwrap_or_else(|_| unreachable!()).take();
            object_request
                .create_connection::<MutableConnection<_>, _>(scope, directory, flags)
                .await
        } else if node.is::<FxFile>() {
            let file = node.downcast::<FxFile>().unwrap_or_else(|_| unreachable!());
            FxFile::create_connection_async(file, scope, flags, object_request).await
        } else if node.is::<FxSymlink>() {
            let symlink = node.downcast::<FxSymlink>().unwrap_or_else(|_| unreachable!());
            object_request
                .create_connection::<symlink::Connection<_>, _>(
                    scope.clone(),
                    symlink.take(),
                    flags,
                )
                .await
        } else {
            unreachable!();
        }
    }

    async fn read_dirents(
        &self,
        pos: &TraversalPosition,
        mut sink: Box<dyn dirents_sink::Sink>,
    ) -> Result<(TraversalPosition, Box<dyn dirents_sink::Sealed>), zx::Status> {
        if let TraversalPosition::End = pos {
            return Ok((TraversalPosition::End, sink.seal()));
        } else if let TraversalPosition::Index(_) = pos {
            // The VFS should never send this to us, since we never return it here.
            return Err(zx::Status::BAD_STATE);
        }

        let store = self.store();
        let fs = store.filesystem();
        let _read_guard = fs
            .lock_manager()
            .read_lock(lock_keys![LockKey::object(store.store_object_id(), self.object_id())])
            .await;
        if self.is_deleted() {
            return Ok((TraversalPosition::End, sink.seal()));
        }

        let layer_set = self.store().tree().layer_set();
        let mut merger = layer_set.merger();
        let mut iter = match pos {
            TraversalPosition::Start => {
                // Synthesize a "." entry if we're at the start of the stream.
                match sink
                    .append(&EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::Directory), ".")
                {
                    AppendResult::Ok(new_sink) => sink = new_sink,
                    AppendResult::Sealed(sealed) => {
                        // Note that the VFS should have yielded an error since the first entry
                        // didn't fit. This is defensive in case the VFS' behaviour changes, so that
                        // we return a reasonable value.
                        return Ok((TraversalPosition::Start, sealed));
                    }
                }
                self.directory.iter(&mut merger).await
            }
            TraversalPosition::Name(name) => self.directory.iter_from(&mut merger, name).await,
            TraversalPosition::Bytes(bytes) => {
                self.directory.iter_from_bytes(&mut merger, bytes).await
            }
            _ => unreachable!(),
        }
        .map_err(map_to_status)?;
        while let Some((name, object_id, object_descriptor)) = iter.get() {
            let entry_type = match object_descriptor {
                ObjectDescriptor::File => fio::DirentType::File,
                ObjectDescriptor::Directory => fio::DirentType::Directory,
                ObjectDescriptor::Symlink => fio::DirentType::Symlink,
                ObjectDescriptor::Volume => return Err(zx::Status::IO_DATA_INTEGRITY),
            };

            let info = EntryInfo::new(object_id, entry_type);
            match sink.append(&info, &name) {
                AppendResult::Ok(new_sink) => sink = new_sink,
                AppendResult::Sealed(sealed) => {
                    // We did *not* add the current entry to the sink (e.g. because the sink was
                    // full), so mark |name| as the next position so that it's the first entry we
                    // process on a subsequent call of read_dirents.
                    // Note that entries inserted between the previous entry and this entry before
                    // the next call to read_dirents would not be included in the results (but
                    // there's no requirement to include them anyways).
                    return Ok((
                        iter.traversal_position(
                            |name| TraversalPosition::Name(name.to_string()),
                            |bytes| TraversalPosition::Bytes(bytes),
                        )
                        .unwrap(),
                        sealed,
                    ));
                }
            }
            iter.advance().await.map_err(map_to_status)?;
        }

        Ok((TraversalPosition::End, sink.seal()))
    }

    fn register_watcher(
        self: Arc<Self>,
        scope: ExecutionScope,
        mask: fio::WatchMask,
        watcher: DirectoryWatcher,
    ) -> Result<(), zx::Status> {
        let controller =
            self.watchers.lock().add(scope.clone(), self.clone(), mask, watcher).clone();
        if mask.contains(fio::WatchMask::EXISTING) && !self.is_deleted() {
            scope.spawn(async move {
                let layer_set = self.store().tree().layer_set();
                let mut merger = layer_set.merger();
                let mut iter = match self.directory.iter_from(&mut merger, "").await {
                    Ok(iter) => iter,
                    Err(e) => {
                        error!(error:? = e; "Failed to iterate directory for watch",);
                        // TODO(https://fxbug.dev/42178164): This really should close the watcher connection
                        // with an epitaph so that the watcher knows.
                        return;
                    }
                };
                // TODO(https://fxbug.dev/42178165): It is possible that we'll duplicate entries that are added
                // as we iterate over directories.  I suspect fixing this might be non-trivial.
                controller.send_event(&mut SingleNameEventProducer::existing("."));
                while let Some((name, _, _)) = iter.get() {
                    controller.send_event(&mut SingleNameEventProducer::existing(name));
                    if let Err(e) = iter.advance().await {
                        error!(error:? = e; "Failed to iterate directory for watch",);
                        return;
                    }
                }
                controller.send_event(&mut SingleNameEventProducer::idle());
            });
        }
        Ok(())
    }

    fn unregister_watcher(self: Arc<Self>, key: usize) {
        self.watchers.lock().remove(key);
    }
}

impl From<Directory<FxVolume>> for FxDirectory {
    fn from(dir: Directory<FxVolume>) -> Self {
        Self::new(None, dir)
    }
}

#[cfg(test)]
mod tests {
    use crate::directory::FxDirectory;
    use crate::file::FxFile;
    use crate::fuchsia::testing::{
        TestFixture, TestFixtureOptions, close_dir_checked, close_file_checked, open_dir,
        open_dir_checked, open_file, open_file_checked,
    };
    use anyhow::bail;
    use assert_matches::assert_matches;
    use fidl::endpoints::{ClientEnd, Proxy, create_proxy};
    use fidl_fuchsia_io as fio;
    use fuchsia_async as fasync;
    use fuchsia_fs::directory::{DirEntry, DirentKind, WatchEvent, WatchMessage, Watcher};
    use fuchsia_fs::file;
    use futures::{StreamExt, join};
    use fxfs::lsm_tree::Query;
    use fxfs::lsm_tree::types::{ItemRef, LayerIterator};
    use fxfs::object_store::transaction::{LockKey, lock_keys};
    use fxfs::object_store::{ObjectKey, ObjectKeyData, ObjectValue, Timestamp};
    use fxfs_crypt_common::CryptBase;
    use fxfs_crypto::{FSCRYPT_PADDING, WrappingKeyId};
    use std::future::poll_fn;
    use std::os::fd::AsRawFd;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::task::Poll;
    use std::time::Duration;
    use storage_device::DeviceHolder;
    use storage_device::fake_device::FakeDevice;
    use vfs::ObjectRequest;
    use vfs::node::Node;
    use vfs::path::Path;

    const WRAPPING_KEY_ID: WrappingKeyId = u128::to_le_bytes(2);

    async fn yield_to_executor() {
        let mut done = false;
        poll_fn(|cx| {
            if done {
                Poll::Ready(())
            } else {
                done = true;
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        })
        .await;
    }

    #[fuchsia::test]
    async fn test_open_root_dir() {
        let fixture = TestFixture::new().await;
        let root = fixture.root();
        let _: Vec<_> = root.query().await.expect("query failed");
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_create_dir_persists() {
        let mut device = DeviceHolder::new(FakeDevice::new(8192, 512));
        for i in 0..2 {
            let fixture = TestFixture::open(
                device,
                TestFixtureOptions { format: i == 0, ..Default::default() },
            )
            .await;
            let root = fixture.root();

            let flags = if i == 0 {
                fio::Flags::FLAG_MAYBE_CREATE | fio::PERM_READABLE
            } else {
                fio::PERM_READABLE
            };
            let dir = open_dir_checked(
                &root,
                "foo",
                flags | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
            .await;
            close_dir_checked(dir).await;

            device = fixture.close().await;
        }
    }

    #[fuchsia::test]
    async fn test_open_nonexistent_file() {
        let fixture = TestFixture::new().await;
        let root = fixture.root();

        assert_eq!(
            open_file(
                &root,
                "foo",
                fio::PERM_READABLE | fio::Flags::PROTOCOL_FILE,
                &Default::default()
            )
            .await
            .expect_err("Open succeeded")
            .root_cause()
            .downcast_ref::<zx::Status>()
            .expect("No status"),
            &zx::Status::NOT_FOUND,
        );

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_create_file() {
        let fixture = TestFixture::new().await;
        let root = fixture.root();

        let f = open_file_checked(
            &root,
            "foo",
            fio::Flags::FLAG_MAYBE_CREATE | fio::PERM_READABLE | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;
        close_file_checked(f).await;

        let f = open_file_checked(
            &root,
            "foo",
            fio::PERM_READABLE | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;
        close_file_checked(f).await;

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_create_dir_nested() {
        let fixture = TestFixture::new().await;
        let root = fixture.root();

        let d = open_dir_checked(
            &root,
            "foo",
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_DIRECTORY,
            Default::default(),
        )
        .await;
        close_dir_checked(d).await;

        let d = open_dir_checked(
            &root,
            "foo/bar",
            fio::Flags::FLAG_MAYBE_CREATE | fio::PERM_READABLE | fio::Flags::PROTOCOL_DIRECTORY,
            Default::default(),
        )
        .await;
        close_dir_checked(d).await;

        let d = open_dir_checked(
            &root,
            "foo/bar",
            fio::PERM_READABLE | fio::Flags::PROTOCOL_DIRECTORY,
            Default::default(),
        )
        .await;
        close_dir_checked(d).await;

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_strict_create_file_fails_if_present() {
        let fixture = TestFixture::new().await;
        let root = fixture.root();

        let f = open_file_checked(
            &root,
            "foo",
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::Flags::FLAG_MUST_CREATE
                | fio::PERM_READABLE
                | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;
        close_file_checked(f).await;

        assert_eq!(
            open_file(
                &root,
                "foo",
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::Flags::FLAG_MUST_CREATE
                    | fio::PERM_READABLE
                    | fio::Flags::PROTOCOL_FILE,
                &Default::default()
            )
            .await
            .expect_err("Open succeeded")
            .root_cause()
            .downcast_ref::<zx::Status>()
            .expect("No status"),
            &zx::Status::ALREADY_EXISTS,
        );

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_unlink_file_with_no_refs_immediately_freed() {
        let fixture = TestFixture::new().await;
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

        // Fill up the file with a lot of data, so we can verify that the extents are freed.
        let buf = vec![0xaa as u8; 512];
        loop {
            match file::write(&file, buf.as_slice()).await {
                Ok(_) => {}
                Err(e) => {
                    if let fuchsia_fs::file::WriteError::WriteError(status) = e {
                        if status == zx::Status::NO_SPACE {
                            break;
                        }
                    }
                    panic!("Unexpected write error {:?}", e);
                }
            }
        }

        close_file_checked(file).await;

        root.unlink("foo", &fio::UnlinkOptions::default())
            .await
            .expect("FIDL call failed")
            .expect("unlink failed");

        assert_eq!(
            open_file(
                &root,
                "foo",
                fio::PERM_READABLE | fio::Flags::PROTOCOL_FILE,
                &Default::default()
            )
            .await
            .expect_err("Open succeeded")
            .root_cause()
            .downcast_ref::<zx::Status>()
            .expect("No status"),
            &zx::Status::NOT_FOUND,
        );

        // Create another file so we can verify that the extents were actually freed.
        let file = open_file_checked(
            &root,
            "bar",
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;
        let buf = vec![0xaa as u8; 8192];
        file::write(&file, buf.as_slice()).await.expect("Failed to write new file");
        close_file_checked(file).await;

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_unlink_file() {
        let fixture = TestFixture::new().await;
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
        close_file_checked(file).await;

        root.unlink("foo", &fio::UnlinkOptions::default())
            .await
            .expect("FIDL call failed")
            .expect("unlink failed");

        assert_eq!(
            open_file(
                &root,
                "foo",
                fio::PERM_READABLE | fio::Flags::PROTOCOL_FILE,
                &Default::default()
            )
            .await
            .expect_err("Open succeeded")
            .root_cause()
            .downcast_ref::<zx::Status>()
            .expect("No status"),
            &zx::Status::NOT_FOUND,
        );

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_unlink_file_with_active_references() {
        let fixture = TestFixture::new().await;
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

        let buf = vec![0xaa as u8; 512];
        file::write(&file, buf.as_slice()).await.expect("write failed");

        root.unlink("foo", &fio::UnlinkOptions::default())
            .await
            .expect("FIDL call failed")
            .expect("unlink failed");

        // The child should immediately appear unlinked...
        assert_eq!(
            open_file(
                &root,
                "foo",
                fio::PERM_READABLE | fio::Flags::PROTOCOL_FILE,
                &Default::default()
            )
            .await
            .expect_err("Open succeeded")
            .root_cause()
            .downcast_ref::<zx::Status>()
            .expect("No status"),
            &zx::Status::NOT_FOUND,
        );

        // But its contents should still be readable from the other handle.
        file.seek(fio::SeekOrigin::Start, 0)
            .await
            .expect("seek failed")
            .map_err(zx::Status::err_from_raw)
            .expect("seek error");
        let rbuf = file::read(&file).await.expect("read failed");
        assert_eq!(rbuf, buf);
        close_file_checked(file).await;

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_unlink_dir_with_children_fails() {
        let fixture = TestFixture::new().await;
        let root = fixture.root();

        let dir = open_dir_checked(
            &root,
            "foo",
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_DIRECTORY,
            Default::default(),
        )
        .await;
        let f = open_file_checked(
            &dir,
            "bar",
            fio::Flags::FLAG_MAYBE_CREATE | fio::PERM_READABLE,
            &Default::default(),
        )
        .await;
        close_file_checked(f).await;

        assert_eq!(
            zx::Status::ok(
                root.unlink("foo", &fio::UnlinkOptions::default())
                    .await
                    .expect("FIDL call failed")
                    .expect_err("unlink succeeded")
            ),
            Err(zx::Status::NOT_EMPTY)
        );

        dir.unlink("bar", &fio::UnlinkOptions::default())
            .await
            .expect("FIDL call failed")
            .expect("unlink failed");
        root.unlink("foo", &fio::UnlinkOptions::default())
            .await
            .expect("FIDL call failed")
            .expect("unlink failed");

        close_dir_checked(dir).await;

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_unlink_dir_makes_directory_immutable() {
        let fixture = TestFixture::new().await;
        let root = fixture.root();

        let dir = open_dir_checked(
            &root,
            "foo",
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_DIRECTORY,
            Default::default(),
        )
        .await;

        root.unlink("foo", &fio::UnlinkOptions::default())
            .await
            .expect("FIDL call failed")
            .expect("unlink failed");

        assert_eq!(
            open_file(
                &dir,
                "bar",
                fio::PERM_READABLE | fio::Flags::FLAG_MAYBE_CREATE | fio::Flags::PROTOCOL_FILE,
                &Default::default()
            )
            .await
            .expect_err("Create file succeeded")
            .root_cause()
            .downcast_ref::<zx::Status>()
            .expect("No status"),
            &zx::Status::ACCESS_DENIED,
        );

        close_dir_checked(dir).await;

        fixture.close().await;
    }

    #[fuchsia::test(threads = 10)]
    async fn test_unlink_directory_with_children_race() {
        let fixture = TestFixture::new().await;
        let root = fixture.root();

        const PARENT: &str = "foo";
        const CHILD: &str = "bar";
        const GRANDCHILD: &str = "baz";
        open_dir_checked(
            &root,
            PARENT,
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_DIRECTORY,
            Default::default(),
        )
        .await;

        let open_parent = || async {
            open_dir_checked(
                &root,
                PARENT,
                fio::PERM_READABLE | fio::PERM_WRITABLE | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
            .await
        };
        let parent = open_parent().await;

        // Each iteration proceeds as follows:
        //  - Initialize a directory foo/bar/. (This might still be around from the previous
        //    iteration, which is fine.)
        //  - In one task, try to unlink foo/bar/.
        //  - In another task, try to add a file foo/bar/baz.
        for _ in 0..100 {
            let d = open_dir_checked(
                &parent,
                CHILD,
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE
                    | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
            .await;
            close_dir_checked(d).await;

            let parent = open_parent().await;
            let deleter = fasync::Task::spawn(async move {
                let wait_time = rand::random_range(0..5);
                fasync::Timer::new(Duration::from_millis(wait_time)).await;
                match parent
                    .unlink(CHILD, &fio::UnlinkOptions::default())
                    .await
                    .expect("FIDL call failed")
                    .map_err(zx::Status::err_from_raw)
                {
                    Ok(()) => {}
                    Err(zx::Status::NOT_EMPTY) => {}
                    Err(e) => panic!("Unexpected status from unlink: {:?}", e),
                };
                close_dir_checked(parent).await;
            });

            let parent = open_parent().await;
            let writer = fasync::Task::spawn(async move {
                let child_or = open_dir(
                    &parent,
                    CHILD,
                    fio::PERM_READABLE | fio::PERM_WRITABLE | fio::Flags::PROTOCOL_DIRECTORY,
                    &Default::default(),
                )
                .await;
                if let Err(e) = &child_or {
                    // The directory was already deleted.
                    assert_eq!(
                        e.root_cause().downcast_ref::<zx::Status>().expect("No status"),
                        &zx::Status::NOT_FOUND
                    );
                    close_dir_checked(parent).await;
                    return;
                }
                let child = child_or.unwrap();
                let _: Vec<_> = child.query().await.expect("query failed");
                match open_file(
                    &child,
                    GRANDCHILD,
                    fio::Flags::FLAG_MAYBE_CREATE | fio::PERM_READABLE | fio::Flags::PROTOCOL_FILE,
                    &Default::default(),
                )
                .await
                {
                    Ok(grandchild) => {
                        let _: Vec<_> = grandchild.query().await.expect("query failed");
                        close_file_checked(grandchild).await;
                        // We added the child before the directory was deleted; go ahead and
                        // clean up.
                        child
                            .unlink(GRANDCHILD, &fio::UnlinkOptions::default())
                            .await
                            .expect("FIDL call failed")
                            .expect("unlink failed");
                    }
                    Err(e) => {
                        // The directory started to be deleted before we created a child.
                        // Make sure we get the right error.
                        assert_eq!(
                            e.root_cause().downcast_ref::<zx::Status>().expect("No status"),
                            &zx::Status::ACCESS_DENIED,
                        );
                    }
                };
                close_dir_checked(child).await;
                close_dir_checked(parent).await;
            });
            writer.await;
            deleter.await;
        }

        close_dir_checked(parent).await;
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_readdir() {
        let fixture = TestFixture::new().await;
        let root = fixture.root();

        let open_dir = || {
            open_dir_checked(
                &root,
                "foo",
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE
                    | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
        };
        let parent = Arc::new(open_dir().await);

        let files = ["eenie", "meenie", "minie", "moe"];
        for file in &files {
            let file = open_file_checked(
                parent.as_ref(),
                file,
                fio::Flags::FLAG_MAYBE_CREATE | fio::Flags::PROTOCOL_FILE,
                &Default::default(),
            )
            .await;
            close_file_checked(file).await;
        }
        let dirs = ["fee", "fi", "fo", "fum"];
        for dir in &dirs {
            let dir = open_dir_checked(
                parent.as_ref(),
                dir,
                fio::Flags::FLAG_MAYBE_CREATE | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
            .await;
            close_dir_checked(dir).await;
        }
        {
            parent
                .create_symlink("symlink", b"target", None)
                .await
                .expect("FIDL call failed")
                .expect("create_symlink failed");
        }

        let readdir = |dir: Arc<fio::DirectoryProxy>| async move {
            let status = dir.rewind().await.expect("FIDL call failed");
            zx::Status::ok(status).expect("rewind failed");
            let (status, buf) = dir.read_dirents(fio::MAX_BUF).await.expect("FIDL call failed");
            zx::Status::ok(status).expect("read_dirents failed");
            let mut entries = vec![];
            for res in fuchsia_fs::directory::parse_dir_entries(&buf) {
                entries.push(res.expect("Failed to parse entry"));
            }
            entries
        };

        let mut expected_entries =
            vec![DirEntry { name: ".".to_owned(), kind: DirentKind::Directory }];
        expected_entries.extend(
            files.iter().map(|&name| DirEntry { name: name.to_owned(), kind: DirentKind::File }),
        );
        expected_entries.extend(
            dirs.iter()
                .map(|&name| DirEntry { name: name.to_owned(), kind: DirentKind::Directory }),
        );
        expected_entries.push(DirEntry { name: "symlink".to_owned(), kind: DirentKind::Symlink });
        expected_entries.sort_unstable();
        assert_eq!(expected_entries, readdir(Arc::clone(&parent)).await);

        // Remove an entry.
        parent
            .unlink(&expected_entries.pop().unwrap().name, &fio::UnlinkOptions::default())
            .await
            .expect("FIDL call failed")
            .expect("unlink failed");

        assert_eq!(expected_entries, readdir(Arc::clone(&parent)).await);

        close_dir_checked(Arc::try_unwrap(parent).unwrap()).await;
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_readdir_multiple_calls() {
        let fixture = TestFixture::new().await;
        let root = fixture.root();

        let parent = open_dir_checked(
            &root,
            "foo",
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_DIRECTORY,
            Default::default(),
        )
        .await;

        let files = ["a", "b"];
        for file in &files {
            let file = open_file_checked(
                &parent,
                file,
                fio::Flags::FLAG_MAYBE_CREATE | fio::Flags::PROTOCOL_FILE,
                &Default::default(),
            )
            .await;
            close_file_checked(file).await;
        }

        // TODO(https://fxbug.dev/42177353): Magic number; can we get this from fuchsia.io?
        const DIRENT_SIZE: u64 = 10; // inode: u64, size: u8, kind: u8
        const BUFFER_SIZE: u64 = DIRENT_SIZE + 2; // Enough space for a 2-byte name.

        let parse_entries = |buf| {
            let mut entries = vec![];
            for res in fuchsia_fs::directory::parse_dir_entries(buf) {
                entries.push(res.expect("Failed to parse entry"));
            }
            entries
        };

        let expected_entries = vec![
            DirEntry { name: ".".to_owned(), kind: DirentKind::Directory },
            DirEntry { name: "a".to_owned(), kind: DirentKind::File },
        ];
        let (status, buf) = parent.read_dirents(2 * BUFFER_SIZE).await.expect("FIDL call failed");
        zx::Status::ok(status).expect("read_dirents failed");
        assert_eq!(expected_entries, parse_entries(&buf));

        let expected_entries = vec![DirEntry { name: "b".to_owned(), kind: DirentKind::File }];
        let (status, buf) = parent.read_dirents(2 * BUFFER_SIZE).await.expect("FIDL call failed");
        zx::Status::ok(status).expect("read_dirents failed");
        assert_eq!(expected_entries, parse_entries(&buf));

        // Subsequent calls yield nothing.
        let expected_entries: Vec<DirEntry> = vec![];
        let (status, buf) = parent.read_dirents(2 * BUFFER_SIZE).await.expect("FIDL call failed");
        zx::Status::ok(status).expect("read_dirents failed");
        assert_eq!(expected_entries, parse_entries(&buf));

        close_dir_checked(parent).await;
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_set_large_extended_attribute_on_encrypted_directory() {
        let fixture = TestFixture::new().await;
        let crypt: Arc<CryptBase> = fixture.crypt().unwrap();
        let root = fixture.root();
        let open_dir = || {
            open_dir_checked(
                &root,
                "foo",
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE
                    | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
        };

        let parent: Arc<fio::DirectoryProxy> = Arc::new(open_dir().await);
        crypt
            .add_wrapping_key(WRAPPING_KEY_ID, [1; 32].into())
            .expect("Failed to add wrapping key");
        parent
            .update_attributes(&fio::MutableNodeAttributes {
                wrapping_key_id: Some(WRAPPING_KEY_ID),
                ..Default::default()
            })
            .await
            .expect("FIDL call failed")
            .map_err(zx::ok)
            .expect("update_attributes failed");
        let dir = open_dir_checked(
            parent.as_ref(),
            "fee",
            fio::Flags::FLAG_MAYBE_CREATE | fio::PERM_WRITABLE | fio::Flags::PROTOCOL_DIRECTORY,
            Default::default(),
        )
        .await;

        let xattr_name = b"xattr_name";
        let value_vec = vec![0x3; 300];

        dir.set_extended_attribute(
            xattr_name,
            fio::ExtendedAttributeValue::Bytes(value_vec.clone()),
            fio::SetExtendedAttributeMode::Set,
        )
        .await
        .expect("Failed to make FIDL call")
        .expect("Failed to set xattr with create");

        let subdir = open_dir_checked(
            &dir,
            "fo",
            fio::Flags::FLAG_MAYBE_CREATE | fio::Flags::PROTOCOL_DIRECTORY,
            Default::default(),
        )
        .await;
        close_dir_checked(dir).await;
        close_dir_checked(subdir).await;
        close_dir_checked(Arc::try_unwrap(parent).unwrap()).await;
        let device = fixture.close().await;
        let new_fixture = TestFixture::new_with_device(device).await;
        let root = new_fixture.root();
        let open_dir = || {
            open_dir_checked(
                &root,
                "foo",
                fio::PERM_READABLE | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
        };
        let parent: Arc<fio::DirectoryProxy> = Arc::new(open_dir().await);

        let readdir = |dir: Arc<fio::DirectoryProxy>| async move {
            let status = dir.rewind().await.expect("FIDL call failed");
            zx::Status::ok(status).expect("rewind failed");
            let (status, buf) = dir.read_dirents(fio::MAX_BUF).await.expect("FIDL call failed");
            zx::Status::ok(status).expect("read_dirents failed");
            let mut entries = vec![];
            for res in fuchsia_fs::directory::parse_dir_entries(&buf) {
                entries.push(res.expect("Failed to parse entry"));
            }
            entries
        };

        let encrypted_entries = readdir(Arc::clone(&parent)).await;
        let mut encrypted_name = String::new();
        for entry in encrypted_entries {
            if entry.name == ".".to_owned() {
                continue;
            } else {
                assert!(entry.name.len() >= FSCRYPT_PADDING);
                encrypted_name = entry.name;
                assert!(entry.kind == DirentKind::Directory)
            }
        }

        let encrypted_dir = Arc::new(
            open_dir_checked(
                parent.as_ref(),
                &encrypted_name,
                fio::Flags::PROTOCOL_DIRECTORY | fio::PERM_READABLE,
                Default::default(),
            )
            .await,
        );

        assert_eq!(
            encrypted_dir
                .get_extended_attribute(xattr_name)
                .await
                .expect("Failed to make FIDL call")
                .expect("Failed to get extended attribute"),
            fio::ExtendedAttributeValue::Bytes(value_vec)
        );

        let encrypted_subdir_entries = readdir(Arc::clone(&encrypted_dir)).await;
        for entry in encrypted_subdir_entries {
            if entry.name == ".".to_owned() {
                continue;
            } else {
                assert!(entry.name.len() >= FSCRYPT_PADDING);
                assert!(entry.kind == DirentKind::Directory)
            }
        }
        close_dir_checked(Arc::try_unwrap(encrypted_dir).unwrap()).await;
        close_dir_checked(Arc::try_unwrap(parent).unwrap()).await;
        new_fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_set_large_extended_attribute_on_encrypted_file() {
        let fixture = TestFixture::new().await;
        let crypt: Arc<CryptBase> = fixture.crypt().unwrap();
        let root = fixture.root();
        let open_dir = || {
            open_dir_checked(
                &root,
                "foo",
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE
                    | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
        };

        let parent: Arc<fio::DirectoryProxy> = Arc::new(open_dir().await);
        crypt
            .add_wrapping_key(WRAPPING_KEY_ID, [1; 32].into())
            .expect("Failed to add wrapping key");
        parent
            .update_attributes(&fio::MutableNodeAttributes {
                wrapping_key_id: Some(WRAPPING_KEY_ID),
                ..Default::default()
            })
            .await
            .expect("FIDL call failed")
            .map_err(zx::ok)
            .expect("update_attributes failed");
        let file = open_file_checked(
            parent.as_ref(),
            "fee",
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;

        let buf = vec![0xaa as u8; 512];
        file::write(&file, buf.as_slice()).await.expect("write failed");

        let xattr_name = b"xattr_name";
        let value_vec = vec![0x3; 300];

        file.set_extended_attribute(
            xattr_name,
            fio::ExtendedAttributeValue::Bytes(value_vec.clone()),
            fio::SetExtendedAttributeMode::Set,
        )
        .await
        .expect("Failed to make FIDL call")
        .expect("Failed to set xattr with create");

        close_file_checked(file).await;
        close_dir_checked(Arc::try_unwrap(parent).unwrap()).await;
        let device = fixture.close().await;
        let new_fixture = TestFixture::new_with_device(device).await;
        let crypt: Arc<CryptBase> = new_fixture.crypt().unwrap();
        let root = new_fixture.root();
        let open_dir = || {
            open_dir_checked(
                &root,
                "foo",
                fio::PERM_READABLE | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
        };
        let parent: Arc<fio::DirectoryProxy> = Arc::new(open_dir().await);

        let readdir = |dir: Arc<fio::DirectoryProxy>| async move {
            let status = dir.rewind().await.expect("FIDL call failed");
            zx::Status::ok(status).expect("rewind failed");
            let (status, buf) = dir.read_dirents(fio::MAX_BUF).await.expect("FIDL call failed");
            zx::Status::ok(status).expect("read_dirents failed");
            let mut entries = vec![];
            for res in fuchsia_fs::directory::parse_dir_entries(&buf) {
                entries.push(res.expect("Failed to parse entry"));
            }
            entries
        };

        let encrypted_entries = readdir(Arc::clone(&parent)).await;
        let mut encrypted_name = String::new();
        for entry in encrypted_entries {
            if entry.name == ".".to_owned() {
                continue;
            } else {
                assert!(entry.name.len() >= FSCRYPT_PADDING);
                encrypted_name = entry.name;
                assert!(entry.kind == DirentKind::File)
            }
        }

        let encrypted_file = Arc::new(
            open_file_checked(
                parent.as_ref(),
                &encrypted_name,
                fio::Flags::PROTOCOL_FILE | fio::PERM_READABLE,
                &Default::default(),
            )
            .await,
        );

        assert_eq!(
            encrypted_file
                .get_extended_attribute(xattr_name)
                .await
                .expect("Failed to make FIDL call")
                .expect("Failed to get extended attribute"),
            fio::ExtendedAttributeValue::Bytes(value_vec)
        );

        close_file_checked(Arc::try_unwrap(encrypted_file).unwrap()).await;

        crypt
            .add_wrapping_key(WRAPPING_KEY_ID, [1; 32].into())
            .expect("Failed to add wrapping key");

        let file = Arc::new(
            open_file_checked(
                parent.as_ref(),
                "fee",
                fio::Flags::PROTOCOL_FILE | fio::PERM_READABLE,
                &Default::default(),
            )
            .await,
        );

        let rbuf = file::read(&file).await.expect("read failed");
        assert_eq!(rbuf, buf);

        close_dir_checked(Arc::try_unwrap(parent).unwrap()).await;
        new_fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_encrypt_directory_in_unencrypted_volume() {
        let fixture = TestFixture::new_unencrypted().await;
        let root = fixture.root();
        let open_dir = || {
            open_dir_checked(
                &root,
                "foo",
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE
                    | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
        };

        let parent: Arc<fio::DirectoryProxy> = Arc::new(open_dir().await);
        let _ = parent
            .update_attributes(&fio::MutableNodeAttributes {
                wrapping_key_id: Some(WRAPPING_KEY_ID),
                ..Default::default()
            })
            .await
            .expect("FIDL call failed")
            .map_err(zx::ok)
            .expect_err("encrypting a dir in an unencrypted volume should fail");
        close_dir_checked(Arc::try_unwrap(parent).unwrap()).await;
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_encrypt_directory_with_large_extended_attribute() {
        let fixture = TestFixture::new().await;
        let crypt: Arc<CryptBase> = fixture.crypt().unwrap();
        let root = fixture.root();
        let open_dir = || {
            open_dir_checked(
                &root,
                "foo",
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE
                    | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
        };

        let parent: Arc<fio::DirectoryProxy> = Arc::new(open_dir().await);

        let xattr_name = b"xattr_name";
        let value_vec = vec![0x3; 300];
        parent
            .set_extended_attribute(
                xattr_name,
                fio::ExtendedAttributeValue::Bytes(value_vec.clone()),
                fio::SetExtendedAttributeMode::Set,
            )
            .await
            .expect("Failed to make FIDL call")
            .expect("Failed to set xattr with create");

        crypt
            .add_wrapping_key(WRAPPING_KEY_ID, [1; 32].into())
            .expect("Failed to add wrapping key");
        parent
            .update_attributes(&fio::MutableNodeAttributes {
                wrapping_key_id: Some(WRAPPING_KEY_ID),
                ..Default::default()
            })
            .await
            .expect("FIDL call failed")
            .map_err(zx::ok)
            .expect("update_attributes failed");
        let dir = open_dir_checked(
            parent.as_ref(),
            "fee",
            fio::Flags::FLAG_MAYBE_CREATE | fio::PERM_WRITABLE | fio::Flags::PROTOCOL_DIRECTORY,
            Default::default(),
        )
        .await;

        let subdir = open_dir_checked(
            &dir,
            "fo",
            fio::Flags::FLAG_MAYBE_CREATE | fio::Flags::PROTOCOL_DIRECTORY,
            Default::default(),
        )
        .await;
        close_dir_checked(dir).await;
        close_dir_checked(subdir).await;
        close_dir_checked(Arc::try_unwrap(parent).unwrap()).await;
        let device = fixture.close().await;
        let new_fixture = TestFixture::new_with_device(device).await;
        let root = new_fixture.root();
        let open_dir = || {
            open_dir_checked(
                &root,
                "foo",
                fio::PERM_READABLE | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
        };
        let parent: Arc<fio::DirectoryProxy> = Arc::new(open_dir().await);

        assert_eq!(
            parent
                .get_extended_attribute(xattr_name)
                .await
                .expect("Failed to make FIDL call")
                .expect("Failed to get extended attribute"),
            fio::ExtendedAttributeValue::Bytes(value_vec)
        );

        let readdir = |dir: Arc<fio::DirectoryProxy>| async move {
            let status = dir.rewind().await.expect("FIDL call failed");
            zx::Status::ok(status).expect("rewind failed");
            let (status, buf) = dir.read_dirents(fio::MAX_BUF).await.expect("FIDL call failed");
            zx::Status::ok(status).expect("read_dirents failed");
            let mut entries = vec![];
            for res in fuchsia_fs::directory::parse_dir_entries(&buf) {
                entries.push(res.expect("Failed to parse entry"));
            }
            entries
        };

        let encrypted_entries = readdir(Arc::clone(&parent)).await;
        let mut encrypted_name = None;
        for entry in encrypted_entries {
            if &entry.name == "." {
                continue;
            } else {
                assert!(entry.name.len() >= FSCRYPT_PADDING);
                assert!(encrypted_name.replace(entry.name).is_none());
                assert!(entry.kind == DirentKind::Directory)
            }
        }

        let encrypted_dir = Arc::new(
            open_dir_checked(
                parent.as_ref(),
                &encrypted_name.as_ref().unwrap(),
                fio::Flags::PROTOCOL_DIRECTORY | fio::Flags::PERM_ENUMERATE,
                Default::default(),
            )
            .await,
        );

        let encrypted_subdir_entries = readdir(Arc::clone(&encrypted_dir)).await;
        for entry in encrypted_subdir_entries {
            if &entry.name == "." {
                continue;
            } else {
                assert!(entry.name.len() >= FSCRYPT_PADDING);
                assert!(entry.kind == DirentKind::Directory)
            }
        }
        close_dir_checked(Arc::try_unwrap(encrypted_dir).unwrap()).await;
        close_dir_checked(Arc::try_unwrap(parent).unwrap()).await;
        new_fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_unlock_directory_during_readdir() {
        let fixture = TestFixture::new().await;
        let crypt: Arc<CryptBase> = fixture.crypt().unwrap();
        let root = fixture.root();
        let open_dir = || {
            open_dir_checked(
                &root,
                "foo",
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE
                    | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
        };

        let parent: Arc<fio::DirectoryProxy> = Arc::new(open_dir().await);
        crypt
            .add_wrapping_key(WRAPPING_KEY_ID, [1; 32].into())
            .expect("Failed to add wrapping key");
        parent
            .update_attributes(&fio::MutableNodeAttributes {
                wrapping_key_id: Some(WRAPPING_KEY_ID),
                ..Default::default()
            })
            .await
            .expect("FIDL call failed")
            .map_err(zx::ok)
            .expect("update_attributes failed");

        // Need enough entries such that multiple read_dirents calls are required to drain all the
        // entries.
        for i in 0..300 {
            let dir = open_dir_checked(
                parent.as_ref(),
                &format!("plaintext_{}", i),
                fio::Flags::FLAG_MAYBE_CREATE | fio::PERM_WRITABLE | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
            .await;
            close_dir_checked(dir).await;
        }

        close_dir_checked(Arc::try_unwrap(parent).unwrap()).await;
        let device = fixture.close().await;
        let new_fixture = TestFixture::new_with_device(device).await;
        let crypt: Arc<CryptBase> = new_fixture.crypt().unwrap();
        let root = new_fixture.root();
        let open_dir = || {
            open_dir_checked(
                &root,
                "foo",
                fio::PERM_READABLE | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
        };
        let parent: Arc<fio::DirectoryProxy> = Arc::new(open_dir().await);

        let readdir = |dir: Arc<fio::DirectoryProxy>| async move {
            let (status, buf) = dir.read_dirents(fio::MAX_BUF).await.expect("FIDL call failed");
            zx::Status::ok(status).expect("read_dirents failed");
            let mut entries = vec![];
            for res in fuchsia_fs::directory::parse_dir_entries(&buf) {
                entries.push(res.expect("Failed to parse entry"));
            }
            entries
        };

        let encrypted_entries = readdir(Arc::clone(&parent)).await;
        for entry in encrypted_entries {
            if entry.name == ".".to_owned() {
                continue;
            } else {
                assert!(entry.name.len() >= FSCRYPT_PADDING);
                assert!(!entry.name.starts_with("plaintext_"), "{entry:?} isn't encrypted!");
                assert!(entry.kind == DirentKind::Directory)
            }
        }
        crypt
            .add_wrapping_key(WRAPPING_KEY_ID, [1; 32].into())
            .expect("Failed to add wrapping key");
        let unencrypted_entries = readdir(Arc::clone(&parent)).await;
        for entry in unencrypted_entries {
            if entry.name == ".".to_owned() {
                continue;
            } else {
                assert!(entry.name.starts_with("plaintext_"), "{entry:?} is still encrypted!");
                assert!(entry.kind == DirentKind::Directory)
            }
        }

        close_dir_checked(Arc::try_unwrap(parent).unwrap()).await;
        new_fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_readdir_locked_directory() {
        let fixture = TestFixture::new().await;
        let crypt: Arc<CryptBase> = fixture.crypt().unwrap();
        let root = fixture.root();
        let open_dir = || {
            open_dir_checked(
                &root,
                "foo",
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE
                    | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
        };

        let parent: Arc<fio::DirectoryProxy> = Arc::new(open_dir().await);
        crypt
            .add_wrapping_key(WRAPPING_KEY_ID, [1; 32].into())
            .expect("Failed to add wrapping key");
        parent
            .update_attributes(&fio::MutableNodeAttributes {
                wrapping_key_id: Some(WRAPPING_KEY_ID),
                ..Default::default()
            })
            .await
            .expect("FIDL call failed")
            .map_err(zx::ok)
            .expect("update_attributes failed");
        let dir = open_dir_checked(
            parent.as_ref(),
            "fee",
            fio::Flags::FLAG_MAYBE_CREATE | fio::PERM_WRITABLE | fio::Flags::PROTOCOL_DIRECTORY,
            Default::default(),
        )
        .await;

        let subdir = open_dir_checked(
            &dir,
            "fo",
            fio::Flags::FLAG_MAYBE_CREATE | fio::Flags::PROTOCOL_DIRECTORY,
            Default::default(),
        )
        .await;
        close_dir_checked(dir).await;
        close_dir_checked(subdir).await;

        let readdir = |dir: Arc<fio::DirectoryProxy>| async move {
            let status = dir.rewind().await.expect("FIDL call failed");
            zx::Status::ok(status).expect("rewind failed");
            let (status, buf) = dir.read_dirents(fio::MAX_BUF).await.expect("FIDL call failed");
            zx::Status::ok(status).expect("read_dirents failed");
            let mut entries = vec![];
            for res in fuchsia_fs::directory::parse_dir_entries(&buf) {
                entries.push(res.expect("Failed to parse entry"));
            }
            entries
        };

        let mut expected_entries =
            vec![DirEntry { name: ".".to_owned(), kind: DirentKind::Directory }];

        expected_entries.push(DirEntry { name: "fee".to_owned(), kind: DirentKind::Directory });
        expected_entries.sort_unstable();
        assert_eq!(expected_entries, readdir(Arc::clone(&parent)).await);

        close_dir_checked(Arc::try_unwrap(parent).unwrap()).await;
        let device = fixture.close().await;
        let new_fixture = TestFixture::new_with_device(device).await;
        let root = new_fixture.root();
        let open_dir = || {
            open_dir_checked(
                &root,
                "foo",
                fio::PERM_READABLE | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
        };
        let parent: Arc<fio::DirectoryProxy> = Arc::new(open_dir().await);

        let encrypted_entries = readdir(Arc::clone(&parent)).await;
        let mut encrypted_name = String::new();
        for entry in encrypted_entries {
            if entry.name == ".".to_owned() {
                continue;
            } else {
                assert!(entry.name.len() >= FSCRYPT_PADDING);
                encrypted_name = entry.name;
                assert!(entry.kind == DirentKind::Directory)
            }
        }

        let encrypted_dir = Arc::new(
            open_dir_checked(
                parent.as_ref(),
                &encrypted_name,
                fio::Flags::PROTOCOL_DIRECTORY | fio::Flags::PERM_ENUMERATE,
                Default::default(),
            )
            .await,
        );

        let encrypted_subdir_entries = readdir(Arc::clone(&encrypted_dir)).await;
        for entry in encrypted_subdir_entries {
            if entry.name == ".".to_owned() {
                continue;
            } else {
                assert!(entry.name.len() >= FSCRYPT_PADDING);
                assert!(entry.kind == DirentKind::Directory)
            }
        }
        close_dir_checked(Arc::try_unwrap(encrypted_dir).unwrap()).await;
        close_dir_checked(Arc::try_unwrap(parent).unwrap()).await;
        new_fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_link_into_locked_directory_fails() {
        let fixture = TestFixture::new().await;
        let crypt: Arc<CryptBase> = fixture.crypt().unwrap();
        let root = fixture.root();
        let open_dir_1 = || {
            open_dir_checked(
                &root,
                "foo",
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE
                    | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
        };

        let parent_1: Arc<fio::DirectoryProxy> = Arc::new(open_dir_1().await);

        let open_dir_2 = || {
            open_dir_checked(
                &root,
                "foo_2",
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE
                    | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
        };
        let parent_2: Arc<fio::DirectoryProxy> = Arc::new(open_dir_2().await);

        crypt
            .add_wrapping_key(WRAPPING_KEY_ID, [1; 32].into())
            .expect("Failed to add wrapping key");
        parent_1
            .update_attributes(&fio::MutableNodeAttributes {
                wrapping_key_id: Some(WRAPPING_KEY_ID),
                ..Default::default()
            })
            .await
            .expect("FIDL call failed")
            .map_err(zx::ok)
            .expect("update_attributes failed");
        parent_2
            .update_attributes(&fio::MutableNodeAttributes {
                wrapping_key_id: Some(WRAPPING_KEY_ID),
                ..Default::default()
            })
            .await
            .expect("FIDL call failed")
            .map_err(zx::ok)
            .expect("update_attributes failed");
        let file = open_file_checked(
            parent_1.as_ref(),
            "fee",
            fio::Flags::FLAG_MAYBE_CREATE,
            &Default::default(),
        )
        .await;

        close_file_checked(file).await;
        close_dir_checked(Arc::try_unwrap(parent_1).unwrap()).await;
        close_dir_checked(Arc::try_unwrap(parent_2).unwrap()).await;

        let device = fixture.close().await;
        let new_fixture = TestFixture::new_with_device(device).await;
        let root = new_fixture.root();
        let open_dir_1 = || {
            open_dir_checked(
                &root,
                "foo",
                fio::PERM_READABLE | fio::PERM_WRITABLE | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
        };
        let parent_1: Arc<fio::DirectoryProxy> = Arc::new(open_dir_1().await);

        let open_dir_2 = || {
            open_dir_checked(
                &root,
                "foo_2",
                fio::PERM_READABLE | fio::PERM_WRITABLE | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
        };
        let parent_2: Arc<fio::DirectoryProxy> = Arc::new(open_dir_2().await);

        let readdir = |dir: Arc<fio::DirectoryProxy>| async move {
            let status = dir.rewind().await.expect("FIDL call failed");
            zx::Status::ok(status).expect("rewind failed");
            let (status, buf) = dir.read_dirents(fio::MAX_BUF).await.expect("FIDL call failed");
            zx::Status::ok(status).expect("read_dirents failed");
            let mut entries = vec![];
            for res in fuchsia_fs::directory::parse_dir_entries(&buf) {
                entries.push(res.expect("Failed to parse entry"));
            }
            entries
        };

        let encrypted_entries = readdir(Arc::clone(&parent_1)).await;
        let mut encrypted_name = String::new();
        for entry in encrypted_entries {
            if entry.name == ".".to_owned() {
                continue;
            } else {
                assert!(entry.name.len() >= FSCRYPT_PADDING);
                encrypted_name = entry.name;
                assert!(entry.kind == DirentKind::File)
            }
        }

        let (status, parent_2_token) = parent_2.get_token().await.expect("get token failed");
        zx::Status::ok(status).unwrap();

        assert_eq!(
            parent_1
                .link(&encrypted_name, parent_2_token.unwrap().into(), "file_2")
                .await
                .expect("FIDL transport error"),
            zx::Status::ACCESS_DENIED.into_raw()
        );

        close_dir_checked(Arc::try_unwrap(parent_1).unwrap()).await;
        close_dir_checked(Arc::try_unwrap(parent_2).unwrap()).await;
        new_fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_rename_in_locked_directory_fails() {
        let fixture = TestFixture::new().await;
        let crypt: Arc<CryptBase> = fixture.crypt().unwrap();
        let root = fixture.root();
        let open_dir_1 = || {
            open_dir_checked(
                &root,
                "foo",
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE
                    | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
        };

        let parent_1: Arc<fio::DirectoryProxy> = Arc::new(open_dir_1().await);

        let open_dir_2 = || {
            open_dir_checked(
                &root,
                "foo_2",
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE
                    | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
        };
        let parent_2: Arc<fio::DirectoryProxy> = Arc::new(open_dir_2().await);

        crypt
            .add_wrapping_key(WRAPPING_KEY_ID, [1; 32].into())
            .expect("Failed to add wrapping key");
        parent_1
            .update_attributes(&fio::MutableNodeAttributes {
                wrapping_key_id: Some(WRAPPING_KEY_ID),
                ..Default::default()
            })
            .await
            .expect("FIDL call failed")
            .map_err(zx::ok)
            .expect("update_attributes failed");
        parent_2
            .update_attributes(&fio::MutableNodeAttributes {
                wrapping_key_id: Some(WRAPPING_KEY_ID),
                ..Default::default()
            })
            .await
            .expect("FIDL call failed")
            .map_err(zx::ok)
            .expect("update_attributes failed");
        let file = open_file_checked(
            parent_1.as_ref(),
            "fee",
            fio::Flags::FLAG_MAYBE_CREATE,
            &Default::default(),
        )
        .await;

        close_file_checked(file).await;
        close_dir_checked(Arc::try_unwrap(parent_1).unwrap()).await;
        close_dir_checked(Arc::try_unwrap(parent_2).unwrap()).await;

        let device = fixture.close().await;
        let new_fixture = TestFixture::new_with_device(device).await;
        let root = new_fixture.root();
        let open_dir_1 = || {
            open_dir_checked(
                &root,
                "foo",
                fio::PERM_READABLE | fio::PERM_WRITABLE | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
        };
        let parent_1: Arc<fio::DirectoryProxy> = Arc::new(open_dir_1().await);

        let open_dir_2 = || {
            open_dir_checked(
                &root,
                "foo_2",
                fio::PERM_READABLE | fio::PERM_WRITABLE | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
        };
        let parent_2: Arc<fio::DirectoryProxy> = Arc::new(open_dir_2().await);

        let readdir = |dir: Arc<fio::DirectoryProxy>| async move {
            let status = dir.rewind().await.expect("FIDL call failed");
            zx::Status::ok(status).expect("rewind failed");
            let (status, buf) = dir.read_dirents(fio::MAX_BUF).await.expect("FIDL call failed");
            zx::Status::ok(status).expect("read_dirents failed");
            let mut entries = vec![];
            for res in fuchsia_fs::directory::parse_dir_entries(&buf) {
                entries.push(res.expect("Failed to parse entry"));
            }
            entries
        };

        let encrypted_entries = readdir(Arc::clone(&parent_1)).await;
        let mut encrypted_name = String::new();
        for entry in encrypted_entries {
            if entry.name == ".".to_owned() {
                continue;
            } else {
                assert!(entry.name.len() >= FSCRYPT_PADDING);
                encrypted_name = entry.name;
                assert!(entry.kind == DirentKind::File)
            }
        }

        let (status, parent_2_token) = parent_2.get_token().await.expect("get token failed");
        zx::Status::ok(status).unwrap();

        // Rename cross-directory when locked should fail.
        assert_eq!(
            parent_1
                .rename(&encrypted_name, parent_2_token.unwrap().into(), "file_2")
                .await
                .expect("FIDL transport error"),
            Err(zx::Status::ACCESS_DENIED.into_raw())
        );

        let (status, parent_1_token) = parent_1.get_token().await.expect("get token failed");
        zx::Status::ok(status).unwrap();

        // Rename same-directory when locked should fail.
        assert_eq!(
            parent_1
                .rename(&encrypted_name, parent_1_token.unwrap().into(), "file_2")
                .await
                .expect("FIDL transport error"),
            Err(zx::Status::ACCESS_DENIED.into_raw())
        );

        close_dir_checked(Arc::try_unwrap(parent_1).unwrap()).await;
        close_dir_checked(Arc::try_unwrap(parent_2).unwrap()).await;
        new_fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_link_encrypted_file_into_directory_encrypted_with_different_key_fails() {
        let fixture = TestFixture::new().await;
        let crypt: Arc<CryptBase> = fixture.crypt().unwrap();
        let root = fixture.root();
        let open_dir_1 = || {
            open_dir_checked(
                &root,
                "foo",
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE
                    | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
        };

        let parent_1: Arc<fio::DirectoryProxy> = Arc::new(open_dir_1().await);

        let open_dir_2 = || {
            open_dir_checked(
                &root,
                "foo_2",
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE
                    | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
        };
        let parent_2: Arc<fio::DirectoryProxy> = Arc::new(open_dir_2().await);

        crypt
            .add_wrapping_key(WRAPPING_KEY_ID, [1; 32].into())
            .expect("Failed to add wrapping key");

        const WRAPPING_KEY_ID_2: WrappingKeyId = u128::to_le_bytes(3);
        crypt
            .add_wrapping_key(WRAPPING_KEY_ID_2, [2; 32].into())
            .expect("Failed to add wrapping key");

        parent_1
            .update_attributes(&fio::MutableNodeAttributes {
                wrapping_key_id: Some(WRAPPING_KEY_ID),
                ..Default::default()
            })
            .await
            .expect("FIDL call failed")
            .map_err(zx::ok)
            .expect("update_attributes failed");
        parent_2
            .update_attributes(&fio::MutableNodeAttributes {
                wrapping_key_id: Some(WRAPPING_KEY_ID_2),
                ..Default::default()
            })
            .await
            .expect("FIDL call failed")
            .map_err(zx::ok)
            .expect("update_attributes failed");
        let file = open_file_checked(
            parent_1.as_ref(),
            "fee",
            fio::Flags::FLAG_MAYBE_CREATE,
            &Default::default(),
        )
        .await;

        close_file_checked(file).await;

        let (status, parent_2_token) = parent_2.get_token().await.expect("get token failed");
        zx::Status::ok(status).unwrap();

        assert_eq!(
            parent_1
                .link("fee", parent_2_token.unwrap().into(), "file_2")
                .await
                .expect("FIDL transport error"),
            zx::Status::BAD_STATE.into_raw()
        );
        close_dir_checked(Arc::try_unwrap(parent_1).unwrap()).await;
        close_dir_checked(Arc::try_unwrap(parent_2).unwrap()).await;
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_link_unencrypted_file_into_encrypted_directory_fails() {
        let fixture = TestFixture::new().await;
        let crypt: Arc<CryptBase> = fixture.crypt().unwrap();
        let root = fixture.root();
        let open_dir_1 = || {
            open_dir_checked(
                &root,
                "foo",
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE
                    | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
        };

        let parent_1: Arc<fio::DirectoryProxy> = Arc::new(open_dir_1().await);

        let open_dir_2 = || {
            open_dir_checked(
                &root,
                "foo_2",
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE
                    | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
        };
        let parent_2: Arc<fio::DirectoryProxy> = Arc::new(open_dir_2().await);

        crypt
            .add_wrapping_key(WRAPPING_KEY_ID, [1; 32].into())
            .expect("Failed to add wrapping key");

        parent_1
            .update_attributes(&fio::MutableNodeAttributes {
                wrapping_key_id: Some(WRAPPING_KEY_ID),
                ..Default::default()
            })
            .await
            .expect("FIDL call failed")
            .map_err(zx::ok)
            .expect("update_attributes failed");

        let file = open_file_checked(
            parent_2.as_ref(),
            "fee",
            fio::Flags::FLAG_MAYBE_CREATE | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;

        close_file_checked(file).await;

        let (status, parent_1_token) = parent_1.get_token().await.expect("get token failed");
        zx::Status::ok(status).unwrap();

        assert_eq!(
            parent_2
                .link("fee", parent_1_token.unwrap().into(), "file")
                .await
                .expect("FIDL transport error"),
            zx::Status::BAD_STATE.into_raw()
        );
        close_dir_checked(Arc::try_unwrap(parent_1).unwrap()).await;
        close_dir_checked(Arc::try_unwrap(parent_2).unwrap()).await;
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_link_locked_directory_into_unencrypted_dir() {
        let fixture = TestFixture::new().await;
        let crypt: Arc<CryptBase> = fixture.crypt().unwrap();
        let root = fixture.root();
        let open_dir_1 = || {
            open_dir_checked(
                &root,
                "foo",
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE
                    | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
        };

        let parent_1: Arc<fio::DirectoryProxy> = Arc::new(open_dir_1().await);

        let open_dir_2 = || {
            open_dir_checked(
                &root,
                "foo_2",
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE
                    | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
        };
        let parent_2: Arc<fio::DirectoryProxy> = Arc::new(open_dir_2().await);

        crypt
            .add_wrapping_key(WRAPPING_KEY_ID, [1; 32].into())
            .expect("Failed to add wrapping key");
        parent_1
            .update_attributes(&fio::MutableNodeAttributes {
                wrapping_key_id: Some(WRAPPING_KEY_ID),
                ..Default::default()
            })
            .await
            .expect("FIDL call failed")
            .map_err(zx::ok)
            .expect("update_attributes failed");
        let file = open_file_checked(
            parent_1.as_ref(),
            "fee",
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;
        let _ = file
            .write(&[8; 8192])
            .await
            .expect("FIDL call failed")
            .map_err(zx::Status::err_from_raw)
            .expect("write failed");

        close_file_checked(file).await;
        close_dir_checked(Arc::try_unwrap(parent_1).unwrap()).await;
        close_dir_checked(Arc::try_unwrap(parent_2).unwrap()).await;

        let device = fixture.close().await;
        let new_fixture = TestFixture::new_with_device(device).await;
        let root = new_fixture.root();
        let open_dir_1 = || {
            open_dir_checked(
                &root,
                "foo",
                fio::PERM_READABLE | fio::PERM_WRITABLE | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
        };
        let parent_1: Arc<fio::DirectoryProxy> = Arc::new(open_dir_1().await);

        let open_dir_2 = || {
            open_dir_checked(
                &root,
                "foo_2",
                fio::PERM_READABLE | fio::PERM_WRITABLE | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
        };
        let parent_2: Arc<fio::DirectoryProxy> = Arc::new(open_dir_2().await);

        let readdir = |dir: Arc<fio::DirectoryProxy>| async move {
            let status = dir.rewind().await.expect("FIDL call failed");
            zx::Status::ok(status).expect("rewind failed");
            let (status, buf) = dir.read_dirents(fio::MAX_BUF).await.expect("FIDL call failed");
            zx::Status::ok(status).expect("read_dirents failed");
            let mut entries = vec![];
            for res in fuchsia_fs::directory::parse_dir_entries(&buf) {
                entries.push(res.expect("Failed to parse entry"));
            }
            entries
        };

        let encrypted_entries = readdir(Arc::clone(&parent_1)).await;
        let mut encrypted_name = String::new();
        for entry in encrypted_entries {
            if entry.name == ".".to_owned() {
                continue;
            } else {
                assert!(entry.name.len() >= FSCRYPT_PADDING);
                encrypted_name = entry.name;
                assert!(entry.kind == DirentKind::File)
            }
        }

        let (status, parent_2_token) = parent_2.get_token().await.expect("get token failed");
        zx::Status::ok(status).unwrap();

        assert_eq!(
            parent_1
                .link(&encrypted_name, parent_2_token.unwrap().into(), "file_2")
                .await
                .expect("FIDL transport error"),
            zx::sys::ZX_OK
        );

        let file =
            open_file_checked(parent_2.as_ref(), "file_2", fio::PERM_READABLE, &Default::default())
                .await;
        let (mutable_attributes, _immutable_attributes) = file
            .get_attributes(
                fio::NodeAttributesQuery::CONTENT_SIZE
                    | fio::NodeAttributesQuery::STORAGE_SIZE
                    | fio::NodeAttributesQuery::LINK_COUNT
                    | fio::NodeAttributesQuery::MODIFICATION_TIME
                    | fio::NodeAttributesQuery::CHANGE_TIME
                    | fio::NodeAttributesQuery::WRAPPING_KEY_ID,
            )
            .await
            .expect("FIDL call failed")
            .map_err(zx::Status::err_from_raw)
            .expect("get_attributes failed");
        assert_eq!(mutable_attributes.wrapping_key_id, Some(WRAPPING_KEY_ID));
        assert_eq!(
            file.read(fio::MAX_BUF)
                .await
                .expect("FIDL call failed")
                .expect_err("reading an encrypted file should fail"),
            zx::Status::BAD_STATE.into_raw()
        );

        close_dir_checked(Arc::try_unwrap(parent_1).unwrap()).await;
        close_dir_checked(Arc::try_unwrap(parent_2).unwrap()).await;
        new_fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_encrypted_filename_does_not_have_slashes() {
        let fixture = TestFixture::new().await;
        let crypt: Arc<CryptBase> = fixture.crypt().unwrap();
        let root = fixture.root();
        let open_dir = || {
            open_dir_checked(
                &root,
                "foo",
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE
                    | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
        };

        let parent: Arc<fio::DirectoryProxy> = Arc::new(open_dir().await);
        crypt
            .add_wrapping_key(WRAPPING_KEY_ID, [1; 32].into())
            .expect("Failed to add wrapping key");
        parent
            .update_attributes(&fio::MutableNodeAttributes {
                wrapping_key_id: Some(WRAPPING_KEY_ID),
                ..Default::default()
            })
            .await
            .expect("FIDL call failed")
            .map_err(zx::ok)
            .expect("update_attributes failed");
        const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
        for _ in 0..100 {
            let one_char = || CHARSET[rand::random_range(0..CHARSET.len())] as char;
            let filename: String = std::iter::repeat_with(one_char).take(100).collect();
            let dir = open_dir_checked(
                parent.as_ref(),
                &filename,
                fio::Flags::FLAG_MAYBE_CREATE | fio::PERM_WRITABLE | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
            .await;
            close_dir_checked(dir).await;
        }

        close_dir_checked(Arc::try_unwrap(parent).unwrap()).await;
        let readdir = |dir: Arc<fio::DirectoryProxy>| async move {
            let status = dir.rewind().await.expect("FIDL call failed");
            zx::Status::ok(status).expect("rewind failed");
            let (status, buf) = dir.read_dirents(fio::MAX_BUF).await.expect("FIDL call failed");
            zx::Status::ok(status).expect("read_dirents failed");
            let mut entries = vec![];
            for res in fuchsia_fs::directory::parse_dir_entries(&buf) {
                entries.push(res.expect("Failed to parse entry"));
            }
            entries
        };

        let device = fixture.close().await;
        let new_fixture = TestFixture::new_with_device(device).await;
        let root = new_fixture.root();
        let open_dir = || {
            open_dir_checked(
                &root,
                "foo",
                fio::PERM_READABLE | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
        };
        let parent: Arc<fio::DirectoryProxy> = Arc::new(open_dir().await);

        let encrypted_entries = readdir(Arc::clone(&parent)).await;
        for entry in encrypted_entries {
            if entry.name == ".".to_owned() {
                continue;
            } else {
                assert!(entry.name.len() >= FSCRYPT_PADDING);
                assert!(!entry.name.contains("/"));
                assert!(entry.kind == DirentKind::Directory)
            }
        }

        close_dir_checked(Arc::try_unwrap(parent).unwrap()).await;
        new_fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_stat_locked_file() {
        let fixture = TestFixture::new().await;
        let crypt: Arc<CryptBase> = fixture.crypt().unwrap();
        let root = fixture.root();
        let open_dir = || {
            open_dir_checked(
                &root,
                "foo",
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE
                    | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
        };
        let parent = Arc::new(open_dir().await);
        crypt
            .add_wrapping_key(WRAPPING_KEY_ID, [1; 32].into())
            .expect("Failed to add wrapping key");
        parent
            .update_attributes(&fio::MutableNodeAttributes {
                wrapping_key_id: Some(WRAPPING_KEY_ID),
                ..Default::default()
            })
            .await
            .expect("FIDL call failed")
            .map_err(zx::ok)
            .expect("update_attributes failed");

        let file = open_file_checked(
            parent.as_ref(),
            "file",
            fio::Flags::FLAG_MAYBE_CREATE | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;

        close_file_checked(file).await;
        close_dir_checked(Arc::try_unwrap(parent).unwrap()).await;

        let device = fixture.close().await;
        let new_fixture = TestFixture::new_with_device(device).await;
        let root = new_fixture.root();
        let open_dir = || {
            open_dir_checked(
                &root,
                "foo",
                fio::PERM_READABLE | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
        };
        let parent: Arc<fio::DirectoryProxy> = Arc::new(open_dir().await);
        let (status, buf) = parent.read_dirents(fio::MAX_BUF).await.expect("FIDL call failed");
        zx::Status::ok(status).expect("read_dirents failed");
        let mut encrypted_entries = vec![];
        for res in fuchsia_fs::directory::parse_dir_entries(&buf) {
            encrypted_entries.push(res.expect("Failed to parse entry"));
        }
        let mut encrypted_name = String::new();
        for entry in encrypted_entries {
            if entry.name == ".".to_owned() {
                continue;
            } else {
                assert!(entry.name.len() >= FSCRYPT_PADDING);
                encrypted_name = entry.name;
                assert!(entry.kind == DirentKind::File)
            }
        }

        let file = open_file_checked(
            parent.as_ref(),
            &encrypted_name,
            fio::Flags::PROTOCOL_FILE | fio::Flags::PERM_GET_ATTRIBUTES,
            &Default::default(),
        )
        .await;
        let (_mutable_attributes, _immutable_attributes) = file
            .get_attributes(
                fio::NodeAttributesQuery::CONTENT_SIZE
                    | fio::NodeAttributesQuery::STORAGE_SIZE
                    | fio::NodeAttributesQuery::LINK_COUNT
                    | fio::NodeAttributesQuery::MODIFICATION_TIME
                    | fio::NodeAttributesQuery::CHANGE_TIME,
            )
            .await
            .expect("FIDL call failed")
            .map_err(zx::Status::err_from_raw)
            .expect("get_attributes failed");
        close_file_checked(file).await;
        new_fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_unlink_locked_directory() {
        let fixture = TestFixture::new().await;
        let crypt: Arc<CryptBase> = fixture.crypt().unwrap();
        let root = fixture.root();
        let open_dir = || {
            open_dir_checked(
                &root,
                "foo",
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE
                    | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
        };

        let parent: Arc<fio::DirectoryProxy> = Arc::new(open_dir().await);
        crypt
            .add_wrapping_key(WRAPPING_KEY_ID, [1; 32].into())
            .expect("Failed to add wrapping key");
        parent
            .update_attributes(&fio::MutableNodeAttributes {
                wrapping_key_id: Some(WRAPPING_KEY_ID),
                ..Default::default()
            })
            .await
            .expect("FIDL call failed")
            .map_err(zx::ok)
            .expect("update_attributes failed");
        let dir = open_dir_checked(
            parent.as_ref(),
            "fee",
            fio::Flags::FLAG_MAYBE_CREATE | fio::PERM_WRITABLE | fio::Flags::PROTOCOL_DIRECTORY,
            Default::default(),
        )
        .await;

        close_dir_checked(dir).await;
        close_dir_checked(Arc::try_unwrap(parent).unwrap()).await;
        let device = fixture.close().await;
        let new_fixture = TestFixture::new_with_device(device).await;
        let root = new_fixture.root();
        let open_dir = || {
            open_dir_checked(
                &root,
                "foo",
                fio::PERM_READABLE | fio::PERM_WRITABLE | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
        };
        let parent: Arc<fio::DirectoryProxy> = Arc::new(open_dir().await);

        let readdir = |dir: Arc<fio::DirectoryProxy>| async move {
            let status = dir.rewind().await.expect("FIDL call failed");
            zx::Status::ok(status).expect("rewind failed");
            let (status, buf) = dir.read_dirents(fio::MAX_BUF).await.expect("FIDL call failed");
            zx::Status::ok(status).expect("read_dirents failed");
            let mut entries = vec![];
            for res in fuchsia_fs::directory::parse_dir_entries(&buf) {
                entries.push(res.expect("Failed to parse entry"));
            }
            entries
        };

        let encrypted_entries = readdir(Arc::clone(&parent)).await;
        let mut encrypted_name = String::new();
        for entry in encrypted_entries {
            if entry.name == ".".to_owned() {
                continue;
            } else {
                assert!(entry.name.len() >= FSCRYPT_PADDING);
                encrypted_name = entry.name;
                assert!(entry.kind == DirentKind::Directory)
            }
        }

        parent
            .unlink(&encrypted_name, &fio::UnlinkOptions::default())
            .await
            .expect("FIDL call failed")
            .expect("unlink failed");

        let encrypted_entries = readdir(Arc::clone(&parent)).await;
        let mut count = 0;
        for entry in encrypted_entries {
            if entry.name == ".".to_owned() {
                continue;
            } else {
                assert!(entry.name.len() >= FSCRYPT_PADDING);
                assert!(entry.kind == DirentKind::Directory)
            }
            count += 1;
        }
        assert_eq!(count, 0);
        close_dir_checked(Arc::try_unwrap(parent).unwrap()).await;
        new_fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_rename_within_locked_encrypted_directory() {
        let fixture = TestFixture::new().await;
        let crypt: Arc<CryptBase> = fixture.crypt().unwrap();
        let root = fixture.root();
        let open_dir = || {
            open_dir_checked(
                &root,
                "foo",
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE
                    | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
        };

        let parent: Arc<fio::DirectoryProxy> = Arc::new(open_dir().await);
        crypt
            .add_wrapping_key(WRAPPING_KEY_ID, [1; 32].into())
            .expect("Failed to add wrapping key");
        parent
            .update_attributes(&fio::MutableNodeAttributes {
                wrapping_key_id: Some(WRAPPING_KEY_ID),
                ..Default::default()
            })
            .await
            .expect("FIDL call failed")
            .map_err(zx::ok)
            .expect("update_attributes failed");
        let dir = open_dir_checked(
            parent.as_ref(),
            "fee",
            fio::Flags::FLAG_MAYBE_CREATE | fio::PERM_WRITABLE | fio::Flags::PROTOCOL_DIRECTORY,
            Default::default(),
        )
        .await;

        close_dir_checked(dir).await;
        close_dir_checked(Arc::try_unwrap(parent).unwrap()).await;
        let device = fixture.close().await;
        let new_fixture = TestFixture::new_with_device(device).await;
        let crypt: Arc<CryptBase> = new_fixture.crypt().unwrap();
        let root = new_fixture.root();
        let open_dir = || {
            open_dir_checked(
                &root,
                "foo",
                fio::PERM_READABLE | fio::PERM_WRITABLE | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
        };
        let parent: Arc<fio::DirectoryProxy> = Arc::new(open_dir().await);

        let readdir = |dir: Arc<fio::DirectoryProxy>| async move {
            let status = dir.rewind().await.expect("FIDL call failed");
            zx::Status::ok(status).expect("rewind failed");
            let (status, buf) = dir.read_dirents(fio::MAX_BUF).await.expect("FIDL call failed");
            zx::Status::ok(status).expect("read_dirents failed");
            let mut entries = vec![];
            for res in fuchsia_fs::directory::parse_dir_entries(&buf) {
                entries.push(res.expect("Failed to parse entry"));
            }
            entries
        };

        let encrypted_entries = readdir(Arc::clone(&parent)).await;
        let mut encrypted_name = String::new();
        for entry in encrypted_entries {
            if entry.name == ".".to_owned() {
                continue;
            } else {
                assert!(entry.name.len() >= FSCRYPT_PADDING);
                encrypted_name = entry.name;
                assert!(entry.kind == DirentKind::Directory)
            }
        }

        let (status, dst_token) = parent.get_token().await.expect("FIDL call failed");
        zx::Status::ok(status).expect("get_token failed");
        let new_encrypted_name = "aabbcc";
        parent
            .rename(&encrypted_name, zx::Event::from(dst_token.unwrap()), new_encrypted_name)
            .await
            .expect("FIDL call failed")
            .expect_err("rename should fail on a locked directory");
        let (status, dst_token) = parent.get_token().await.expect("FIDL call failed");
        zx::Status::ok(status).expect("get_token failed");
        crypt
            .add_wrapping_key(WRAPPING_KEY_ID, [1; 32].into())
            .expect("Failed to add wrapping key");
        parent
            .rename("fee", zx::Event::from(dst_token.unwrap()), "new_fee")
            .await
            .expect("FIDL call failed")
            .expect("rename should fail on a locked directory");

        let _dir = open_dir_checked(
            parent.as_ref(),
            "new_fee",
            fio::Flags::PROTOCOL_DIRECTORY,
            Default::default(),
        )
        .await;
        close_dir_checked(Arc::try_unwrap(parent).unwrap()).await;
        new_fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_link_symlink_into_encrypted_directory() {
        let fixture = TestFixture::new().await;
        let crypt: Arc<CryptBase> = fixture.crypt().unwrap();
        let root = fixture.root();
        let open_dir = || {
            open_dir_checked(
                &root,
                "foo",
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE
                    | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
        };
        let parent = Arc::new(open_dir().await);
        crypt
            .add_wrapping_key(WRAPPING_KEY_ID, [1; 32].into())
            .expect("Failed to add wrapping key");
        parent
            .update_attributes(&fio::MutableNodeAttributes {
                wrapping_key_id: Some(WRAPPING_KEY_ID),
                ..Default::default()
            })
            .await
            .expect("FIDL call failed")
            .map_err(zx::ok)
            .expect("update_attributes failed");

        {
            parent
                .create_symlink("symlink", b"target", None)
                .await
                .expect("FIDL call failed")
                .expect("create_symlink failed");

            async fn open_symlink(root: &fio::DirectoryProxy, path: &str) -> fio::SymlinkProxy {
                let (proxy, server_end) = create_proxy::<fio::SymlinkMarker>();
                root.open(
                    path,
                    fio::PERM_READABLE | fio::PERM_WRITABLE | fio::Flags::FLAG_SEND_REPRESENTATION,
                    &Default::default(),
                    server_end.into_channel(),
                )
                .expect("open failed");

                let representation = proxy
                    .take_event_stream()
                    .next()
                    .await
                    .expect("missing Symlink event")
                    .expect("failed to read Symlink event")
                    .into_on_representation()
                    .expect("failed to decode OnRepresentation");

                assert_matches!(representation,
                    fio::Representation::Symlink(fio::SymlinkInfo{
                        target: Some(target), ..
                    }) if target == b"target"
                );

                proxy
            }

            let proxy = open_symlink(&parent, "symlink").await;

            let (status, dst_token) = parent.get_token().await.expect("FIDL call failed");
            zx::Status::ok(status).expect("get_token failed");
            proxy
                .link_into(zx::Event::from(dst_token.unwrap()), "symlink2")
                .await
                .expect("link_into (FIDL) failed")
                .expect("link_into failed");

            open_symlink(&parent, "symlink2").await;
        }

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_link_symlink_into_locked_directory_fails() {
        let fixture = TestFixture::new().await;
        let crypt: Arc<CryptBase> = fixture.crypt().unwrap();
        let root = fixture.root();
        let open_dir = || {
            open_dir_checked(
                &root,
                "foo",
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE
                    | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
        };
        let parent = Arc::new(open_dir().await);
        crypt
            .add_wrapping_key(WRAPPING_KEY_ID, [1; 32].into())
            .expect("Failed to add wrapping key");
        parent
            .update_attributes(&fio::MutableNodeAttributes {
                wrapping_key_id: Some(WRAPPING_KEY_ID),
                ..Default::default()
            })
            .await
            .expect("FIDL call failed")
            .map_err(zx::ok)
            .expect("update_attributes failed");
        close_dir_checked(Arc::try_unwrap(parent).unwrap()).await;

        let device = fixture.close().await;
        let new_fixture = TestFixture::new_with_device(device).await;
        let root = new_fixture.root();
        let open_dir = || {
            open_dir_checked(
                &root,
                "foo",
                fio::PERM_READABLE | fio::PERM_WRITABLE | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
        };
        let parent: Arc<fio::DirectoryProxy> = Arc::new(open_dir().await);
        {
            root.create_symlink("symlink", b"target", None)
                .await
                .expect("FIDL call failed")
                .expect("create_symlink failed");

            async fn open_symlink(root: &fio::DirectoryProxy, path: &str) -> fio::SymlinkProxy {
                let (proxy, server_end) = create_proxy::<fio::SymlinkMarker>();
                root.open(
                    path,
                    fio::PERM_READABLE | fio::PERM_WRITABLE | fio::Flags::FLAG_SEND_REPRESENTATION,
                    &Default::default(),
                    server_end.into_channel(),
                )
                .expect("open failed");

                let representation = proxy
                    .take_event_stream()
                    .next()
                    .await
                    .expect("missing Symlink event")
                    .expect("failed to read Symlink event")
                    .into_on_representation()
                    .expect("failed to decode OnRepresentation");

                assert_matches!(representation,
                    fio::Representation::Symlink(fio::SymlinkInfo{
                        target: Some(target), ..
                    }) if target == b"target"
                );

                proxy
            }

            let proxy = open_symlink(&root, "symlink").await;

            let (status, dst_token) = parent.get_token().await.expect("FIDL call failed");
            zx::Status::ok(status).expect("get_token failed");
            proxy
                .link_into(zx::Event::from(dst_token.unwrap()), "symlink2")
                .await
                .expect("link_into (FIDL) failed")
                .expect_err("linking into a locked directory should fail");
        }

        new_fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_stat_locked_symlink() {
        let fixture = TestFixture::new().await;
        let crypt: Arc<CryptBase> = fixture.crypt().unwrap();
        let root = fixture.root();
        let open_dir = || {
            open_dir_checked(
                &root,
                "foo",
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE
                    | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
        };
        let parent = Arc::new(open_dir().await);
        crypt
            .add_wrapping_key(WRAPPING_KEY_ID, [1; 32].into())
            .expect("Failed to add wrapping key");
        parent
            .update_attributes(&fio::MutableNodeAttributes {
                wrapping_key_id: Some(WRAPPING_KEY_ID),
                ..Default::default()
            })
            .await
            .expect("FIDL call failed")
            .map_err(zx::ok)
            .expect("update_attributes failed");

        // This is where we create the symlink
        parent
            .create_symlink("symlink", b"target", None)
            .await
            .expect("FIDL call failed")
            .expect("create_symlink failed");

        close_dir_checked(Arc::try_unwrap(parent).unwrap()).await;

        let device = fixture.close().await;
        let new_fixture = TestFixture::new_with_device(device).await;
        let root = new_fixture.root();
        let open_dir = || {
            open_dir_checked(
                &root,
                "foo",
                fio::PERM_READABLE | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
        };
        let parent: Arc<fio::DirectoryProxy> = Arc::new(open_dir().await);
        let (status, buf) = parent.read_dirents(fio::MAX_BUF).await.expect("FIDL call failed");
        zx::Status::ok(status).expect("read_dirents failed");
        let mut encrypted_entries = vec![];
        for res in fuchsia_fs::directory::parse_dir_entries(&buf) {
            encrypted_entries.push(res.expect("Failed to parse entry"));
        }
        let mut encrypted_name = String::new();
        for entry in encrypted_entries {
            if entry.name == ".".to_owned() {
                continue;
            } else {
                assert!(entry.name.len() >= FSCRYPT_PADDING);
                encrypted_name = entry.name;
                assert!(entry.kind == DirentKind::Symlink)
            }
        }
        {
            let (symlink, server_end) = create_proxy::<fio::SymlinkMarker>();
            parent
                .open(
                    &encrypted_name,
                    fio::PERM_READABLE | fio::Flags::FLAG_SEND_REPRESENTATION,
                    &Default::default(),
                    server_end.into_channel(),
                )
                .expect("open failed");

            let representation = symlink
                .take_event_stream()
                .next()
                .await
                .expect("missing Symlink event")
                .expect("failed to read Symlink event")
                .into_on_representation()
                .expect("failed to decode OnRepresentation");
            let mut encrypted_target = None;
            if let fio::Representation::Symlink(fio::SymlinkInfo { target: Some(target), .. }) =
                representation
            {
                encrypted_target = Some(target)
            };

            let (_mutable, immutable) = symlink
                .get_attributes(fio::NodeAttributesQuery::CONTENT_SIZE)
                .await
                .expect("transport error on get_attributes")
                .expect("failed to get attributes on a locked symlink");

            assert_eq!(immutable.content_size, encrypted_target.map(|x| x.len() as u64));
        }

        new_fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_create_symlink_in_locked_directory() {
        let fixture = TestFixture::new().await;
        let crypt: Arc<CryptBase> = fixture.crypt().unwrap();
        let root = fixture.root();
        let open_dir = || {
            open_dir_checked(
                &root,
                "foo",
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE
                    | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
        };
        let parent = Arc::new(open_dir().await);
        crypt
            .add_wrapping_key(WRAPPING_KEY_ID, [1; 32].into())
            .expect("Failed to add wrapping key");
        parent
            .update_attributes(&fio::MutableNodeAttributes {
                wrapping_key_id: Some(WRAPPING_KEY_ID),
                ..Default::default()
            })
            .await
            .expect("FIDL call failed")
            .map_err(zx::ok)
            .expect("update_attributes failed");

        close_dir_checked(Arc::try_unwrap(parent).unwrap()).await;

        let device = fixture.close().await;
        let new_fixture = TestFixture::new_with_device(device).await;
        let root = new_fixture.root();
        let open_dir = || {
            open_dir_checked(
                &root,
                "foo",
                fio::PERM_READABLE | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
        };
        let parent: Arc<fio::DirectoryProxy> = Arc::new(open_dir().await);
        parent
            .create_symlink("symlink", b"target", None)
            .await
            .expect("FIDL call failed")
            .expect_err("creating a symlink in a locked directory should fail");

        new_fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_symlink() {
        let fixture = TestFixture::new().await;

        {
            let root = fixture.root();

            root.create_symlink("symlink", b"target", None)
                .await
                .expect("FIDL call failed")
                .expect("create_symlink failed");

            let (proxy, server_end) = create_proxy::<fio::SymlinkMarker>();
            root.open(
                "symlink",
                fio::PERM_READABLE | fio::Flags::FLAG_SEND_REPRESENTATION,
                &Default::default(),
                server_end.into_channel(),
            )
            .expect("open failed");

            let representation = proxy
                .take_event_stream()
                .next()
                .await
                .expect("missing Symlink event")
                .expect("failed to read Symlink event")
                .into_on_representation()
                .expect("failed to decode OnRepresentation");

            assert_matches!(representation,
                fio::Representation::Symlink(fio::SymlinkInfo{
                    target: Some(target), ..
                }) if target == b"target"
            );

            let (proxy, server_end) = create_proxy::<fio::SymlinkMarker>();
            root.create_symlink("symlink2", b"target2", Some(server_end))
                .await
                .expect("FIDL call failed")
                .expect("create_symlink failed");

            let node_info = proxy.describe().await.expect("FIDL call failed");
            assert_matches!(
                node_info,
                fio::SymlinkInfo { target: Some(target), .. } if target == b"target2"
            );

            // Unlink the second symlink.
            root.unlink("symlink2", &fio::UnlinkOptions::default())
                .await
                .expect("FIDL call failed")
                .expect("unlink failed");

            // Rename over the first symlink.
            open_file_checked(
                &root,
                "target",
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE
                    | fio::Flags::PROTOCOL_FILE,
                &Default::default(),
            )
            .await;
            let (status, dst_token) = root.get_token().await.expect("FIDL call failed");
            zx::Status::ok(status).expect("get_token failed");
            root.rename("target", zx::Event::from(dst_token.unwrap()), "symlink")
                .await
                .expect("FIDL call failed")
                .expect("rename failed");

            proxy
                .get_attributes(fio::NodeAttributesQuery::empty())
                .await
                .expect("FIDL call failed")
                .expect("get_attributes failed");
            let node_info = proxy.describe().await.expect("FIDL call failed");
            assert_matches!(
                node_info,
                fio::SymlinkInfo { target: Some(target), .. } if target == b"target2"
            );
        }

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_symlink_link_into_unlinked_fails() {
        let fixture = TestFixture::new().await;
        {
            let root = fixture.root();

            // 1. Create a symlink.
            root.create_symlink("symlink", b"target", None)
                .await
                .expect("FIDL call failed")
                .expect("create_symlink failed");

            // 2. Open a connection to it.
            let (proxy, server_end) = create_proxy::<fio::SymlinkMarker>();
            root.open(
                "symlink",
                fio::PERM_READABLE | fio::PERM_WRITABLE,
                &Default::default(),
                server_end.into_channel(),
            )
            .expect("open failed");

            // 3. Unlink the symlink.
            root.unlink("symlink", &fio::UnlinkOptions::default())
                .await
                .expect("FIDL call failed")
                .expect("unlink failed");

            // 4. Try to LinkInto to a new path.
            let (status, dst_token) = root.get_token().await.expect("FIDL call failed");
            zx::Status::ok(status).expect("get_token failed");

            let link_result =
                proxy.link_into(zx::Event::from(dst_token.unwrap()), "symlink_new").await;
            assert_matches!(
                link_result,
                Ok(Err(status)) if status == zx::Status::NOT_FOUND.into_raw()
            );

            // 5. Verify that symlink_new does not exist.
            let (proxy_new, server_end_new) = create_proxy::<fio::SymlinkMarker>();
            root.open(
                "symlink_new",
                fio::PERM_READABLE,
                &Default::default(),
                server_end_new.into_channel(),
            )
            .expect("open failed");

            let describe_result = proxy_new.describe().await;
            assert_matches!(describe_result, Err(_));
        }
        fixture.close().await;
    }

    // Creates two files in an inner directory and creates a race between linking the first file
    // into another directory and renaming the second file over the first file. There is naive
    // TOCTOU bug since we need to take a lock on the source file being linked but we have to look
    // up what that file id is before we take any locks.
    #[fuchsia::test]
    async fn test_race_hard_link_with_unlink() {
        let fixture = TestFixture::new().await;
        {
            let root = fixture.root();

            let inner = open_dir_checked(
                root,
                "bar",
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE
                    | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
            .await;
            let inner2 = open_dir_checked(
                &inner,
                ".",
                fio::PERM_READABLE | fio::PERM_WRITABLE | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
            .await;

            let file = open_file_checked(
                &inner,
                "foo",
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE
                    | fio::Flags::PROTOCOL_FILE,
                &Default::default(),
            )
            .await;
            assert_eq!(
                file.write("valid".as_bytes()).await.expect("FIDL failed").expect("Write success"),
                5
            );
            close_file_checked(file).await;

            let file = open_file_checked(
                &inner,
                "foo2",
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE
                    | fio::Flags::PROTOCOL_FILE,
                &Default::default(),
            )
            .await;
            assert_eq!(
                file.write("valid".as_bytes()).await.expect("FIDL failed").expect("Write success"),
                5
            );
            close_file_checked(file).await;

            let inner_token = inner
                .get_token()
                .await
                .expect("fidl failed")
                .1
                .expect("get_token returned no handle");
            let root_token = root
                .get_token()
                .await
                .expect("fidl failed")
                .1
                .expect("get_token returned no handle");

            // Takes the lock on the destination dir of the link. Causing it to stall while trying
            // take the requisite lock to add a child there. A lock also needs to be taken on the
            // object being linked which will interfere with the rename call 50% of the time. Which
            // lock gets taken first depends on the sort order, which will be dependent on the
            // object ids for the two objects. So 50% of the time, this test can spuriously pass.
            let write_lock = fixture
                .fs()
                .lock_manager()
                .write_lock(lock_keys![LockKey::object(
                    fixture.volume().volume().store().store_object_id(),
                    fixture.volume().root_dir().directory().object_id()
                )])
                .await;

            join!(
                async move {
                    // Ensure that the other the link task can stay blocked while locking until
                    // renaming is complete.
                    fasync::Timer::new(Duration::from_millis(50)).await;
                    let _lock = write_lock;
                },
                async move {
                    // Give time for the link call to do some initial lookups that the rename will
                    // invalidate.
                    fasync::Timer::new(Duration::from_millis(25)).await;
                    inner
                        .rename("foo2", inner_token.into(), "foo")
                        .await
                        .expect("FIDL call failed")
                        .expect("Rename failed");
                },
                async move {
                    // This link should always succeed. Since the rename should be atomic, we
                    // should either link in the old file or the new.
                    assert_eq!(
                        inner2.link("foo", root_token, "baz").await.expect("Fidl call"),
                        zx::sys::ZX_OK
                    );
                }
            );
        }

        // Ensure that the file contents can be read back. If a race in object management happens
        // it may resurrect the object in the tree, but the extents will all still be missing.
        let file = open_file_checked(
            fixture.root(),
            "baz",
            fio::PERM_READABLE | fio::PERM_WRITABLE | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;
        let buff = file.read(5).await.expect("FIDL failed").expect("Read failed");
        close_file_checked(file).await;
        assert_eq!(buff.as_slice(), "valid".as_bytes());
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_hard_link_to_symlink() {
        let fixture = TestFixture::new().await;

        {
            let root = fixture.root();

            root.create_symlink("symlink", b"target", None)
                .await
                .expect("FIDL call failed")
                .expect("create_symlink failed");

            async fn open_symlink(root: &fio::DirectoryProxy, path: &str) -> fio::SymlinkProxy {
                let (proxy, server_end) = create_proxy::<fio::SymlinkMarker>();
                root.open(
                    path,
                    fio::PERM_READABLE | fio::PERM_WRITABLE | fio::Flags::FLAG_SEND_REPRESENTATION,
                    &Default::default(),
                    server_end.into_channel(),
                )
                .expect("open failed");

                let representation = proxy
                    .take_event_stream()
                    .next()
                    .await
                    .expect("missing Symlink event")
                    .expect("failed to read Symlink event")
                    .into_on_representation()
                    .expect("failed to decode OnRepresentation");

                assert_matches!(representation,
                    fio::Representation::Symlink(fio::SymlinkInfo{
                        target: Some(target), ..
                    }) if target == b"target"
                );

                proxy
            }

            let proxy = open_symlink(&root, "symlink").await;

            let (status, dst_token) = root.get_token().await.expect("FIDL call failed");
            zx::Status::ok(status).expect("get_token failed");
            proxy
                .link_into(zx::Event::from(dst_token.unwrap()), "symlink2")
                .await
                .expect("link_into (FIDL) failed")
                .expect("link_into failed");

            open_symlink(&root, "symlink2").await;
        }

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_symlink_stat() {
        let fixture = TestFixture::new().await;

        {
            let root = fixture.root();

            root.create_symlink("symlink", b"target", None)
                .await
                .expect("FIDL call failed")
                .expect("create_symlink failed");

            let root = fuchsia_fs::directory::clone(root).expect("clone failed");

            fasync::unblock(|| {
                let root: std::os::fd::OwnedFd =
                    fdio::create_fd(root.into_channel().unwrap().into_zx_channel().into())
                        .expect("create_fd failed");

                let mut stat: libc::stat = unsafe { std::mem::zeroed() };
                let name = std::ffi::CString::new("symlink").expect("CString::new failed");
                assert_eq!(
                    unsafe { libc::fstatat(root.as_raw_fd(), name.as_ptr(), &mut stat, 0) },
                    0
                );
            })
            .await;
        }

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_remove_dir_all_with_symlink() {
        // This test makes sure that remove_dir_all works.  At time of writing remove_dir_all uses
        // d_type from the directory entry to determine whether or not to recurse into directories,
        // so this tests that is working correctly.

        let fixture = TestFixture::new().await;

        {
            let root = fixture.root();

            let dir = open_dir_checked(
                &root,
                "dir",
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE
                    | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
            .await;

            dir.create_symlink("symlink", b"target", None)
                .await
                .expect("FIDL call failed")
                .expect("create_symlink failed");

            let namespace = fdio::Namespace::installed().expect("Unable to get namespace");
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let path = format!("/test_symlink_stat.{}", COUNTER.fetch_add(1, Ordering::Relaxed));
            let root = fuchsia_fs::directory::clone(root).expect("clone failed");
            namespace
                .bind(&path, ClientEnd::new(root.into_channel().unwrap().into_zx_channel()))
                .expect("bind failed");
            let path_copy = path.clone();
            scopeguard::defer!({
                let _ = namespace.unbind(&path_copy);
            });

            fasync::unblock(move || {
                assert_matches!(std::fs::remove_dir_all(&format!("{path}/dir")), Ok(()));
            })
            .await;
        }

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn extended_attributes() {
        let fixture = TestFixture::new().await;
        let root = fixture.root();

        let file = open_dir_checked(
            &root,
            "foo",
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_DIRECTORY,
            Default::default(),
        )
        .await;

        let name = b"security.selinux";
        let value_vec = b"bar".to_vec();

        {
            let (iterator_client, iterator_server) =
                fidl::endpoints::create_proxy::<fio::ExtendedAttributeIteratorMarker>();
            file.list_extended_attributes(iterator_server).expect("Failed to make FIDL call");
            let (chunk, last) = iterator_client
                .get_next()
                .await
                .expect("Failed to make FIDL call")
                .expect("Failed to get next iterator chunk");
            assert!(last);
            assert_eq!(chunk, Vec::<Vec<u8>>::new());
        }
        assert_eq!(
            file.get_extended_attribute(name)
                .await
                .expect("Failed to make FIDL call")
                .expect_err("Got successful message back for missing attribute"),
            zx::Status::NOT_FOUND.into_raw(),
        );

        file.set_extended_attribute(
            name,
            fio::ExtendedAttributeValue::Bytes(value_vec.clone()),
            fio::SetExtendedAttributeMode::Set,
        )
        .await
        .expect("Failed to make FIDL call")
        .expect("Failed to set extended attribute");

        {
            let (iterator_client, iterator_server) =
                fidl::endpoints::create_proxy::<fio::ExtendedAttributeIteratorMarker>();
            file.list_extended_attributes(iterator_server).expect("Failed to make FIDL call");
            let (chunk, last) = iterator_client
                .get_next()
                .await
                .expect("Failed to make FIDL call")
                .expect("Failed to get next iterator chunk");
            assert!(last);
            assert_eq!(chunk, vec![name]);
        }
        assert_eq!(
            file.get_extended_attribute(name)
                .await
                .expect("Failed to make FIDL call")
                .expect("Failed to get extended attribute"),
            fio::ExtendedAttributeValue::Bytes(value_vec)
        );

        file.remove_extended_attribute(name)
            .await
            .expect("Failed to make FIDL call")
            .expect("Failed to remove extended attribute");

        {
            let (iterator_client, iterator_server) =
                fidl::endpoints::create_proxy::<fio::ExtendedAttributeIteratorMarker>();
            file.list_extended_attributes(iterator_server).expect("Failed to make FIDL call");
            let (chunk, last) = iterator_client
                .get_next()
                .await
                .expect("Failed to make FIDL call")
                .expect("Failed to get next iterator chunk");
            assert!(last);
            assert_eq!(chunk, Vec::<Vec<u8>>::new());
        }
        assert_eq!(
            file.get_extended_attribute(name)
                .await
                .expect("Failed to make FIDL call")
                .expect_err("Got successful message back for missing attribute"),
            zx::Status::NOT_FOUND.into_raw(),
        );

        close_dir_checked(file).await;
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn extended_attribute_set_modes() {
        let fixture = TestFixture::new().await;
        let root = fixture.root();

        let dir = open_dir_checked(
            &root,
            "foo",
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_DIRECTORY,
            Default::default(),
        )
        .await;

        let name = b"security.selinux";
        let value_vec = b"bar".to_vec();
        let value2_vec = b"new value".to_vec();

        // Can't replace an attribute that doesn't exist yet.
        assert_eq!(
            dir.set_extended_attribute(
                name,
                fio::ExtendedAttributeValue::Bytes(value_vec.clone()),
                fio::SetExtendedAttributeMode::Replace
            )
            .await
            .expect("Failed to make FIDL call")
            .expect_err("Got successful message back from replacing a nonexistent attribute"),
            zx::Status::NOT_FOUND.into_raw()
        );

        // Create works when it doesn't exist.
        dir.set_extended_attribute(
            name,
            fio::ExtendedAttributeValue::Bytes(value_vec.clone()),
            fio::SetExtendedAttributeMode::Create,
        )
        .await
        .expect("Failed to make FIDL call")
        .expect("Failed to set xattr with create");

        // Create doesn't work once it exists though.
        assert_eq!(
            dir.set_extended_attribute(
                name,
                fio::ExtendedAttributeValue::Bytes(value2_vec.clone()),
                fio::SetExtendedAttributeMode::Create
            )
            .await
            .expect("Failed to make FIDL call")
            .expect_err("Got successful message back from replacing a nonexistent attribute"),
            zx::Status::ALREADY_EXISTS.into_raw()
        );

        // But replace does.
        dir.set_extended_attribute(
            name,
            fio::ExtendedAttributeValue::Bytes(value2_vec.clone()),
            fio::SetExtendedAttributeMode::Replace,
        )
        .await
        .expect("Failed to make FIDL call")
        .expect("Failed to set xattr with create");

        close_dir_checked(dir).await;
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_remove_large_xattr() {
        let fixture = TestFixture::new().await;
        {
            let root = fixture.root();
            let dir = open_dir_checked(
                &root,
                "foo",
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE
                    | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
            .await;

            dir.set_extended_attribute(
                "name".as_bytes(),
                fio::ExtendedAttributeValue::Bytes(vec![17u8; 300]),
                fio::SetExtendedAttributeMode::Create,
            )
            .await
            .expect("FIDL call failed")
            .expect("Set xattr failed");

            dir.remove_extended_attribute("name".as_bytes())
                .await
                .expect("FIDL call failed")
                .expect("Set xattr failed");
        }
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_create_dir_with_mutable_node_attributes() {
        let fixture = TestFixture::new().await;
        {
            let root_dir = fixture.volume().root_dir();

            let path_str = "foo";
            let path = Path::validate_and_split(path_str).unwrap();

            let (_proxy, server_end) = create_proxy::<fio::DirectoryMarker>();
            let mode: u32 = 0o123;
            let flags = fio::Flags::PROTOCOL_DIRECTORY | fio::Flags::FLAG_MAYBE_CREATE;
            let options = fio::Options {
                create_attributes: Some(fio::MutableNodeAttributes {
                    mode: Some(mode),
                    ..Default::default()
                }),
                ..Default::default()
            };

            let request = ObjectRequest::new(flags, &options, server_end.into_channel());
            let dir = root_dir.lookup(&flags, path, &request).await.expect("lookup failed");

            let attrs = dir
                .clone()
                .into_any()
                .downcast::<FxDirectory>()
                .expect("Not a directory")
                .get_attributes(
                    fio::NodeAttributesQuery::MODE
                        | fio::NodeAttributesQuery::UID
                        | fio::NodeAttributesQuery::ACCESS_TIME,
                )
                .await
                .expect("FIDL call failed");
            assert_eq!(attrs.mutable_attributes.mode.unwrap(), mode);
            // Since the POSIX mode attribute was set, we expect default values for the other POSIX
            // attributes.
            assert_eq!(attrs.mutable_attributes.uid.unwrap(), 0);
            // Expect these attributes to be None as they were not queried in `get_attributes(..)`
            assert!(attrs.mutable_attributes.gid.is_none());
            assert!(attrs.mutable_attributes.rdev.is_none());
            assert!(attrs.mutable_attributes.access_time.is_some());
        }
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_create_dir_with_default_mutable_node_attributes() {
        let fixture = TestFixture::new().await;
        {
            let root_dir = fixture.volume().root_dir();

            let path_str = "foo";
            let path = Path::validate_and_split(path_str).unwrap();

            let (_proxy, server_end) = create_proxy::<fio::DirectoryMarker>();
            let flags = fio::Flags::PROTOCOL_DIRECTORY | fio::Flags::FLAG_MAYBE_CREATE;
            let options = fio::Options {
                create_attributes: Some(fio::MutableNodeAttributes { ..Default::default() }),
                ..Default::default()
            };

            let request = ObjectRequest::new(flags, &options, server_end.into_channel());
            let dir = root_dir.lookup(&flags, path, &request).await.expect("lookup failed");

            let attrs = dir
                .clone()
                .into_any()
                .downcast::<FxDirectory>()
                .expect("Not a directory")
                .get_attributes(fio::NodeAttributesQuery::MODE)
                .await
                .expect("FIDL call failed");
            // Although mode was requested, it was not set when creating the directory. So we
            // expect None.
            assert!(attrs.mutable_attributes.mode.is_none());
            // The attributes not requested should be None.
            assert!(attrs.mutable_attributes.uid.is_none());
            assert!(attrs.mutable_attributes.gid.is_none());
            assert!(attrs.mutable_attributes.rdev.is_none());
            assert!(attrs.mutable_attributes.creation_time.is_none());
            assert!(attrs.mutable_attributes.modification_time.is_none());
            assert!(attrs.mutable_attributes.access_time.is_none());
        }
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_create_dir_using_flags_and_options() {
        let fixture = TestFixture::new().await;
        {
            let root_dir = fixture.volume().root_dir();

            let path_str = "foo";
            let path = Path::validate_and_split(path_str).unwrap();

            let (_proxy, server_end) = create_proxy::<fio::DirectoryMarker>();
            let mode: u32 = 0o123;
            let flags = fio::Flags::PROTOCOL_DIRECTORY | fio::Flags::FLAG_MAYBE_CREATE;
            let options = fio::Options {
                create_attributes: Some(fio::MutableNodeAttributes {
                    mode: Some(mode),
                    ..Default::default()
                }),
                ..Default::default()
            };

            // Create directory node.
            let request = ObjectRequest::new(flags, &options, server_end.into());
            let dir = root_dir.lookup(&flags, path, &request).await.expect("lookup failed");

            // Verify that the node was created with the attributes requested.
            let attrs = dir
                .clone()
                .into_any()
                .downcast::<FxDirectory>()
                .expect("Not a directory")
                .get_attributes(
                    fio::NodeAttributesQuery::MODE
                        | fio::NodeAttributesQuery::UID
                        | fio::NodeAttributesQuery::ACCESS_TIME,
                )
                .await
                .expect("FIDL call failed");
            assert_eq!(attrs.mutable_attributes.mode.unwrap(), mode);
            // Since the POSIX mode attribute was set, we expect default values for the other POSIX
            // attributes.
            assert_eq!(attrs.mutable_attributes.uid.unwrap(), 0);
            // Expect these attributes to be None as they were not queried in `get_attributes(..)`
            assert!(attrs.mutable_attributes.gid.is_none());
            assert!(attrs.mutable_attributes.rdev.is_none());
            assert!(attrs.mutable_attributes.access_time.is_some());
        }
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_create_file_with_mutable_node_attributes() {
        let fixture = TestFixture::new().await;
        {
            let root_dir = fixture.volume().root_dir();

            let path_str = "foo";
            let path = Path::validate_and_split(path_str).unwrap();

            let (_proxy, server_end) = create_proxy::<fio::FileMarker>();
            let mode: u32 = 0o123;
            let uid = 1;
            let gid = 2;
            let rdev = 3;
            let modification_time = Timestamp::now().as_nanos();

            let flags = fio::Flags::PROTOCOL_FILE | fio::Flags::FLAG_MAYBE_CREATE;
            let options = fio::Options {
                create_attributes: Some(fio::MutableNodeAttributes {
                    modification_time: Some(modification_time),
                    mode: Some(mode),
                    uid: Some(uid),
                    gid: Some(gid),
                    rdev: Some(rdev),
                    ..Default::default()
                }),
                ..Default::default()
            };

            let request = ObjectRequest::new(flags, &options, server_end.into_channel());
            let file = root_dir.lookup(&flags, path, &request).await.expect("lookup failed");

            let attributes = file
                .clone()
                .into_any()
                .downcast::<FxFile>()
                .expect("Not a file")
                .get_attributes(
                    fio::NodeAttributesQuery::CREATION_TIME
                        | fio::NodeAttributesQuery::MODIFICATION_TIME
                        | fio::NodeAttributesQuery::CHANGE_TIME
                        | fio::NodeAttributesQuery::MODE
                        | fio::NodeAttributesQuery::UID
                        | fio::NodeAttributesQuery::GID
                        | fio::NodeAttributesQuery::RDEV,
                )
                .await
                .expect("FIDL call failed");
            assert_eq!(mode, attributes.mutable_attributes.mode.unwrap());
            assert_eq!(uid, attributes.mutable_attributes.uid.unwrap());
            assert_eq!(gid, attributes.mutable_attributes.gid.unwrap());
            assert_eq!(rdev, attributes.mutable_attributes.rdev.unwrap());
            assert_eq!(modification_time, attributes.mutable_attributes.modification_time.unwrap());
            assert!(attributes.mutable_attributes.creation_time.is_some());
            assert!(attributes.immutable_attributes.change_time.is_some());
        }
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_create_file_with_default_mutable_node_attributes() {
        let fixture = TestFixture::new().await;
        {
            let root_dir = fixture.volume().root_dir();

            let path_str = "foo";
            let path = Path::validate_and_split(path_str).unwrap();

            let (_proxy, server_end) = create_proxy::<fio::FileMarker>();

            let flags = fio::Flags::PROTOCOL_FILE | fio::Flags::FLAG_MAYBE_CREATE;
            let options = Default::default();

            let request = ObjectRequest::new(flags, &options, server_end.into_channel());
            let file = root_dir.lookup(&flags, path, &request).await.expect("lookup failed");

            let attrs = file
                .clone()
                .into_any()
                .downcast::<FxFile>()
                .expect("Not a directory")
                .get_attributes(fio::NodeAttributesQuery::MODE)
                .await
                .expect("FIDL call failed");
            // Although mode was requested, it was not set when creating the directory. So we
            // expect that it is None.
            assert!(attrs.mutable_attributes.mode.is_none());
            // The attributes not requested should be None.
            assert!(attrs.mutable_attributes.uid.is_none());
            assert!(attrs.mutable_attributes.gid.is_none());
            assert!(attrs.mutable_attributes.rdev.is_none());
            assert!(attrs.mutable_attributes.creation_time.is_none());
            assert!(attrs.mutable_attributes.modification_time.is_none());
        }
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_create_file_using_flags_and_options() {
        let fixture = TestFixture::new().await;
        {
            let root_dir = fixture.volume().root_dir();

            let path_str = "foo";
            let path = Path::validate_and_split(path_str).unwrap();

            let (_proxy, server_end) = create_proxy::<fio::DirectoryMarker>();
            let mode: u32 = 0o123;
            let uid = 1;
            let gid = 2;
            let rdev = 3;
            let modification_time = Timestamp::now().as_nanos();
            let flags = fio::Flags::PROTOCOL_FILE | fio::Flags::FLAG_MAYBE_CREATE;
            let options = fio::Options {
                create_attributes: Some(fio::MutableNodeAttributes {
                    modification_time: Some(modification_time),
                    mode: Some(mode),
                    uid: Some(uid),
                    gid: Some(gid),
                    rdev: Some(rdev),
                    ..Default::default()
                }),
                ..Default::default()
            };

            // Create file node.
            let request = ObjectRequest::new(flags, &options, server_end.into());
            let file = root_dir.lookup(&flags, path, &request).await.expect("lookup failed");

            // Verify that the node was created with the attributes requested.
            let attributes = file
                .clone()
                .into_any()
                .downcast::<FxFile>()
                .expect("Not a file")
                .get_attributes(
                    fio::NodeAttributesQuery::CREATION_TIME
                        | fio::NodeAttributesQuery::MODIFICATION_TIME
                        | fio::NodeAttributesQuery::CHANGE_TIME
                        | fio::NodeAttributesQuery::MODE
                        | fio::NodeAttributesQuery::UID
                        | fio::NodeAttributesQuery::GID
                        | fio::NodeAttributesQuery::RDEV,
                )
                .await
                .expect("FIDL call failed");
            assert_eq!(mode, attributes.mutable_attributes.mode.unwrap());
            assert_eq!(uid, attributes.mutable_attributes.uid.unwrap());
            assert_eq!(gid, attributes.mutable_attributes.gid.unwrap());
            assert_eq!(rdev, attributes.mutable_attributes.rdev.unwrap());
            assert_eq!(modification_time, attributes.mutable_attributes.modification_time.unwrap());
            assert!(attributes.mutable_attributes.creation_time.is_some());
            assert!(attributes.immutable_attributes.change_time.is_some());
        }
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_update_attributes_also_updates_ctime() {
        let fixture = TestFixture::new().await;
        let root = fixture.root();

        let dir = open_dir_checked(
            &root,
            "foo",
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_DIRECTORY,
            Default::default(),
        )
        .await;

        let (_mutable_attributes, immutable_attributes) = dir
            .get_attributes(fio::NodeAttributesQuery::CHANGE_TIME)
            .await
            .expect("FIDL call failed")
            .map_err(zx::ok)
            .expect("get_attributes failed");

        dir.update_attributes(&fio::MutableNodeAttributes {
            modification_time: Some(Timestamp::now().as_nanos()),
            mode: Some(111),
            gid: Some(222),
            ..Default::default()
        })
        .await
        .expect("FIDL call failed")
        .map_err(zx::ok)
        .expect("update_attributes failed");

        let (_mutable_attributes, immutable_attributes_after_update) = dir
            .get_attributes(fio::NodeAttributesQuery::CHANGE_TIME)
            .await
            .expect("FIDL call failed")
            .map_err(zx::ok)
            .expect("get_attributes failed");
        assert!(immutable_attributes_after_update.change_time > immutable_attributes.change_time);
        fixture.close().await;
    }

    async fn open_to_get_selinux_context(
        root_dir: &fio::DirectoryProxy,
        path: &str,
        protocol: fio::Flags,
    ) -> Option<fio::SelinuxContext> {
        // Reopen, querying for the value.
        let flags =
            protocol | fio::Flags::FLAG_SEND_REPRESENTATION | fio::Flags::PERM_GET_ATTRIBUTES;
        let options = fio::Options {
            attributes: Some(fio::NodeAttributesQuery::SELINUX_CONTEXT),
            ..Default::default()
        };
        let (node, server_end) = create_proxy::<fio::NodeMarker>();
        root_dir.open(path, flags, &options, server_end.into_channel()).expect("Reopening node");
        let repr = node
            .take_event_stream()
            .next()
            .await
            .expect("Need representation")
            .expect("Failed to read")
            .into_on_representation()
            .unwrap();
        match repr {
            fio::Representation::Directory(fio::DirectoryInfo {
                attributes: Some(attr), ..
            })
            | fio::Representation::File(fio::FileInfo { attributes: Some(attr), .. })
            | fio::Representation::Symlink(fio::SymlinkInfo { attributes: Some(attr), .. }) => {
                attr.mutable_attributes.selinux_context
            }
            _ => panic!("Wrong type returned."),
        }
    }

    #[fuchsia::test]
    async fn test_selinux_context_via_open() {
        const CONTEXT: &str = "valid";
        const CONTEXT2: &str = "also_valid";
        let node_info: Vec<(&str, fio::Flags)> =
            vec![("dir", fio::Flags::PROTOCOL_DIRECTORY), ("file", fio::Flags::PROTOCOL_FILE)];
        let fixture = TestFixture::new().await;
        {
            let root_dir = fixture.root();

            for (path, protocol) in node_info {
                // Create node with the context.
                let flags = protocol
                    | fio::Flags::FLAG_SEND_REPRESENTATION
                    | fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE;
                let options = fio::Options {
                    create_attributes: Some(fio::MutableNodeAttributes {
                        selinux_context: Some(fio::SelinuxContext::Data(CONTEXT.into())),
                        ..Default::default()
                    }),
                    ..Default::default()
                };
                let (node, server_end) = create_proxy::<fio::NodeMarker>();
                root_dir
                    .open(path, flags, &options, server_end.into_channel())
                    .expect("Creating node");
                // Check event stream to allow the creation to complete.
                assert!(
                    node.take_event_stream()
                        .next()
                        .await
                        .expect("Need representation")
                        .expect("Failed to read")
                        .into_on_representation()
                        .is_some()
                );

                // Fetches the set value just fine.
                assert_eq!(
                    open_to_get_selinux_context(&root_dir, &path, protocol).await,
                    Some(fio::SelinuxContext::Data(CONTEXT.into()))
                );

                // See that it is synced with the xattr.
                node.set_extended_attribute(
                    fio::SELINUX_CONTEXT_NAME.as_bytes(),
                    fio::ExtendedAttributeValue::Bytes(CONTEXT2.into()),
                    fio::SetExtendedAttributeMode::Replace,
                )
                .await
                .unwrap()
                .expect("Updating xattr");
                assert_eq!(
                    open_to_get_selinux_context(&root_dir, &path, protocol).await,
                    Some(fio::SelinuxContext::Data(CONTEXT2.into()))
                );

                // Make it too long so that it must use the xattr interface.
                let vmo = zx::Vmo::create(4000).expect("Creating VMO");
                node.set_extended_attribute(
                    fio::SELINUX_CONTEXT_NAME.as_bytes(),
                    fio::ExtendedAttributeValue::Buffer(vmo),
                    fio::SetExtendedAttributeMode::Replace,
                )
                .await
                .unwrap()
                .expect("Updating xattr");
                assert_matches!(
                    open_to_get_selinux_context(&root_dir, &path, protocol).await,
                    Some(fio::SelinuxContext::UseExtendedAttributes(fio::EmptyStruct {}))
                );

                node.remove_extended_attribute(fio::SELINUX_CONTEXT_NAME.as_bytes())
                    .await
                    .unwrap()
                    .expect("Deleting xattr");
                assert_matches!(
                    open_to_get_selinux_context(&root_dir, &path, protocol).await,
                    None
                );
            }
        }
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_selinux_context_via_open_symlink() {
        const CONTEXT: &str = "valid";
        let fixture = TestFixture::new().await;
        {
            let path = "symlink";
            let root_dir = fixture.root();
            // Create node with the context.
            let (node, server_end) = create_proxy::<fio::SymlinkMarker>();
            root_dir
                .create_symlink(path, ".".as_bytes(), Some(server_end))
                .await
                .expect("Fidl query")
                .expect("Create symlink");

            node.set_extended_attribute(
                fio::SELINUX_CONTEXT_NAME.as_bytes(),
                fio::ExtendedAttributeValue::Bytes(CONTEXT.into()),
                fio::SetExtendedAttributeMode::Create,
            )
            .await
            .unwrap()
            .expect("Updating xattr");

            // Fetches the set value just fine.
            assert_eq!(
                open_to_get_selinux_context(&root_dir, &path, fio::Flags::PROTOCOL_SYMLINK).await,
                Some(fio::SelinuxContext::Data(CONTEXT.into()))
            );

            // Make it too long so that it must use the xattr interface.
            let vmo = zx::Vmo::create(4000).expect("Creating VMO");
            node.set_extended_attribute(
                fio::SELINUX_CONTEXT_NAME.as_bytes(),
                fio::ExtendedAttributeValue::Buffer(vmo),
                fio::SetExtendedAttributeMode::Replace,
            )
            .await
            .unwrap()
            .expect("Updating xattr");
            assert_matches!(
                open_to_get_selinux_context(&root_dir, &path, fio::Flags::PROTOCOL_SYMLINK).await,
                Some(fio::SelinuxContext::UseExtendedAttributes(fio::EmptyStruct {}))
            );

            // Erase it comes back with empty string.
            node.remove_extended_attribute(fio::SELINUX_CONTEXT_NAME.as_bytes())
                .await
                .unwrap()
                .expect("Deleting xattr");
            assert_matches!(
                open_to_get_selinux_context(&root_dir, &path, fio::Flags::PROTOCOL_SYMLINK).await,
                None
            );
        }
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_open_deleted_self() {
        let fixture = TestFixture::new().await;
        let root = fixture.root();

        let dir = open_dir_checked(
            &root,
            "foo",
            fio::Flags::FLAG_MAYBE_CREATE | fio::Flags::PROTOCOL_DIRECTORY,
            Default::default(),
        )
        .await;

        root.unlink("foo", &fio::UnlinkOptions::default())
            .await
            .expect("FIDL call failed")
            .expect("unlink failed");

        assert_eq!(
            open_dir(&root, "foo", fio::Flags::PROTOCOL_DIRECTORY, &Default::default())
                .await
                .expect_err("Open succeeded")
                .root_cause()
                .downcast_ref::<zx::Status>()
                .expect("No status"),
            &zx::Status::NOT_FOUND,
        );

        open_dir_checked(&dir, ".", fio::Flags::PROTOCOL_DIRECTORY, Default::default()).await;

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_open3_deleted_self() {
        let fixture = TestFixture::new().await;
        let root = fixture.root();

        const PATH: &str = "foo";

        let dir = open_dir_checked(
            &root,
            PATH,
            fio::Flags::FLAG_MAYBE_CREATE | fio::Flags::PROTOCOL_DIRECTORY,
            Default::default(),
        )
        .await;

        root.unlink(PATH, &fio::UnlinkOptions::default())
            .await
            .expect("FIDL call failed")
            .expect("unlink failed");

        assert_eq!(
            open_dir(
                &root,
                PATH,
                fio::Flags::PROTOCOL_DIRECTORY | fio::Flags::FLAG_SEND_REPRESENTATION,
                &fio::Options::default()
            )
            .await
            .expect_err("Open succeeded unexpectedly")
            .root_cause()
            .downcast_ref::<zx::Status>()
            .expect("No status"),
            &zx::Status::NOT_FOUND,
        );

        open_dir_checked(
            &dir,
            ".",
            fio::Flags::PROTOCOL_DIRECTORY | fio::Flags::FLAG_SEND_REPRESENTATION,
            fio::Options::default(),
        )
        .await;

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_update_attributes_persists() {
        const DIR: &str = "foo";
        let mtime = Some(Timestamp::now().as_nanos());
        let atime = Some(Timestamp::now().as_nanos());
        let mode = Some(111);

        let device = {
            let fixture = TestFixture::new().await;
            let root = fixture.root();

            let dir = open_dir_checked(
                &root,
                DIR,
                fio::Flags::FLAG_MAYBE_CREATE | fio::PERM_WRITABLE | fio::Flags::PROTOCOL_DIRECTORY,
                Default::default(),
            )
            .await;

            dir.update_attributes(&fio::MutableNodeAttributes {
                modification_time: mtime,
                access_time: atime,
                mode: mode,
                ..Default::default()
            })
            .await
            .expect("update_attributes FIDL call failed")
            .map_err(zx::ok)
            .expect("update_attributes failed");

            // Calling close should flush the node attributes to the device.
            fixture.close().await
        };

        let fixture =
            TestFixture::open(device, TestFixtureOptions { format: false, ..Default::default() })
                .await;
        let root = fixture.root();
        let dir = open_dir_checked(
            &root,
            DIR,
            fio::PERM_READABLE | fio::Flags::PROTOCOL_DIRECTORY,
            Default::default(),
        )
        .await;

        let (mutable_attributes, _immutable_attributes) = dir
            .get_attributes(
                fio::NodeAttributesQuery::MODIFICATION_TIME
                    | fio::NodeAttributesQuery::ACCESS_TIME
                    | fio::NodeAttributesQuery::MODE,
            )
            .await
            .expect("update_attributesFIDL call failed")
            .map_err(zx::ok)
            .expect("get_attributes failed");
        assert_eq!(mutable_attributes.modification_time, mtime);
        assert_eq!(mutable_attributes.access_time, atime);
        assert_eq!(mutable_attributes.mode, mode);
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_atime_from_pending_access_time_update_request() {
        const DIR: &str = "foo";

        let (device, expected_atime, expected_ctime) = {
            let fixture = TestFixture::new().await;
            let root = fixture.root();

            let dir = open_dir_checked(
                &root,
                DIR,
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_WRITABLE
                    | fio::Flags::PROTOCOL_DIRECTORY
                    | fio::Flags::PERM_GET_ATTRIBUTES,
                fio::Options {
                    attributes: Some(fio::NodeAttributesQuery::CHANGE_TIME),
                    ..Default::default()
                },
            )
            .await;

            let (mutable_attributes, immutable_attributes) = dir
                .get_attributes(
                    fio::NodeAttributesQuery::CHANGE_TIME
                        | fio::NodeAttributesQuery::ACCESS_TIME
                        | fio::NodeAttributesQuery::MODIFICATION_TIME,
                )
                .await
                .expect("update_attributes FIDL call failed")
                .map_err(zx::ok)
                .expect("get_attributes failed");
            let initial_ctime = immutable_attributes.change_time;
            let initial_atime = mutable_attributes.access_time;
            // When creating a node, ctime, mtime, and atime are all updated to the current time.
            assert_eq!(initial_atime, initial_ctime);
            assert_eq!(initial_atime, mutable_attributes.modification_time);

            // Client manages atime and they signal to Fxfs that an access has occurred and it may
            // require an access time update. They do so by querying with
            // `fio::NodeAttributesQuery::PENDING_ACCESS_TIME_UPDATE`.
            let (mutable_attributes, immutable_attributes) = dir
                .get_attributes(
                    fio::NodeAttributesQuery::CHANGE_TIME
                        | fio::NodeAttributesQuery::ACCESS_TIME
                        | fio::NodeAttributesQuery::PENDING_ACCESS_TIME_UPDATE,
                )
                .await
                .expect("update_attributes FIDL call failed")
                .map_err(zx::ok)
                .expect("get_attributes failed");
            // atime will be updated as atime <= ctime (or mtime)
            assert!(initial_atime < mutable_attributes.access_time);
            let updated_atime = mutable_attributes.access_time;
            // Calling get_attributes with PENDING_ACCESS_TIME_UPDATE will trigger an update of
            // object attributes if access_time needs to be updated. Check that ctime isn't updated.
            assert_eq!(initial_ctime, immutable_attributes.change_time);

            let (mutable_attributes, _) = dir
                .get_attributes(
                    fio::NodeAttributesQuery::ACCESS_TIME
                        | fio::NodeAttributesQuery::PENDING_ACCESS_TIME_UPDATE,
                )
                .await
                .expect("update_attributes FIDL call failed")
                .map_err(zx::ok)
                .expect("get_attributes failed");
            // atime will be not be updated as atime > ctime (or mtime)
            assert_eq!(updated_atime, mutable_attributes.access_time);

            (fixture.close().await, mutable_attributes.access_time, initial_ctime)
        };

        let fixture =
            TestFixture::open(device, TestFixtureOptions { format: false, ..Default::default() })
                .await;
        let root = fixture.root();
        let dir = open_dir_checked(
            &root,
            DIR,
            fio::PERM_READABLE | fio::Flags::PROTOCOL_DIRECTORY,
            Default::default(),
        )
        .await;

        let (mutable_attributes, immutable_attributes) = dir
            .get_attributes(
                fio::NodeAttributesQuery::CHANGE_TIME | fio::NodeAttributesQuery::ACCESS_TIME,
            )
            .await
            .expect("update_attributesFIDL call failed")
            .map_err(zx::ok)
            .expect("get_attributes failed");
        assert_eq!(immutable_attributes.change_time, expected_ctime);
        assert_eq!(mutable_attributes.access_time, expected_atime);
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_directory_immediately_tombstoned() {
        let fixture = TestFixture::new().await;
        let root = fixture.root();

        let dir = open_dir_checked(
            &root,
            "foo",
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_DIRECTORY
                | fio::Flags::PERM_GET_ATTRIBUTES,
            fio::Options::default(),
        )
        .await;

        let (_mutable, immutable) = dir
            .get_attributes(fio::NodeAttributesQuery::ID)
            .await
            .expect("transport error on get_attributes")
            .expect("get_attributes failed");
        let foo_object_id = immutable.id.unwrap();

        let dir = open_dir_checked(
            &root,
            "bar",
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_DIRECTORY
                | fio::Flags::PERM_GET_ATTRIBUTES,
            fio::Options::default(),
        )
        .await;

        let (_mutable, immutable) = dir
            .get_attributes(fio::NodeAttributesQuery::ID)
            .await
            .expect("transport error on get_attributes")
            .expect("get_attributes failed");
        let bar_object_id = immutable.id.unwrap();

        // Check rename.
        let (status, dst_token) = root.get_token().await.expect("FIDL call failed");
        zx::Status::ok(status).expect("get_token failed");
        root.rename("foo", zx::Event::from(dst_token.unwrap()), "bar")
            .await
            .expect("FIDL call failed")
            .expect("rename failed");

        // Allow the graveyard to run.
        yield_to_executor().await;

        // The easiest way to verify the object has been deleted is to scan the LSM tree.
        let assert_not_found = async |oid| {
            let tree = fixture.volume().volume().store().tree();
            let layer_set = tree.layer_set();
            let mut merger = layer_set.merger();
            let mut iter = merger.query(Query::FullScan).await.unwrap();
            while let Some(item) = iter.get() {
                match item {
                    ItemRef { value: ObjectValue::None, .. } => {}
                    ItemRef {
                        key: ObjectKey { object_id, data: ObjectKeyData::Object }, ..
                    } => {
                        assert_ne!(*object_id, oid);
                    }
                    _ => {}
                }
                iter.advance().await.unwrap();
            }
        };

        assert_not_found(bar_object_id).await;

        // Now check unlink.
        root.unlink("bar", &Default::default())
            .await
            .expect("FIDL call failed")
            .expect("unlink failed");

        assert_not_found(foo_object_id).await;

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_failed_create_unnamed_file_transaction() {
        let fail = AtomicU64::new(0);
        let (mut hooks, fs_hooks) = fxfs::hooks::Hooks::new();
        hooks.set_pre_commit(|_| {
            if fail.load(Ordering::Relaxed) > 0 {
                fail.fetch_sub(1, Ordering::Relaxed);
                bail!("Aborted transaction");
            }
            Ok(())
        });
        let fixture = TestFixture::open(
            DeviceHolder::new(FakeDevice::new(16384, 512)),
            TestFixtureOptions { hooks: Some(fs_hooks), ..Default::default() },
        )
        .await;
        let root = fixture.root();

        fail.fetch_add(1, Ordering::Relaxed);

        let _dir = open_file(
            &root,
            ".",
            fio::Flags::FLAG_CREATE_AS_UNNAMED_TEMPORARY,
            &fio::Options::default(),
        )
        .await
        .expect_err("Create unexpectedly succeeded");

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_update_access_time_on_deleted_directory() {
        let fixture = TestFixture::new().await;
        let root = fixture.root();

        let dir = open_dir_checked(
            &root,
            "foo",
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_DIRECTORY,
            fio::Options::default(),
        )
        .await;

        root.unlink("foo", &fio::UnlinkOptions::default())
            .await
            .expect("FIDL call failed")
            .expect("unlink failed");

        // Requesting PENDING_ACCESS_TIME_UPDATE should not fail even if the directory is deleted.
        dir.get_attributes(fio::NodeAttributesQuery::PENDING_ACCESS_TIME_UPDATE)
            .await
            .expect("FIDL call failed")
            .expect("get_attributes failed");

        fixture.close().await;
    }

    async fn enable_casefold(dir: &fio::DirectoryProxy) {
        dir.update_attributes(&fio::MutableNodeAttributes {
            casefold: Some(true),
            ..Default::default()
        })
        .await
        .expect("update_attributes FIDL call failed")
        .map_err(zx::ok)
        .expect("update_attributes failed");
    }

    #[fuchsia::test]
    async fn test_casefold_cache_lookup_no_duplicates() {
        let fixture = TestFixture::new().await;
        let root = fixture.root();

        let dir = open_dir_checked(
            &root,
            "dir",
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_DIRECTORY,
            Default::default(),
        )
        .await;
        enable_casefold(&dir).await;

        let file = open_file_checked(
            &dir,
            "foo",
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;
        close_file_checked(file).await;

        let cache = fixture.volume().volume().dirent_cache();
        cache.clear();

        let file = open_file_checked(
            &dir,
            "foo",
            fio::PERM_READABLE | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;
        close_file_checked(file).await;

        let len_after_first_lookup = cache.len();
        assert!(len_after_first_lookup >= 1, "Cache should contain at least the looked up file");

        let file = open_file_checked(
            &dir,
            "FOO",
            fio::PERM_READABLE | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;
        close_file_checked(file).await;

        assert_eq!(
            cache.len(),
            len_after_first_lookup,
            "Cache size increased (duplicate entries found)"
        );

        let file = open_file_checked(
            &dir,
            "Foo",
            fio::PERM_READABLE | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;
        close_file_checked(file).await;

        assert_eq!(
            cache.len(),
            len_after_first_lookup,
            "Cache size increased (duplicate entries found)"
        );

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_casefold_cache_rename_invalidation() {
        let fixture = TestFixture::new().await;
        let root = fixture.root();

        let dir = open_dir_checked(
            &root,
            "dir",
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_DIRECTORY,
            Default::default(),
        )
        .await;
        enable_casefold(&dir).await;

        let file = open_file_checked(
            &dir,
            "foo",
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;
        close_file_checked(file).await;

        let file = open_file_checked(
            &dir,
            "foo",
            fio::PERM_READABLE | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;
        close_file_checked(file).await;

        let (status, dst_token) = dir.get_token().await.expect("FIDL call failed");
        zx::Status::ok(status).expect("get_token failed");
        dir.rename("FOO", zx::Event::from(dst_token.unwrap()), "bar")
            .await
            .expect("Rename FIDL call failed")
            .expect("rename failed");

        assert_matches!(
            open_file(
                &dir,
                "foo",
                fio::PERM_READABLE | fio::Flags::PROTOCOL_FILE,
                &Default::default()
            )
            .await
            .expect_err("Open \"foo\" succeeded after rename")
            .root_cause()
            .downcast_ref::<zx::Status>(),
            Some(&zx::Status::NOT_FOUND)
        );

        let file = open_file_checked(
            &dir,
            "bar",
            fio::PERM_READABLE | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;
        close_file_checked(file).await;

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_casefold_cache_unlink_invalidation() {
        let fixture = TestFixture::new().await;
        let root = fixture.root();

        let dir = open_dir_checked(
            &root,
            "dir",
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_DIRECTORY,
            Default::default(),
        )
        .await;
        enable_casefold(&dir).await;

        let file = open_file_checked(
            &dir,
            "foo",
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;
        close_file_checked(file).await;

        let file = open_file_checked(
            &dir,
            "foo",
            fio::PERM_READABLE | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;
        close_file_checked(file).await;

        dir.unlink("FOO", &fio::UnlinkOptions::default())
            .await
            .expect("Unlink FIDL call failed")
            .expect("Unlink failed");

        assert_matches!(
            open_file(
                &dir,
                "foo",
                fio::PERM_READABLE | fio::Flags::PROTOCOL_FILE,
                &Default::default()
            )
            .await
            .expect_err("Open \"foo\" succeeded after unlink")
            .root_cause()
            .downcast_ref::<zx::Status>(),
            Some(&zx::Status::NOT_FOUND)
        );

        fixture.close().await;
    }

    async fn assert_watch_event(
        watcher: &mut Watcher,
        expected_event: WatchEvent,
        expected_name: &str,
    ) {
        assert_eq!(
            watcher.next().await.unwrap().unwrap(),
            WatchMessage { event: expected_event, filename: expected_name.into() }
        );
    }

    async fn set_up_casefold_dir_with_files(
        fixture: &TestFixture,
        dir_name: &str,
        files: &[&str],
    ) -> (fio::DirectoryProxy, Watcher) {
        let root = fixture.root();
        let dir = open_dir_checked(
            root,
            dir_name,
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_DIRECTORY,
            Default::default(),
        )
        .await;
        enable_casefold(&dir).await;

        for &file_name in files {
            let file = open_file_checked(
                &dir,
                file_name,
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE
                    | fio::Flags::PROTOCOL_FILE,
                &Default::default(),
            )
            .await;
            close_file_checked(file).await;
        }

        let mut watcher = Watcher::new(&dir).await.unwrap();
        assert_watch_event(&mut watcher, WatchEvent::EXISTING, ".").await;

        let mut existing_files = files
            .iter()
            .map(|s| std::path::PathBuf::from(*s))
            .collect::<std::collections::HashSet<_>>();
        while !existing_files.is_empty() {
            let msg = watcher.next().await.unwrap().unwrap();
            assert_eq!(msg.event, WatchEvent::EXISTING);
            assert!(
                existing_files.remove(&msg.filename),
                "Unexpected existing file: {:?}",
                msg.filename
            );
        }

        assert_watch_event(&mut watcher, WatchEvent::IDLE, "").await;

        (dir, watcher)
    }

    #[fuchsia::test]
    async fn test_casefold_rename_watcher_events() {
        let fixture = TestFixture::new().await;
        let (dir, mut watcher) = set_up_casefold_dir_with_files(&fixture, "dir", &["foo"]).await;

        let (status, dst_token) = dir.get_token().await.expect("FIDL call failed");
        zx::Status::ok(status).expect("get_token failed");
        dir.rename("FOO", zx::Event::from(dst_token.unwrap()), "BAR")
            .await
            .expect("Rename FIDL call failed")
            .expect("rename failed");

        assert_watch_event(&mut watcher, WatchEvent::REMOVE_FILE, "foo").await;
        assert_watch_event(&mut watcher, WatchEvent::ADD_FILE, "BAR").await;

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_casefold_rename_overwrite_watcher_events() {
        let fixture = TestFixture::new().await;
        let (dir, mut watcher) =
            set_up_casefold_dir_with_files(&fixture, "dir", &["foo", "bar"]).await;

        let (status, dst_token) = dir.get_token().await.expect("FIDL call failed");
        zx::Status::ok(status).expect("get_token failed");
        dir.rename("FOO", zx::Event::from(dst_token.unwrap()), "BAR")
            .await
            .expect("Rename FIDL call failed")
            .expect("rename failed");

        assert_watch_event(&mut watcher, WatchEvent::REMOVE_FILE, "foo").await;
        assert_watch_event(&mut watcher, WatchEvent::REMOVE_FILE, "bar").await;
        assert_watch_event(&mut watcher, WatchEvent::ADD_FILE, "BAR").await;

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_casefold_rename_cross_dir_overwrite_watcher_events() {
        let fixture = TestFixture::new().await;
        let (src_dir, mut src_watcher) =
            set_up_casefold_dir_with_files(&fixture, "src_dir", &["foo"]).await;
        let (dst_dir, mut dst_watcher) =
            set_up_casefold_dir_with_files(&fixture, "dst_dir", &["bar"]).await;

        let (status, dst_token) = dst_dir.get_token().await.expect("FIDL call failed");
        zx::Status::ok(status).expect("get_token failed");
        src_dir
            .rename("FOO", zx::Event::from(dst_token.unwrap()), "BAR")
            .await
            .expect("Rename FIDL call failed")
            .expect("rename failed");

        assert_watch_event(&mut src_watcher, WatchEvent::REMOVE_FILE, "foo").await;

        assert_watch_event(&mut dst_watcher, WatchEvent::REMOVE_FILE, "bar").await;
        assert_watch_event(&mut dst_watcher, WatchEvent::ADD_FILE, "BAR").await;

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_casefold_unlink_watcher_events() {
        let fixture = TestFixture::new().await;
        let (dir, mut watcher) = set_up_casefold_dir_with_files(&fixture, "dir", &["foo"]).await;

        dir.unlink("FOO", &fio::UnlinkOptions::default())
            .await
            .expect("Unlink FIDL call failed")
            .expect("Unlink failed");

        assert_watch_event(&mut watcher, WatchEvent::REMOVE_FILE, "foo").await;

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_casefold_rename_case_only_watcher_events() {
        let fixture = TestFixture::new().await;
        let (dir, mut watcher) = set_up_casefold_dir_with_files(&fixture, "dir", &["Foo"]).await;

        let (status, dst_token) = dir.get_token().await.expect("FIDL call failed");
        zx::Status::ok(status).expect("get_token failed");
        dir.rename("Foo", zx::Event::from(dst_token.unwrap()), "FOO")
            .await
            .expect("Rename FIDL call failed")
            .expect("rename failed");

        assert_watch_event(&mut watcher, WatchEvent::REMOVE_FILE, "Foo").await;
        assert_watch_event(&mut watcher, WatchEvent::ADD_FILE, "FOO").await;

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_unlink_purges_in_single_transaction_when_closed() {
        let fixture = TestFixture::new().await;
        let root = fixture.root();
        let volume = fixture.volume().volume();
        let store = volume.store();
        let root_dir = fixture
            .volume()
            .root()
            .clone()
            .into_any()
            .downcast::<FxDirectory>()
            .expect("Not a directory");

        // 1. Verify that calling `remove_from_dirent_cache` on a closed file drops the last
        // strong reference and synchronously removes it from `NodeCache`.
        let file = open_file_checked(
            root,
            "cached_file",
            fio::Flags::FLAG_MAYBE_CREATE | fio::PERM_READABLE | fio::PERM_WRITABLE,
            &fio::Options::default(),
        )
        .await;
        let (_mutable, immutable) = file
            .get_attributes(fio::NodeAttributesQuery::ID)
            .await
            .expect("transport error on get_attributes")
            .expect("get_attributes failed");
        let cached_id = immutable.id.unwrap();
        file::write(&file, b"hello").await.expect("write failed");
        close_file_checked(file).await;

        // While the file is closed, `dirent_cache` still holds a strong `Arc<dyn FxNode>`, so
        // `NodeCache` contains the entry.
        assert!(volume.cache().contains_key(cached_id));
        root_dir.remove_from_dirent_cache("cached_file");
        assert!(!volume.cache().contains_key(cached_id));

        // 2. Create another file, write to it, and close it without manually removing from
        // `dirent_cache`. Verify that `unlink` evicts it from `dirent_cache`, sees that it is no
        // longer in `NodeCache`, and purges it in a single transaction (never touching the
        // graveyard).
        let closed_file = open_file_checked(
            root,
            "closed_file",
            fio::Flags::FLAG_MAYBE_CREATE | fio::PERM_READABLE | fio::PERM_WRITABLE,
            &fio::Options::default(),
        )
        .await;
        let (_mutable, immutable) = closed_file
            .get_attributes(fio::NodeAttributesQuery::ID)
            .await
            .expect("transport error on get_attributes")
            .expect("get_attributes failed");
        let closed_id = immutable.id.unwrap();
        file::write(&closed_file, b"hello").await.expect("write failed");
        close_file_checked(closed_file).await;

        assert!(volume.cache().contains_key(closed_id));
        root.unlink("closed_file", &fio::UnlinkOptions::default())
            .await
            .expect("FIDL call failed")
            .expect("unlink failed");
        assert!(!volume.cache().contains_key(closed_id));
        // Because it was purged in a single transaction, no graveyard entry was ever inserted.
        assert!(
            store
                .tree()
                .find(&ObjectKey::graveyard_entry(store.graveyard_directory_object_id(), closed_id))
                .await
                .expect("find failed")
                .is_none()
        );

        // 3. Contrast with unlinking an open file: since an open connection holds a strong
        // reference, `contains_key` remains true after `remove_from_dirent_cache`, so `unlink`
        // falls back to adding the file to the graveyard.
        let open_file = open_file_checked(
            root,
            "open_file",
            fio::Flags::FLAG_MAYBE_CREATE | fio::PERM_READABLE | fio::PERM_WRITABLE,
            &fio::Options::default(),
        )
        .await;
        let (_mutable, immutable) = open_file
            .get_attributes(fio::NodeAttributesQuery::ID)
            .await
            .expect("transport error on get_attributes")
            .expect("get_attributes failed");
        let open_id = immutable.id.unwrap();
        file::write(&open_file, b"hello").await.expect("write failed");

        root.unlink("open_file", &fio::UnlinkOptions::default())
            .await
            .expect("FIDL call failed")
            .expect("unlink failed");
        assert!(volume.cache().contains_key(open_id));
        assert!(
            store
                .tree()
                .find(&ObjectKey::graveyard_entry(store.graveyard_directory_object_id(), open_id))
                .await
                .expect("find failed")
                .is_some()
        );
        close_file_checked(open_file).await;

        drop(root_dir);
        fixture.close().await;
    }
}
