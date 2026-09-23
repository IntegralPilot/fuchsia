// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::file::FatFile;
use crate::filesystem::FatFilesystem;
use crate::node::{Closer, FatNode, Node, WeakFatNode};
use crate::refs::{FatfsDirRef, FatfsFileRef};
use crate::types::{Dir, DirEntry, File};
use crate::util::{
    dos_date_to_unix_time, dos_to_unix_time, fatfs_error_to_status, unix_to_dos_time,
};
use fatfs::validate_filename;
use fidl::endpoints::ServerEnd;
use fidl_fuchsia_io as fio;
use fragile::Fragile;
use futures::future::BoxFuture;
use std::borrow::Borrow;
use std::cell::{Ref, RefCell, RefMut};
use std::collections::HashMap;
use std::fmt::Debug;
use std::hash::{Hash, Hasher};
use std::ops::Deref;
use std::rc::Rc;
use std::sync::Arc;
use vfs::directory::dirents_sink::{self, AppendResult};
use vfs::directory::entry::{DirectoryEntry, EntryInfo, GetEntryInfo, OpenRequest};
use vfs::directory::entry_container::{Directory, DirectoryWatcher, MutableDirectory};
use vfs::directory::mutable::connection::MutableConnection;
use vfs::directory::traversal_position::TraversalPosition;
use vfs::directory::watchers::Watchers;
use vfs::directory::watchers::event_producers::{SingleNameEventProducer, StaticVecEventProducer};
use vfs::execution_scope::ExecutionScope;
use vfs::file::FidlIoConnection;
use vfs::path::Path;
use vfs::{ObjectRequestRef, ProtocolsExt as _, ToObjectRequest, attributes};
use zx::Status;

fn check_open_flags_for_existing_entry(flags: fio::OpenFlags) -> Result<(), Status> {
    if flags.intersects(fio::OpenFlags::CREATE_IF_ABSENT) {
        return Err(Status::ALREADY_EXISTS);
    }
    // Other flags are verified by VFS's new_connection_validate_flags method.
    Ok(())
}

struct FatDirectoryData {
    /// The parent directory of this entry. Might be None if this is the root directory,
    /// or if this directory has been deleted.
    parent: Option<Arc<FatDirectory>>,
    /// We keep a cache of `FatDirectory`/`FatFile`s to ensure
    /// there is only ever one canonical version of each. This means
    /// we can use the reference count in the Arc<> to make sure rename, etc. operations are safe.
    children: HashMap<InsensitiveString, WeakFatNode>,
    /// True if this directory has been deleted.
    deleted: bool,
    watchers: Watchers,
    /// Name of this directory. TODO: we should be able to change to HashSet.
    name: String,
}

// Whilst it's tempting to use the unicase crate, at time of writing, it had its own case tables,
// which might not match Rust's built-in tables (which is what fatfs uses).  It's important what we
// do here is consistent with the fatfs crate.  It would be nice if that were consistent with other
// implementations, but it probably isn't the end of the world if it isn't since we shouldn't have
// clients using obscure ranges of Unicode.
struct InsensitiveString(String);

impl Hash for InsensitiveString {
    fn hash<H: Hasher>(&self, hasher: &mut H) {
        for c in self.0.chars().flat_map(|c| c.to_uppercase()) {
            hasher.write_u32(c as u32);
        }
    }
}

impl PartialEq for InsensitiveString {
    fn eq(&self, other: &Self) -> bool {
        self.0
            .chars()
            .flat_map(|c| c.to_uppercase())
            .eq(other.0.chars().flat_map(|c| c.to_uppercase()))
    }
}

impl Eq for InsensitiveString {}

// A trait that allows us to find entries in our hash table using &str.
pub(crate) trait InsensitiveStringRef {
    fn as_str(&self) -> &str;
}

impl<'a> Borrow<dyn InsensitiveStringRef + 'a> for InsensitiveString {
    fn borrow(&self) -> &(dyn InsensitiveStringRef + 'a) {
        self
    }
}

impl<'a> Eq for dyn InsensitiveStringRef + 'a {}

impl<'a> PartialEq for dyn InsensitiveStringRef + 'a {
    fn eq(&self, other: &dyn InsensitiveStringRef) -> bool {
        self.as_str()
            .chars()
            .flat_map(|c| c.to_uppercase())
            .eq(other.as_str().chars().flat_map(|c| c.to_uppercase()))
    }
}

impl<'a> Hash for dyn InsensitiveStringRef + 'a {
    fn hash<H: Hasher>(&self, hasher: &mut H) {
        for c in self.as_str().chars().flat_map(|c| c.to_uppercase()) {
            hasher.write_u32(c as u32);
        }
    }
}

impl InsensitiveStringRef for &str {
    fn as_str(&self) -> &str {
        self
    }
}

impl InsensitiveStringRef for InsensitiveString {
    fn as_str(&self) -> &str {
        &self.0
    }
}

/// This wraps a directory on the FAT volume.
pub struct FatDirectory(Fragile<FatDirectoryInner>);

pub struct FatDirectoryInner {
    /// The underlying directory.
    dir: FatfsDirRef,
    /// Other information about this FatDirectory.
    data: RefCell<FatDirectoryData>,
}

impl FatDirectoryInner {
    pub(crate) fn fs(&self) -> &Rc<FatFilesystem> {
        self.dir.filesystem()
    }
}

impl Deref for FatDirectory {
    type Target = FatDirectoryInner;

    fn deref(&self) -> &Self::Target {
        self.0.get()
    }
}

enum ExistingRef<'a, 'b> {
    None,
    File(&'a mut crate::types::File<'b>),
    Dir(&'a mut crate::types::Dir<'b>),
}

impl FatDirectory {
    /// Create a new FatDirectory.
    pub(crate) fn new(
        dir: FatfsDirRef,
        parent: Option<Arc<FatDirectory>>,
        name: String,
    ) -> Arc<Self> {
        Arc::new(FatDirectory(Fragile::new(FatDirectoryInner {
            dir,
            data: RefCell::new(FatDirectoryData {
                parent,
                children: HashMap::new(),
                deleted: false,
                watchers: Watchers::new(),
                name,
            }),
        })))
    }

    pub(crate) fn borrow_dir<'a>(&'a self) -> Result<Ref<'a, Dir<'a>>, Status> {
        self.dir.get().ok_or(Status::BAD_HANDLE)
    }

    /// Borrow the underlying fatfs `Dir` that corresponds to this directory.
    pub(crate) fn borrow_dir_mut<'a>(&'a self) -> Option<RefMut<'a, Dir<'a>>> {
        self.dir.get_mut()
    }

    /// Gets a child directory entry from the underlying fatfs implementation.
    pub(crate) fn find_child<'a>(&'a self, name: &str) -> Result<Option<DirEntry<'a>>, Status> {
        if self.data.borrow().deleted {
            return Ok(None);
        }
        let dir = self.borrow_dir()?;
        for entry in dir.iter().into_iter() {
            let entry = entry.map_err(fatfs_error_to_status)?;
            if entry.eq_name(name) {
                return Ok(Some(entry));
            }
        }
        Ok(None)
    }

    /// Remove and detach a child node from this FatDirectory, returning it if it exists in the
    /// cache.  The caller must ensure that the corresponding filesystem entry is removed to prevent
    /// the item being added back to the cache, and must later attach() the returned node somewhere.
    pub fn remove_child(&self, name: &str) -> Option<FatNode> {
        let node = self.cache_remove(name);
        if let Some(node) = node {
            node.detach();
            Some(node)
        } else {
            None
        }
    }

    /// Add and attach a child node to this FatDirectory. The caller needs to make sure that the
    /// entry corresponds to a node on the filesystem, and that there is no existing entry with
    /// that name in the cache.
    pub fn add_child(self: &Arc<Self>, name: String, child: FatNode) -> Result<(), Status> {
        child.attach(self.clone(), &name)?;
        // We only add back to the cache if the above succeeds, otherwise we have no
        // interest in serving more connections to a file that doesn't exist.
        let mut data = self.data.borrow_mut();
        // TODO: need to delete cache entries somewhere.
        if let Some(node) = data.children.insert(InsensitiveString(name), child.downgrade()) {
            assert!(node.upgrade().is_none(), "conflicting cache entries with the same name")
        }
        Ok(())
    }

    /// Remove a child entry from the cache, if it exists.
    pub(crate) fn cache_remove(&self, name: &str) -> Option<FatNode> {
        let mut data = self.data.borrow_mut();
        data.children.remove(&name as &dyn InsensitiveStringRef).and_then(|entry| entry.upgrade())
    }

    /// Lookup a child entry in the cache.
    pub fn cache_get(&self, name: &str) -> Option<FatNode> {
        self.data
            .borrow()
            .children
            .get(&name as &dyn InsensitiveStringRef)
            .and_then(|entry| entry.upgrade())
    }

    fn lookup(
        self: &Arc<Self>,
        flags: fio::OpenFlags,
        mut path: Path,
        closer: &mut Closer,
    ) -> Result<FatNode, Status> {
        let mut cur_entry = FatNode::Dir(self.clone());

        while !path.is_empty() {
            let child_flags =
                if path.is_single_component() { flags } else { fio::OpenFlags::DIRECTORY };

            match cur_entry {
                FatNode::Dir(entry) => {
                    let name = path.next().unwrap();
                    validate_filename(name).map_err(fatfs_error_to_status)?;
                    cur_entry = entry.open_child(name, child_flags, closer)?;
                }
                FatNode::File(_) => {
                    return Err(Status::NOT_DIR);
                }
            };
        }

        Ok(cur_entry)
    }

    fn lookup_with_open3_flags(
        self: &Arc<Self>,
        flags: fio::Flags,
        mut path: Path,
        closer: &mut Closer,
    ) -> Result<FatNode, Status> {
        let mut current_entry = FatNode::Dir(self.clone());

        while !path.is_empty() {
            let child_flags =
                if path.is_single_component() { flags } else { fio::Flags::PROTOCOL_DIRECTORY };

            match current_entry {
                FatNode::Dir(entry) => {
                    let name = path.next().unwrap();
                    validate_filename(name).map_err(fatfs_error_to_status)?;
                    current_entry = entry.open3_child(name, child_flags, closer)?;
                }
                FatNode::File(_) => {
                    return Err(Status::NOT_DIR);
                }
            };
        }

        Ok(current_entry)
    }

    /// Open a child entry with the given name.
    /// Flags can be any of the following, matching their fuchsia.io definitions:
    /// * OPEN_FLAG_CREATE
    /// * OPEN_FLAG_CREATE_IF_ABSENT
    /// * OPEN_FLAG_DIRECTORY
    /// * OPEN_FLAG_NOT_DIRECTORY
    pub(crate) fn open_child(
        self: &Arc<Self>,
        name: &str,
        flags: fio::OpenFlags,
        closer: &mut Closer,
    ) -> Result<FatNode, Status> {
        // First, check the cache.
        if let Some(entry) = self.cache_get(name) {
            check_open_flags_for_existing_entry(flags)?;
            entry.open_ref()?;
            return Ok(closer.add(entry));
        };

        let mut created = false;
        let node = {
            // Cache failed - try the real filesystem.
            let entry = self.find_child(name)?;
            if let Some(entry) = entry {
                check_open_flags_for_existing_entry(flags)?;
                if entry.is_dir() {
                    self.add_directory(entry.to_dir(), name, closer)
                } else {
                    self.add_file(entry.to_file(), name, closer)
                }
            } else if flags.intersects(fio::OpenFlags::CREATE) {
                // Child entry does not exist, but we've been asked to create it.
                created = true;
                let dir = self.borrow_dir()?;
                if flags.intersects(fio::OpenFlags::DIRECTORY) {
                    let dir = dir.create_dir(name).map_err(fatfs_error_to_status)?;
                    self.add_directory(dir, name, closer)
                } else {
                    let file = dir.create_file(name).map_err(fatfs_error_to_status)?;
                    self.add_file(file, name, closer)
                }
            } else {
                // Not creating, and no existing entry => not found.
                return Err(Status::NOT_FOUND);
            }
        };

        let mut data = self.data.borrow_mut();
        data.children.insert(InsensitiveString(name.to_owned()), node.downgrade());
        if created {
            data.watchers.send_event(&mut SingleNameEventProducer::added(name));
            self.fs().mark_dirty();
        }

        Ok(node)
    }

    pub(crate) fn open3_child(
        self: &Arc<Self>,
        name: &str,
        flags: fio::Flags,
        closer: &mut Closer,
    ) -> Result<FatNode, Status> {
        if flags.create_unnamed_temporary_in_directory_path() {
            return Err(Status::NOT_SUPPORTED);
        }
        // Check if the entry already exists in the cache.
        if let Some(entry) = self.cache_get(name) {
            if flags.creation_mode() == vfs::CreationMode::Always {
                return Err(Status::ALREADY_EXISTS);
            }
            entry.open_ref()?;
            return Ok(closer.add(entry));
        };

        let mut created_entry = false;
        let node = match self.find_child(name)? {
            Some(entry) => {
                if flags.creation_mode() == vfs::CreationMode::Always {
                    return Err(Status::ALREADY_EXISTS);
                }
                if entry.is_dir() {
                    self.add_directory(entry.to_dir(), name, closer)
                } else {
                    self.add_file(entry.to_file(), name, closer)
                }
            }
            None => {
                if flags.creation_mode() == vfs::CreationMode::Never {
                    return Err(Status::NOT_FOUND);
                }
                created_entry = true;
                let dir = self.borrow_dir()?;

                // Create directory if the directory protocol was explicitly specified.
                if flags.intersects(fio::Flags::PROTOCOL_DIRECTORY) {
                    let dir = dir.create_dir(name).map_err(fatfs_error_to_status)?;
                    self.add_directory(dir, name, closer)
                } else {
                    let file = dir.create_file(name).map_err(fatfs_error_to_status)?;
                    self.add_file(file, name, closer)
                }
            }
        };

        let mut data = self.data.borrow_mut();
        data.children.insert(InsensitiveString(name.to_owned()), node.downgrade());
        if created_entry {
            data.watchers.send_event(&mut SingleNameEventProducer::added(name));
            self.fs().mark_dirty();
        }

        Ok(node)
    }

    /// True if this directory has been deleted.
    pub(crate) fn is_deleted(&self) -> bool {
        self.data.borrow().deleted
    }

    /// Called to indicate a file or directory was removed from this directory.
    pub(crate) fn did_remove(&self, name: &str) {
        self.data.borrow_mut().watchers.send_event(&mut SingleNameEventProducer::removed(name));
    }

    /// Called to indicate a file or directory was added to this directory.
    pub(crate) fn did_add(&self, name: &str) {
        self.data.borrow_mut().watchers.send_event(&mut SingleNameEventProducer::added(name));
    }

    /// Do a simple rename of the file, without unlinking dst.
    /// This assumes that either "dst" and "src" are the same file, or that "dst" has already been
    /// unlinked.
    fn rename_internal(
        &self,
        src_dir: &Arc<FatDirectory>,
        src_name: &str,
        dst_name: &str,
        existing: ExistingRef<'_, '_>,
    ) -> Result<(), Status> {
        // We're ready to go: remove the entry from the source cache, and close the reference to
        // the underlying file (this ensures all pending writes, etc. have been flushed).
        // We remove the entry with rename() below. Since we are single-threaded, nothing will
        // put the entry back in the cache. After renaming we also re-attach the entry to its
        // parent.

        // Do the rename.
        let src_fatfs_dir = src_dir.borrow_dir()?;
        let dst_fatfs_dir = self.borrow_dir()?;

        match existing {
            ExistingRef::None => {
                src_fatfs_dir
                    .rename(src_name, &dst_fatfs_dir, dst_name)
                    .map_err(fatfs_error_to_status)?;
            }
            ExistingRef::File(file) => {
                src_fatfs_dir
                    .rename_over_file(src_name, &dst_fatfs_dir, dst_name, file)
                    .map_err(fatfs_error_to_status)?;
            }
            ExistingRef::Dir(dir) => {
                src_fatfs_dir
                    .rename_over_dir(src_name, &dst_fatfs_dir, dst_name, dir)
                    .map_err(fatfs_error_to_status)?;
            }
        }

        src_dir.did_remove(src_name);
        self.did_add(dst_name);

        src_dir.fs().mark_dirty();

        // TODO: do the watcher event for existing.

        Ok(())
    }

    // Helper that adds a directory to the FatFilesystem
    fn add_directory(self: &Arc<Self>, dir: Dir<'_>, name: &str, closer: &mut Closer) -> FatNode {
        // SAFETY: The `dir` reference is from the same filesystem (`self.fs()`) as the one
        // we associate it with.
        let dir_ref = unsafe { FatfsDirRef::from(dir, self.fs().clone()) };
        closer.add(FatNode::Dir(FatDirectory::new(dir_ref, Some(self.clone()), name.to_owned())))
    }

    // Helper that adds a file to the FatFilesystem
    fn add_file(self: &Arc<Self>, file: File<'_>, name: &str, closer: &mut Closer) -> FatNode {
        // SAFETY: The `file` reference is from the same filesystem (`self.fs()`) as the one
        // we associate it with.
        let file_ref = unsafe { FatfsFileRef::from(file, self.fs().clone()) };
        closer.add(FatNode::File(FatFile::new(file_ref, self.clone(), name.to_owned())))
    }
}

impl Node for FatDirectory {
    /// Flush to disk and invalidate the reference that's contained within this FatDir.
    /// Any operations on the directory will return Status::BAD_HANDLE until it is re-attached.
    fn detach(&self) {
        // This causes a flush to disk when the underlying fatfs Dir is dropped.
        self.dir.clear();
    }

    /// Re-open the underlying `FatfsDirRef` this directory represents, and attach to the given
    /// parent.
    fn attach(&self, new_parent: Arc<FatDirectory>, name: &str) -> Result<(), Status> {
        let mut data = self.data.borrow_mut();
        data.name = name.to_owned();

        // Safe because we have a reference to the FatFilesystem.
        self.dir.maybe_reopen(Some(&new_parent), name)?;

        assert!(data.parent.replace(new_parent).is_some());
        Ok(())
    }

    fn did_delete(&self) {
        let mut data = self.data.borrow_mut();
        data.parent.take();
        data.watchers.send_event(&mut SingleNameEventProducer::deleted());
        data.deleted = true;
    }

    fn open_ref(&self) -> Result<(), Status> {
        let data = self.data.borrow();
        self.dir.open(data.parent.as_ref(), &data.name)
    }

    fn shut_down(&self) -> Result<(), Status> {
        self.dir.clear();
        let mut data = self.data.borrow_mut();
        for (_, child) in data.children.drain() {
            if let Some(child) = child.upgrade() {
                child.shut_down()?;
            }
        }
        Ok(())
    }

    fn flush_dir_entry(&self) -> Result<(), Status> {
        if let Some(ref mut dir) = self.borrow_dir_mut() {
            dir.flush_dir_entry().map_err(fatfs_error_to_status)?;
        }
        Ok(())
    }

    fn close_ref(&self) {
        self.dir.close();
    }
}

impl Debug for FatDirectory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FatDirectory")
            .field("parent", &self.data.borrow().parent)
            .finish_non_exhaustive()
    }
}

impl MutableDirectory for FatDirectory {
    async fn unlink(self: Arc<Self>, name: &str, must_be_directory: bool) -> Result<(), Status> {
        let parent = self.borrow_dir()?;
        let mut existing_node = self.cache_get(name);
        let mut done = false;
        match existing_node {
            Some(FatNode::File(ref mut file)) => {
                if must_be_directory {
                    return Err(Status::NOT_DIR);
                }
                if let Some(mut file) = file.borrow_file_mut() {
                    parent.unlink_file(&mut *file).map_err(fatfs_error_to_status)?;
                    done = true;
                }
            }
            Some(FatNode::Dir(ref mut dir)) => {
                if let Some(mut dir) = dir.borrow_dir_mut() {
                    parent.unlink_dir(&mut *dir).map_err(fatfs_error_to_status)?;
                    done = true;
                }
            }
            None => {
                if must_be_directory {
                    let entry = self.find_child(name)?;
                    if !entry.ok_or(Status::NOT_FOUND)?.is_dir() {
                        return Err(Status::NOT_DIR);
                    }
                }
            }
        }
        if !done {
            parent.remove(name).map_err(fatfs_error_to_status)?;
        }
        if existing_node.is_some() {
            self.cache_remove(name);
        }
        match existing_node {
            Some(FatNode::File(ref mut file)) => file.did_delete(),
            Some(FatNode::Dir(ref mut dir)) => dir.did_delete(),
            None => {}
        }

        self.fs().mark_dirty();
        self.data.borrow_mut().watchers.send_event(&mut SingleNameEventProducer::removed(name));
        Ok(())
    }

    async fn update_attributes(
        &self,
        attributes: fio::MutableNodeAttributes,
    ) -> Result<(), Status> {
        const SUPPORTED_MUTABLE_ATTRIBUTES: fio::NodeAttributesQuery =
            fio::NodeAttributesQuery::CREATION_TIME
                .union(fio::NodeAttributesQuery::MODIFICATION_TIME);

        if !SUPPORTED_MUTABLE_ATTRIBUTES
            .contains(vfs::common::mutable_node_attributes_to_query(&attributes))
        {
            return Err(Status::NOT_SUPPORTED);
        }

        let mut dir = self.borrow_dir_mut().ok_or(Status::BAD_HANDLE)?;
        if let Some(creation_time) = attributes.creation_time {
            dir.set_created(unix_to_dos_time(creation_time));
        }
        if let Some(modification_time) = attributes.modification_time {
            dir.set_modified(unix_to_dos_time(modification_time));
        }

        self.fs().mark_dirty();
        Ok(())
    }

    async fn sync(&self) -> Result<(), Status> {
        // TODO(https://fxbug.dev/42132904): Support sync on root of fatfs volume.
        Ok(())
    }

    fn rename(
        self: Arc<Self>,
        src_dir: Arc<dyn MutableDirectory>,
        src_path: Path,
        dst_path: Path,
    ) -> BoxFuture<'static, Result<(), Status>> {
        Box::pin(async move {
            let src_dir =
                src_dir.into_any().downcast::<FatDirectory>().map_err(|_| Status::INVALID_ARGS)?;
            if self.is_deleted() {
                // Can't rename into a deleted folder.
                return Err(Status::NOT_FOUND);
            }

            let src_name = src_path.peek().unwrap();
            validate_filename(src_name).map_err(fatfs_error_to_status)?;
            let dst_name = dst_path.peek().unwrap();
            validate_filename(dst_name).map_err(fatfs_error_to_status)?;

            let mut closer = Closer::new();

            // Figure out if src is a directory.
            let entry = src_dir.find_child(src_name)?;
            if entry.is_none() {
                // No such src (if we don't return NOT_FOUND here, fatfs will return it when we
                // call rename() later).
                return Err(Status::NOT_FOUND);
            }
            let src_is_dir = entry.unwrap().is_dir();
            if (dst_path.is_dir() || src_path.is_dir()) && !src_is_dir {
                // The caller wanted a directory (src or dst), but src is not a directory. This is
                // an error.
                return Err(Status::NOT_DIR);
            }

            // Renaming a file to itself is trivial, but we do it after we've checked that the file
            // exists and that src and dst have the same type.
            if Arc::ptr_eq(&src_dir, &self)
                && (&src_name as &dyn InsensitiveStringRef)
                    == (&dst_name as &dyn InsensitiveStringRef)
            {
                if src_name != dst_name {
                    // Cases don't match - we don't unlink, but we still need to fix the file's LFN.
                    return self.rename_internal(&src_dir, src_name, dst_name, ExistingRef::None);
                }
                return Ok(());
            }

            // It's not legal to move a directory into itself or any child of itself.
            if let Some(src_node) = src_dir.cache_get(src_name) {
                if let FatNode::Dir(dir) = &src_node {
                    if Arc::ptr_eq(&dir, &self) {
                        return Err(Status::INVALID_ARGS);
                    }
                    // Walk the parents of the destination and make sure it doesn't match the source.
                    let mut dest = self.clone();
                    loop {
                        let next_dir = if let Some(parent) = &dest.data.borrow().parent {
                            if Arc::ptr_eq(&dir, parent) {
                                return Err(Status::INVALID_ARGS);
                            }
                            parent.clone()
                        } else {
                            break;
                        };
                        dest = next_dir;
                    }
                }
                src_node.flush_dir_entry()?;
            }

            let mut existing_node = self.cache_get(dst_name);
            let remove_from_cache = existing_node.is_some();
            let mut dir;
            let mut file;
            let mut borrowed_dir;
            let mut borrowed_file;
            let existing = match existing_node {
                None => {
                    self.open_ref()?;
                    closer.add(FatNode::Dir(self.clone()));
                    match self.find_child(dst_name)? {
                        Some(ref dir_entry) => {
                            if dir_entry.is_dir() {
                                dir = Some(dir_entry.to_dir());
                                ExistingRef::Dir(dir.as_mut().unwrap())
                            } else {
                                file = Some(dir_entry.to_file());
                                ExistingRef::File(file.as_mut().unwrap())
                            }
                        }
                        None => ExistingRef::None,
                    }
                }
                Some(ref mut node) => {
                    node.open_ref()?;
                    closer.add(node.clone());
                    match node {
                        FatNode::Dir(node_dir) => {
                            // Within `rename_internal` we will attempt to borrow the source and
                            // destination directories. This can't be the destination directory, but
                            // we must check that the directory here is not the same as the source
                            // directory.
                            if Arc::ptr_eq(node_dir, &src_dir) {
                                return Err(Status::INVALID_ARGS);
                            }
                            borrowed_dir = node_dir.borrow_dir_mut().unwrap();
                            ExistingRef::Dir(&mut *borrowed_dir)
                        }
                        FatNode::File(node_file) => {
                            borrowed_file = node_file.borrow_file_mut().unwrap();
                            ExistingRef::File(&mut *borrowed_file)
                        }
                    }
                }
            };

            match existing {
                ExistingRef::File(_) => {
                    if src_is_dir {
                        return Err(Status::NOT_DIR);
                    }
                }
                ExistingRef::Dir(_) => {
                    if !src_is_dir {
                        return Err(Status::NOT_FILE);
                    }
                }
                ExistingRef::None => {}
            }

            self.rename_internal(&src_dir, src_name, dst_name, existing)?;

            if remove_from_cache {
                self.cache_remove(dst_name).unwrap().did_delete();
            }

            // We succeeded in renaming, so now move the nodes around.
            if let Some(node) = src_dir.remove_child(src_name) {
                self.add_child(dst_name.to_owned(), node).unwrap_or_else(|e| {
                    panic!("Rename failed, but fatfs says it didn't? - {:?}", e)
                });
            }

            Ok(())
        })
    }
}

impl DirectoryEntry for FatDirectory {
    fn open_entry(self: Arc<Self>, request: OpenRequest<'_>) -> Result<(), Status> {
        request.open_dir(self)
    }
}

impl GetEntryInfo for FatDirectory {
    fn entry_info(&self) -> EntryInfo {
        EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::Directory)
    }
}

impl vfs::node::Node for FatDirectory {
    async fn get_attributes(
        &self,
        requested_attributes: fio::NodeAttributesQuery,
    ) -> Result<fio::NodeAttributes2, Status> {
        let dir = self.borrow_dir()?;

        let creation_time = dos_to_unix_time(dir.created());
        let modification_time = dos_to_unix_time(dir.modified());
        let access_time = dos_date_to_unix_time(dir.accessed());

        Ok(attributes!(
            requested_attributes,
            Mutable {
                creation_time: creation_time,
                modification_time: modification_time,
                access_time: access_time
            },
            Immutable {
                protocols: fio::NodeProtocolKinds::DIRECTORY,
                abilities: fio::Operations::GET_ATTRIBUTES
                    | fio::Operations::UPDATE_ATTRIBUTES
                    | fio::Operations::ENUMERATE
                    | fio::Operations::TRAVERSE
                    | fio::Operations::MODIFY_DIRECTORY,
                link_count: 1, // FAT does not support hard links, so there is always 1 "link".
            }
        ))
    }

    fn close(self: Arc<Self>) {
        self.close_ref();
    }

    fn query_filesystem(&self) -> Result<fio::FilesystemInfo, Status> {
        self.fs().query_filesystem()
    }

    fn will_clone(&self) {
        self.open_ref().unwrap();
    }
}

impl Directory for FatDirectory {
    fn deprecated_open(
        self: Arc<Self>,
        _scope: ExecutionScope,
        flags: fio::OpenFlags,
        path: Path,
        server_end: ServerEnd<fio::NodeMarker>,
    ) {
        let mut closer = Closer::new();

        flags.to_object_request(server_end).handle(|object_request| {
            match self.lookup(flags, path, &mut closer)? {
                FatNode::Dir(entry) => {
                    let () = entry.open_ref().expect("entry should already be open");
                    object_request.take().create_connection_sync::<MutableConnection<_>, _>(
                        self.fs().scope().clone(),
                        entry,
                        flags,
                    );
                    Ok(())
                }
                FatNode::File(entry) => {
                    let () = entry.open_ref()?;
                    object_request.take().create_connection_sync::<FidlIoConnection<_>, _>(
                        self.fs().scope().clone(),
                        entry,
                        flags,
                    );
                    Ok(())
                }
            }
        });
    }

    fn open(
        self: Arc<Self>,
        _scope: ExecutionScope,
        path: Path,
        flags: fio::Flags,
        object_request: ObjectRequestRef<'_>,
    ) -> Result<(), Status> {
        let mut closer = Closer::new();

        match self.lookup_with_open3_flags(flags, path, &mut closer)? {
            FatNode::Dir(entry) => {
                let () = entry.open_ref()?;
                object_request.take().create_connection_sync::<MutableConnection<_>, _>(
                    self.fs().scope().clone(),
                    entry,
                    flags,
                );
                Ok(())
            }
            FatNode::File(entry) => {
                let () = entry.open_ref()?;
                object_request.take().create_connection_sync::<FidlIoConnection<_>, _>(
                    self.fs().scope().clone(),
                    entry,
                    flags,
                );
                Ok(())
            }
        }
    }

    async fn read_dirents(
        &self,
        pos: &TraversalPosition,
        sink: Box<dyn dirents_sink::Sink>,
    ) -> Result<(TraversalPosition, Box<dyn dirents_sink::Sealed>), Status> {
        if self.is_deleted() {
            return Ok((TraversalPosition::End, sink.seal()));
        }

        let dir = self.borrow_dir()?;

        if let TraversalPosition::End = pos {
            return Ok((TraversalPosition::End, sink.seal()));
        }

        let filter = |name: &str| match pos {
            TraversalPosition::Start => true,
            TraversalPosition::Name(next_name) => name >= next_name.as_str(),
            _ => false,
        };

        // Get all the entries in this directory.
        let mut entries: Vec<_> = dir
            .iter()
            .filter_map(|maybe_entry| {
                maybe_entry
                    .map(|entry| {
                        let name = entry.file_name();
                        if &name == ".." || !filter(&name) {
                            None
                        } else {
                            let entry_type = if entry.is_dir() {
                                fio::DirentType::Directory
                            } else {
                                fio::DirentType::File
                            };
                            Some((name, EntryInfo::new(fio::INO_UNKNOWN, entry_type)))
                        }
                    })
                    .transpose()
            })
            .collect::<std::io::Result<Vec<_>>>()
            .map_err(fatfs_error_to_status)?;

        // If it's the root directory, we need to synthesize a "." entry if appropriate.
        if self.data.borrow().parent.is_none() && filter(".") {
            entries.push((
                ".".to_owned(),
                EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::Directory),
            ));
        }

        // Sort them by alphabetical order.
        entries.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

        // Iterate through the entries, adding them one by one to the sink.
        let mut cur_sink = sink;
        for (name, info) in entries.into_iter() {
            let result = cur_sink.append(&info, &name);

            match result {
                AppendResult::Ok(new_sink) => cur_sink = new_sink,
                AppendResult::Sealed(sealed) => {
                    return Ok((TraversalPosition::Name(name), sealed));
                }
            }
        }

        return Ok((TraversalPosition::End, cur_sink.seal()));
    }

    fn register_watcher(
        self: Arc<Self>,
        scope: ExecutionScope,
        mask: fio::WatchMask,
        watcher: DirectoryWatcher,
    ) -> Result<(), Status> {
        let mut data = self.data.borrow_mut();
        let is_deleted = data.deleted;
        let is_root = data.parent.is_none();
        let controller = data.watchers.add(scope, self.clone(), mask, watcher);
        if mask.contains(fio::WatchMask::EXISTING) && !is_deleted {
            let entries = {
                let dir = self.borrow_dir()?;
                let synthesized_dot = if is_root {
                    // We need to synthesize a "." entry.
                    Some(Ok(".".to_owned()))
                } else {
                    None
                };
                synthesized_dot
                    .into_iter()
                    .chain(dir.iter().filter_map(|maybe_entry| {
                        maybe_entry
                            .map(|entry| {
                                let name = entry.file_name();
                                if &name == ".." { None } else { Some(name) }
                            })
                            .transpose()
                    }))
                    .collect::<std::io::Result<Vec<String>>>()
                    .map_err(fatfs_error_to_status)?
            };
            controller.send_event(&mut StaticVecEventProducer::existing(entries));
        }
        controller.send_event(&mut SingleNameEventProducer::idle());
        Ok(())
    }

    fn unregister_watcher(self: Arc<Self>, key: usize) {
        self.data.borrow_mut().watchers.remove(key);
    }
}

#[cfg(test)]
mod tests {
    // We only test things here that aren't covered by fs_tests.
    use super::*;
    use crate::tests::{TestDiskContents, TestFatDisk};
    use assert_matches::assert_matches;
    use futures::TryStreamExt;
    use scopeguard::defer;
    use vfs::ObjectRequest;
    use vfs::directory::dirents_sink::{Sealed, Sink};
    use vfs::node::Node as _;

    const TEST_DISK_SIZE: u64 = 2048 << 10; // 2048K

    #[fuchsia::test(allow_stalls = false)]
    async fn test_link_fails() {
        let disk = TestFatDisk::empty_disk(TEST_DISK_SIZE);
        let structure = TestDiskContents::dir().add_child("test_file", "test file contents".into());
        structure.create(&disk.root_dir());

        let fs = disk.into_fatfs();
        let dir = fs.get_fatfs_root();
        dir.open_ref().expect("open_ref failed");
        defer! { dir.close_ref() }
        assert_eq!(
            dir.clone().link("test2".to_owned(), dir.clone(), "test3").await.unwrap_err(),
            Status::NOT_SUPPORTED
        );
    }

    #[derive(Clone)]
    struct DummySink {
        max_size: usize,
        entries: Vec<(String, EntryInfo)>,
        sealed: bool,
    }

    impl DummySink {
        pub fn new(max_size: usize) -> Self {
            DummySink { max_size, entries: Vec::with_capacity(max_size), sealed: false }
        }

        fn from_sealed(sealed: Box<dyn dirents_sink::Sealed>) -> Box<DummySink> {
            sealed.into()
        }
    }

    impl From<Box<dyn dirents_sink::Sealed>> for Box<DummySink> {
        fn from(sealed: Box<dyn dirents_sink::Sealed>) -> Self {
            sealed.open().downcast::<DummySink>().unwrap()
        }
    }

    impl Sink for DummySink {
        fn append(mut self: Box<Self>, entry: &EntryInfo, name: &str) -> AppendResult {
            assert!(!self.sealed);
            if self.entries.len() == self.max_size {
                AppendResult::Sealed(self.seal())
            } else {
                self.entries.push((name.to_owned(), entry.clone()));
                AppendResult::Ok(self)
            }
        }

        fn seal(mut self: Box<Self>) -> Box<dyn Sealed> {
            self.sealed = true;
            self
        }
    }

    impl Sealed for DummySink {
        fn open(self: Box<Self>) -> Box<dyn std::any::Any> {
            self
        }
    }

    #[fuchsia::test]
    /// Test with a sink that can't handle the entire directory in one go.
    fn test_read_dirents_small_sink() {
        let disk = TestFatDisk::empty_disk(TEST_DISK_SIZE);
        let structure = TestDiskContents::dir()
            .add_child("test_file", "test file contents".into())
            .add_child("aaa", "this file is first".into())
            .add_child("qwerty", "hello".into())
            .add_child("directory", TestDiskContents::dir().add_child("a", "test".into()));
        structure.create(&disk.root_dir());

        let fs = disk.into_fatfs();
        let dir = fs.get_fatfs_root();

        dir.open_ref().expect("open_ref failed");
        defer! { dir.close_ref() }

        let (pos, sealed) = futures::executor::block_on(
            dir.clone().read_dirents(&TraversalPosition::Start, Box::new(DummySink::new(4))),
        )
        .expect("read_dirents failed");
        assert_eq!(
            DummySink::from_sealed(sealed).entries,
            vec![
                (".".to_owned(), EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::Directory)),
                ("aaa".to_owned(), EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::File)),
                (
                    "directory".to_owned(),
                    EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::Directory)
                ),
                ("qwerty".to_owned(), EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::File)),
            ]
        );

        // Read the next two entries.
        let (_, sealed) =
            futures::executor::block_on(dir.read_dirents(&pos, Box::new(DummySink::new(4))))
                .expect("read_dirents failed");
        assert_eq!(
            DummySink::from_sealed(sealed).entries,
            vec![("test_file".to_owned(), EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::File)),]
        );
    }

    #[fuchsia::test]
    /// Test with a sink that can hold everything.
    fn test_read_dirents_big_sink() {
        let disk = TestFatDisk::empty_disk(TEST_DISK_SIZE);
        let structure = TestDiskContents::dir()
            .add_child("test_file", "test file contents".into())
            .add_child("aaa", "this file is first".into())
            .add_child("qwerty", "hello".into())
            .add_child("directory", TestDiskContents::dir().add_child("a", "test".into()));
        structure.create(&disk.root_dir());

        let fs = disk.into_fatfs();
        let dir = fs.get_fatfs_root();

        dir.open_ref().expect("open_ref failed");
        defer! { dir.close_ref() }

        let (_, sealed) = futures::executor::block_on(
            dir.read_dirents(&TraversalPosition::Start, Box::new(DummySink::new(30))),
        )
        .expect("read_dirents failed");
        assert_eq!(
            DummySink::from_sealed(sealed).entries,
            vec![
                (".".to_owned(), EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::Directory)),
                ("aaa".to_owned(), EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::File)),
                (
                    "directory".to_owned(),
                    EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::Directory)
                ),
                ("qwerty".to_owned(), EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::File)),
                ("test_file".to_owned(), EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::File)),
            ]
        );
    }

    #[fuchsia::test]
    fn test_read_dirents_with_entry_that_sorts_before_dot() {
        let disk = TestFatDisk::empty_disk(TEST_DISK_SIZE);
        let structure = TestDiskContents::dir().add_child("!", "!".into());
        structure.create(&disk.root_dir());

        let fs = disk.into_fatfs();
        let dir = fs.get_fatfs_root();

        dir.open_ref().expect("open_ref failed");
        defer! { dir.close_ref() }

        let (pos, sealed) = futures::executor::block_on(
            dir.clone().read_dirents(&TraversalPosition::Start, Box::new(DummySink::new(1))),
        )
        .expect("read_dirents failed");
        assert_eq!(
            DummySink::from_sealed(sealed).entries,
            vec![("!".to_owned(), EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::File))]
        );

        let (_, sealed) =
            futures::executor::block_on(dir.read_dirents(&pos, Box::new(DummySink::new(1))))
                .expect("read_dirents failed");
        assert_eq!(
            DummySink::from_sealed(sealed).entries,
            vec![(".".to_owned(), EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::Directory)),]
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_deprecated_reopen_root() {
        let disk = TestFatDisk::empty_disk(TEST_DISK_SIZE);
        let structure = TestDiskContents::dir().add_child("test", "Hello".into());
        structure.create(&disk.root_dir());

        let fs = disk.into_fatfs();
        let dir = fs.get_root().expect("get_root OK");

        let proxy = vfs::directory::serve_read_only(dir.clone(), ExecutionScope::new());
        proxy
            .close()
            .await
            .expect("Send request OK")
            .map_err(Status::err_from_raw)
            .expect("First close OK");

        let proxy = vfs::directory::serve_read_only(dir.clone(), ExecutionScope::new());
        proxy
            .close()
            .await
            .expect("Send request OK")
            .map_err(Status::err_from_raw)
            .expect("Second close OK");
        dir.close();
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_reopen_root() {
        let disk = TestFatDisk::empty_disk(TEST_DISK_SIZE);
        let structure = TestDiskContents::dir().add_child("test", "Hello".into());
        structure.create(&disk.root_dir());

        let fs = disk.into_fatfs();
        let root = fs.get_root().expect("get_root failed");

        // Open and close root.
        let proxy = vfs::directory::serve_read_only(root.clone(), ExecutionScope::new());
        proxy
            .close()
            .await
            .expect("FIDL call failed")
            .map_err(Status::err_from_raw)
            .expect("First close failed");

        // Re-open and close root at "test".
        let proxy = vfs::serve_directory(
            root.clone(),
            Path::validate_and_split("test").unwrap(),
            ExecutionScope::new(),
            fio::PERM_READABLE,
        );
        proxy
            .close()
            .await
            .expect("FIDL call failed")
            .map_err(Status::err_from_raw)
            .expect("Second close failed");

        root.close();
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_open_already_exists() {
        let disk = TestFatDisk::empty_disk(TEST_DISK_SIZE);
        let structure = TestDiskContents::dir().add_child("test", "Hello".into());
        structure.create(&disk.root_dir());

        let fs = disk.into_fatfs();
        let root = fs.get_root().expect("get_root failed");

        let scope = ExecutionScope::new();
        let (proxy, server_end) = fidl::endpoints::create_proxy::<fio::NodeMarker>();
        let flags = fio::PERM_READABLE
            | fio::Flags::FLAG_MUST_CREATE
            | fio::Flags::FLAG_SEND_REPRESENTATION;
        ObjectRequest::new(flags, &fio::Options::default(), server_end.into()).handle(|request| {
            root.clone().open(
                scope.clone(),
                Path::validate_and_split("test").unwrap(),
                flags,
                request,
            )
        });

        let event =
            proxy.take_event_stream().try_next().await.expect_err("open passed unexpectedly");

        assert_matches!(
            event,
            fidl::Error::ClientChannelClosed { epitaph, .. } if epitaph == Status::ALREADY_EXISTS
        );

        root.close();
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_update_attributes_directory() {
        let disk = TestFatDisk::empty_disk(TEST_DISK_SIZE);
        let structure = TestDiskContents::dir().add_child("test", "Hello".into());
        structure.create(&disk.root_dir());

        let fs = disk.into_fatfs();
        let root = fs.get_root().expect("get_root failed");
        let proxy = vfs::directory::serve(
            root.clone(),
            ExecutionScope::new(),
            fio::PERM_READABLE | fio::PERM_WRITABLE,
        );

        let mut new_attrs = fio::MutableNodeAttributes {
            creation_time: Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::SystemTime::UNIX_EPOCH)
                    .expect("SystemTime before UNIX EPOCH")
                    .as_nanos()
                    .try_into()
                    .unwrap(),
            ),
            ..Default::default()
        };
        proxy
            .update_attributes(&new_attrs)
            .await
            .expect("FIDL call failed")
            .map_err(Status::err_from_raw)
            .expect("update attributes failed");

        new_attrs.mode = Some(123);
        let status = proxy
            .update_attributes(&new_attrs)
            .await
            .expect("FIDL call failed")
            .map_err(Status::err_from_raw)
            .expect_err("update unsupported attributes passed unexpectedly");
        assert_eq!(status, Status::NOT_SUPPORTED);
        root.close();
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_update_attributes_file() {
        let disk = TestFatDisk::empty_disk(TEST_DISK_SIZE);
        let structure = TestDiskContents::dir().add_child("test_file", "Hello".into());
        structure.create(&disk.root_dir());

        let fs = disk.into_fatfs();
        let root = fs.get_root().expect("get_root failed");
        let proxy = vfs::serve_file(
            root.clone(),
            Path::validate_and_split("test_file").unwrap(),
            ExecutionScope::new(),
            fio::PERM_WRITABLE,
        );

        let mut new_attrs = fio::MutableNodeAttributes {
            creation_time: Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::SystemTime::UNIX_EPOCH)
                    .expect("SystemTime before UNIX EPOCH")
                    .as_nanos()
                    .try_into()
                    .unwrap(),
            ),
            ..Default::default()
        };
        proxy
            .update_attributes(&new_attrs)
            .await
            .expect("FIDL call failed")
            .map_err(Status::err_from_raw)
            .expect("update attributes failed");

        new_attrs.mode = Some(123);
        let status = proxy
            .update_attributes(&new_attrs)
            .await
            .expect("FIDL call failed")
            .map_err(Status::err_from_raw)
            .expect_err("update unsupported attributes passed unexpectedly");
        assert_eq!(status, Status::NOT_SUPPORTED);
        root.close();
    }
}
