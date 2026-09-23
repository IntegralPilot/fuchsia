// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::security;
use crate::task::{CurrentTask, Kernel};
use crate::vfs::fs_args::MountParams;
use crate::vfs::fs_node_cache::FsNodeCache;
use crate::vfs::{
    DirEntry, DirEntryHandle, FsNode, FsNodeFlags, FsNodeHandle, FsNodeInfo, FsNodeOps, FsStr,
    FsString, RenameContext,
};
use flyweights::FlyByteStr;
use linked_hash_map::LinkedHashMap;
use ref_cast::RefCast;
use smallvec::SmallVec;
use starnix_crypt::CryptService;
use starnix_logging::log_warn;
use starnix_sync::{
    DynamicLockDepMutex, FileSystemEntriesLock, FileSystemPermanentLock, FsRename,
    FsRenameRecursive, FuseFsRenameLevel, LockDepMutex,
};
use starnix_uapi::arc_key::ArcKey;
use starnix_uapi::as_any::AsAny;
use starnix_uapi::auth::FsCred;
use starnix_uapi::device_id::DeviceId;
use starnix_uapi::errors::Errno;
use starnix_uapi::file_mode::mode;
use starnix_uapi::mount_flags::{AtomicFileSystemFlags, FileSystemFlags};
use starnix_uapi::{error, ino_t, statfs};
use std::collections::HashSet;
use std::ops::Range;
use std::sync::atomic::Ordering;
use std::sync::{Arc, OnceLock, Weak};

#[derive(Debug, Default)]
pub struct FileSystemRenameToken {}

/// The type of the filesystem for LockDep purposes.
///
/// `Normal` filesystems use standard lock levels.
/// `Recursive` filesystems (like OverlayFS) use lock levels that precede normal ones,
/// allowing them to lock the underlying filesystem without violating the hierarchy.
/// `Fuse` filesystems do blocking calls while holding locks and require specific lock
/// ordering because of this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsLockDepType {
    Normal,
    Recursive,
    Fuse,
}

/// A file system that can be mounted in a namespace.
pub struct FileSystem {
    pub kernel: Weak<Kernel>,
    root: OnceLock<DirEntryHandle>,
    ops: Box<dyn FileSystemOps>,

    /// The options specified when mounting the filesystem. Saved here for display in
    /// /proc/[pid]/mountinfo.
    pub options: FileSystemOptions,

    /// The device ID of this filesystem. Returned in the st_dev field when stating an inode in
    /// this filesystem.
    pub dev_id: DeviceId,

    /// A file-system global mutex to serialize rename operations.
    ///
    /// This mutex is useful because the invariants enforced during a rename
    /// operation involve many DirEntry objects. In the future, we might be
    /// able to remove this mutex, but we will need to think carefully about
    /// how rename operations can interleave.
    ///
    /// See DirEntry::rename.
    pub rename_mutex: DynamicLockDepMutex<FileSystemRenameToken>,

    /// The FsNode cache for this file system.
    ///
    /// When two directory entries are hard links to the same underlying inode,
    /// this cache lets us re-use the same FsNode object for both directory
    /// entries.
    ///
    /// Rather than calling FsNode::new directly, file systems should call
    /// FileSystem::get_or_create_node to see if the FsNode already exists in
    /// the cache.
    node_cache: Arc<FsNodeCache>,

    /// DirEntryHandle cache for the filesystem. Holds strong references to DirEntry objects. For
    /// filesystems with permanent entries, this will hold a strong reference to every node to make
    /// sure it doesn't get freed without being explicitly unlinked. Otherwise, entries are
    /// maintained in an LRU cache.
    dcache: DirEntryCache,

    /// Holds security state for this file system, which is created and used by the Linux Security
    /// Modules subsystem hooks.
    pub security_state: security::FileSystemState,
}

impl std::fmt::Debug for FileSystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FileSystem")
    }
}

#[derive(Debug, Default)]
pub struct FileSystemOptions {
    /// The source string passed as the first argument to mount(), e.g. a block device.
    pub source: FlyByteStr,
    /// Flags kept per-superblock.
    pub flags: AtomicFileSystemFlags,
    /// Filesystem options passed as the last argument to mount().
    pub params: MountParams,
}

impl Clone for FileSystemOptions {
    fn clone(&self) -> Self {
        Self {
            source: self.source.clone(),
            flags: self.flags.load(Ordering::Relaxed).into(),
            params: self.params.clone(),
        }
    }
}

impl FileSystemOptions {
    pub fn source_for_display(&self) -> &FsStr {
        if self.source.is_empty() {
            return "none".into();
        }
        self.source.as_ref()
    }
}

struct LruCache {
    capacity: usize,
    entries: LockDepMutex<LinkedHashMap<ArcKey<DirEntry>, ()>, FileSystemEntriesLock>,
}

enum DirEntryCache {
    Permanent(LockDepMutex<HashSet<ArcKey<DirEntry>>, FileSystemPermanentLock>),
    Lru(LruCache),
    Uncached,
}

/// Configuration for CacheMode::Cached.
pub struct CacheConfig {
    pub capacity: usize,
}

pub enum CacheMode {
    /// Entries are pemanent, instead of a cache of the backing storage. An example is tmpfs: the
    /// DirEntry tree *is* the backing storage, as opposed to ext4, which uses the DirEntry tree as
    /// a cache and removes unused nodes from it.
    Permanent,
    /// Entries are cached.
    Cached(CacheConfig),
    /// Entries are uncached. This can be appropriate in cases where it is difficult for the
    /// filesystem to keep the cache coherent: e.g. the /proc/<pid>/task directory.
    Uncached,
}

impl FileSystem {
    /// Create a new filesystem.
    pub fn new(
        kernel: &Kernel,
        cache_mode: CacheMode,
        ops: impl FileSystemOps,
        mut options: FileSystemOptions,
    ) -> Result<FileSystemHandle, Errno> {
        let uses_external_node_ids = ops.uses_external_node_ids();
        let node_cache = Arc::new(FsNodeCache::new(uses_external_node_ids));
        assert_eq!(ops.uses_external_node_ids(), node_cache.uses_external_node_ids());

        let mount_options = security::sb_eat_lsm_opts(&kernel, &mut options.params)?;
        let security_state = security::file_system_init_security(&mount_options, &ops)?;

        let fs_lockdep_type = ops.fs_lockdep_type();

        let file_system = Arc::new(FileSystem {
            kernel: kernel.weak_self.clone(),
            root: OnceLock::new(),
            ops: Box::new(ops),
            options,
            dev_id: kernel.device_registry.next_anonymous_dev_id(),
            rename_mutex: match fs_lockdep_type {
                FsLockDepType::Normal => {
                    DynamicLockDepMutex::new::<FsRename>(FileSystemRenameToken::default())
                }
                FsLockDepType::Recursive => {
                    DynamicLockDepMutex::new::<FsRenameRecursive>(FileSystemRenameToken::default())
                }
                FsLockDepType::Fuse => {
                    DynamicLockDepMutex::new::<FuseFsRenameLevel>(FileSystemRenameToken::default())
                }
            },
            node_cache,
            dcache: match cache_mode {
                CacheMode::Permanent => DirEntryCache::Permanent(Default::default()),
                CacheMode::Cached(CacheConfig { capacity }) => {
                    DirEntryCache::Lru(LruCache { capacity, entries: Default::default() })
                }
                CacheMode::Uncached => DirEntryCache::Uncached,
            },
            security_state,
        });

        // TODO: https://fxbug.dev/366405587 - Workaround to allow SELinux to note that this
        // `FileSystem` needs labeling, once a policy has been loaded.
        security::file_system_post_init_security(kernel, &file_system);

        Ok(file_system)
    }

    fn set_root(self: &FileSystemHandle, root: FsNodeHandle) {
        // No need to cache the root directory, it is owned by the filesystem.
        let root_dir = DirEntry::new_uncached(root, None, FsString::default());
        assert!(
            self.root.set(root_dir).is_ok(),
            "FileSystem::set_root can't be called more than once"
        );
    }

    pub fn has_permanent_entries(&self) -> bool {
        matches!(self.dcache, DirEntryCache::Permanent(_))
    }

    /// Returns whether this filesystem supports casefolded directory lookups.
    pub fn has_casefold_support(&self) -> bool {
        self.ops.has_casefold_support()
    }

    /// Returns the `FsLockDepType` of this filesystem, delegated from `FileSystemOps`.
    pub fn fs_lockdep_type(&self) -> FsLockDepType {
        self.ops.fs_lockdep_type()
    }

    /// The root directory entry of this file system.
    ///
    /// Panics if this file system does not have a root directory.
    pub fn root(&self) -> &DirEntryHandle {
        self.root.get().unwrap_or_else(|| panic!("FileSystem {} has no root", self.name()))
    }

    /// The root directory entry of this `FileSystem`, if it has one.
    pub fn maybe_root(&self) -> Option<&DirEntryHandle> {
        self.root.get()
    }

    pub fn get_or_create_node<F>(
        &self,
        node_key: ino_t,
        create_fn: F,
    ) -> Result<FsNodeHandle, Errno>
    where
        F: FnOnce() -> Result<FsNodeHandle, Errno>,
    {
        self.get_and_validate_or_create_node(node_key, |_| true, create_fn)
    }

    /// Get a node that is validated with the callback, or create an FsNode for
    /// this file system.
    ///
    /// If node_id is Some, then this function checks the node cache to
    /// determine whether this node is already open. If so, the function
    /// returns the existing FsNode if it passes the validation check. If no
    /// node exists, or a node does but fails the validation check, the function
    /// calls the given create_fn function to create the FsNode.
    ///
    /// If node_id is None, then this function assigns a new identifier number
    /// and calls the given create_fn function to create the FsNode with the
    /// assigned number.
    ///
    /// Returns Err only if create_fn returns Err.
    pub fn get_and_validate_or_create_node<V, C>(
        &self,
        node_key: ino_t,
        validate_fn: V,
        create_fn: C,
    ) -> Result<FsNodeHandle, Errno>
    where
        V: Fn(&FsNodeHandle) -> bool,
        C: FnOnce() -> Result<FsNodeHandle, Errno>,
    {
        self.node_cache.get_and_validate_or_create_node(node_key, validate_fn, create_fn)
    }

    /// File systems that produce their own IDs for nodes should invoke this
    /// function. The ones who leave to this object to assign the IDs should
    /// call |create_node_and_allocate_node_id|.
    pub fn create_node_with_flags(
        self: &Arc<Self>,
        ino: Option<ino_t>,
        ops: impl Into<Box<dyn FsNodeOps>>,
        info: FsNodeInfo,
        flags: FsNodeFlags,
    ) -> FsNodeHandle {
        let ino = ino.unwrap_or_else(|| self.allocate_ino());
        let node = FsNode::new_uncached(ino, ops, self, info, flags);
        self.node_cache.insert_node(&node);
        node
    }

    pub fn create_node(
        self: &Arc<Self>,
        ino: ino_t,
        ops: impl Into<Box<dyn FsNodeOps>>,
        info: FsNodeInfo,
    ) -> FsNodeHandle {
        self.create_node_with_flags(Some(ino), ops, info, FsNodeFlags::empty())
    }

    pub fn create_node_and_allocate_node_id(
        self: &Arc<Self>,
        ops: impl Into<Box<dyn FsNodeOps>>,
        info: FsNodeInfo,
    ) -> FsNodeHandle {
        self.create_node_with_flags(None, ops, info, FsNodeFlags::empty())
    }

    /// Create a node for a directory that has no parent.
    pub fn create_detached_node(
        self: &Arc<Self>,
        ino: ino_t,
        ops: impl Into<Box<dyn FsNodeOps>>,
        info: FsNodeInfo,
    ) -> FsNodeHandle {
        assert!(info.mode.is_dir());
        let node = FsNode::new_uncached(ino, ops, self, info, FsNodeFlags::empty());
        self.node_cache.insert_node(&node);
        node
    }

    /// Create a root node for the filesystem.
    ///
    /// This is a convenience function that creates a root node with the default
    /// directory mode and root credentials.
    pub fn create_root(self: &Arc<Self>, ino: ino_t, ops: impl Into<Box<dyn FsNodeOps>>) {
        let info = FsNodeInfo::new(mode!(IFDIR, 0o777), FsCred::root());
        self.create_root_with_info(ino, ops, info);
    }

    pub fn create_root_with_info(
        self: &Arc<Self>,
        ino: ino_t,
        ops: impl Into<Box<dyn FsNodeOps>>,
        info: FsNodeInfo,
    ) {
        let node = self.create_detached_node(ino, ops, info);
        self.set_root(node);
    }

    /// Remove the given FsNode from the node cache.
    ///
    /// Called from the Release trait of FsNode.
    pub fn remove_node(&self, node: &FsNode) {
        self.node_cache.remove_node(node);
    }

    pub fn allocate_ino(&self) -> ino_t {
        self.node_cache
            .allocate_ino()
            .expect("allocate_ino called on a filesystem that uses external node IDs")
    }

    /// Allocate a contiguous block of node ids.
    pub fn allocate_ino_range(&self, size: usize) -> Range<ino_t> {
        self.node_cache
            .allocate_ino_range(size)
            .expect("allocate_ino_range called on a filesystem that uses external node IDs")
    }

    /// Move |renamed| that is at |old_name| in |old_parent| to |new_name| in |new_parent|
    /// replacing |replaced|.
    /// If |replaced| exists and is a directory, this function must check that |renamed| is n
    /// directory and that |replaced| is empty.
    pub fn rename(
        &self,
        current_task: &CurrentTask,
        context: &mut RenameContext<'_>,
        old_name: &FsStr,
        new_name: &FsStr,
    ) -> Result<(), Errno> {
        self.ops.rename(self, current_task, context, old_name, new_name)
    }

    /// Exchanges the two nodes identified by `name1` and `name2` in the context.
    /// The parent directories and other metadata are contained within the `context`.
    pub fn exchange(
        &self,
        current_task: &CurrentTask,
        context: &mut RenameContext<'_>,
        name1: &FsStr,
        name2: &FsStr,
    ) -> Result<(), Errno> {
        self.ops.exchange(self, current_task, context, name1, name2)
    }

    /// Forces a FileSystem unmount.
    // TODO(https://fxbug.dev/394694891): kernel shutdown should ideally unmount FileSystems via
    // their drop impl, which should be triggered by Mount.unmount().
    pub fn force_unmount_ops(&self) {
        self.ops.unmount();
    }

    /// Returns the `statfs` for this filesystem.
    ///
    /// Each `FileSystemOps` impl is expected to override this to return the specific statfs for
    /// the filesystem.
    ///
    /// Returns `ENOSYS` if the `FileSystemOps` don't implement `stat`.
    pub fn statfs(&self, current_task: &CurrentTask) -> Result<statfs, Errno> {
        security::sb_statfs(current_task, &self)?;
        let mut stat = self.ops.statfs(self, current_task)?;
        if stat.f_frsize == 0 {
            stat.f_frsize = stat.f_bsize as i64;
        }
        Ok(stat)
    }

    pub fn sync(&self, current_task: &CurrentTask) -> Result<(), Errno> {
        self.ops.sync(self, current_task)
    }

    pub fn did_create_dir_entry(&self, entry: &DirEntryHandle) {
        match &self.dcache {
            DirEntryCache::Permanent(p) => {
                p.lock().insert(ArcKey(entry.clone()));
            }
            DirEntryCache::Lru(LruCache { entries, .. }) => {
                entries.lock().insert(ArcKey(entry.clone()), ());
            }
            DirEntryCache::Uncached => {}
        }
    }

    pub fn will_destroy_dir_entry(&self, entry: &DirEntryHandle) {
        match &self.dcache {
            DirEntryCache::Permanent(p) => {
                p.lock().remove(ArcKey::ref_cast(entry));
            }
            DirEntryCache::Lru(LruCache { entries, .. }) => {
                entries.lock().remove(ArcKey::ref_cast(entry));
            }
            DirEntryCache::Uncached => {}
        };
    }

    /// Informs the cache that the entry was used.
    pub fn did_access_dir_entry(&self, entry: &DirEntryHandle) {
        if let DirEntryCache::Lru(LruCache { entries, .. }) = &self.dcache {
            entries.lock().get_refresh(ArcKey::ref_cast(entry));
        }
    }

    /// Purges old entries from the cache. This is done as a separate step to avoid potential
    /// deadlocks that could occur if done at admission time (where locks might be held that are
    /// required when dropping old entries). This should be called after any new entries are
    /// admitted with no locks held that might be required for dropping entries.
    pub fn purge_old_entries(&self) {
        if let DirEntryCache::Lru(l) = &self.dcache {
            let mut purged = SmallVec::<[DirEntryHandle; 4]>::new();
            {
                let mut entries = l.entries.lock();
                while entries.len() > l.capacity {
                    purged.push(entries.pop_front().unwrap().0.0);
                }
            }
            // Entries will get dropped here whilst we're not holding a lock.
            std::mem::drop(purged);
        }
    }

    /// Returns the `FileSystem`'s `FileSystemOps` as a `&T`, or `None` if the downcast fails.
    pub fn downcast_ops<T: 'static>(&self) -> Option<&T> {
        self.ops.as_ref().as_any().downcast_ref()
    }

    pub fn name(&self) -> &'static FsStr {
        self.ops.name()
    }

    pub fn manages_timestamps(&self) -> bool {
        self.ops.manages_timestamps()
    }

    /// Returns the crypt service associated with this filesystem, if any. The crypt service
    /// implements the fuchsia.fxfs.Crypt protocol and maintains an internal structure that maps
    /// each encryption key id to the actual key.
    pub fn crypt_service(&self) -> Option<Arc<CryptService>> {
        self.ops.crypt_service()
    }

    /// Reconfigures the MountFlags associated with the filesystem with the specified `flags`.
    /// Filesystems may customize `FsNodeOps::update_flags()` to take action (e.g. flushing dirty
    /// files when transitioning from read-write to read-only), or to reject reconfiguration.
    pub fn update_flags(
        &self,
        current_task: &CurrentTask,
        flags: FileSystemFlags,
    ) -> Result<(), Errno> {
        self.ops.update_flags(self, current_task, flags)
    }

    pub fn sub_filesystems(&self) -> Vec<Arc<FileSystem>> {
        self.ops.sub_filesystems()
    }

    pub fn purge_dcache(&self) {
        if let DirEntryCache::Lru(l) = &self.dcache {
            let entries = std::mem::take(&mut *l.entries.lock());
            // Entries will get dropped here outside of the lock.
            std::mem::drop(entries);
        }
    }

    pub fn drop_backend_caches(&self) {
        if let Err(e) = self.ops.drop_caches(self) {
            log_warn!("drop_caches failed for filesystem {:?}: {:?}", self.name(), e);
        }
    }

    pub fn purge_all_entries(&self) {
        self.purge_dcache();
        self.drop_backend_caches();
    }
}

/// The filesystem-implementation-specific data for FileSystem.
pub trait FileSystemOps: AsAny + Send + Sync + 'static {
    /// Returns the `FsLockDepType` of this filesystem.
    ///
    /// Defaults to `FsLockDepType::Normal`. Filesystems that can be stacked (like OverlayFS)
    /// should override this to return `FsLockDepType::Recursive`.
    fn fs_lockdep_type(&self) -> FsLockDepType {
        FsLockDepType::Normal
    }

    /// Return information about this filesystem.
    ///
    /// A typical implementation looks like this:
    /// ```
    /// Ok(statfs::default(FILE_SYSTEM_MAGIC))
    /// ```
    /// or, if the filesystem wants to customize fields:
    /// ```
    /// Ok(statfs {
    ///     f_blocks: self.blocks,
    ///     ..statfs::default(FILE_SYSTEM_MAGIC)
    /// })
    /// ```
    fn statfs(&self, _fs: &FileSystem, _current_task: &CurrentTask) -> Result<statfs, Errno>;

    /// Reconfigure the filesystem with the given flags.
    ///
    /// This is called during a remount operation (MS_REMOUNT), to allow the filesystem to update
    /// internal resources as necessary to support the new flags.
    fn update_flags(
        &self,
        fs: &FileSystem,
        _current_task: &CurrentTask,
        new_flags: FileSystemFlags,
    ) -> Result<(), Errno> {
        fs.options.flags.store(new_flags, Ordering::Relaxed);
        Ok(())
    }

    fn name(&self) -> &'static FsStr;

    /// Whether this file system uses external node IDs.
    ///
    /// If this is true, then the file system is responsible for assigning node IDs to its nodes.
    /// Otherwise, the VFS will assign node IDs to the nodes.
    fn uses_external_node_ids(&self) -> bool {
        false
    }

    /// Returns whether this filesystem supports casefolded directory lookups.
    fn has_casefold_support(&self) -> bool {
        false
    }

    /// Rename the given node.
    ///
    /// The node to be renamed is passed as "renamed". It currently has
    /// old_name in old_parent. After the rename operation, it should have
    /// new_name in new_parent.
    ///
    /// If new_parent already has a child named new_name, that node is passed as
    /// "replaced". In that case, both "renamed" and "replaced" will be
    /// directories and the rename operation should succeed only if "replaced"
    /// is empty. The VFS will check that there are no children of "replaced" in
    /// the DirEntry cache, but the implementation of this function is
    /// responsible for checking that there are no children of replaced that are
    /// known only to the file system implementation (e.g., present on-disk but
    /// not in the DirEntry cache).
    fn rename(
        &self,
        _fs: &FileSystem,
        _current_task: &CurrentTask,
        _context: &mut RenameContext<'_>,
        _old_name: &FsStr,
        _new_name: &FsStr,
    ) -> Result<(), Errno> {
        error!(EROFS)
    }

    /// Exchanges the two nodes identified by `name1` and `name2` in the context.
    ///
    /// Semantically, this is an atomic exchange of two paths (similar to two
    /// renames, one in each direction). It uses `RenameContext` because the
    /// locking requirements and metadata needed (parent directories, node info)
    /// are identical to a rename operation involving two paths.
    fn exchange(
        &self,
        _fs: &FileSystem,
        _current_task: &CurrentTask,
        _context: &mut RenameContext<'_>,
        _name1: &FsStr,
        _name2: &FsStr,
    ) -> Result<(), Errno> {
        error!(EINVAL)
    }

    /// Called when the filesystem is unmounted.
    fn unmount(&self) {}

    /// Indicates if the filesystem can manage the timestamps (i.e. ctime and mtime).
    ///
    /// Starnix updates the timestamps in FsNode's `info` directly. However, if the filesystem can
    /// manage the timestamps, then Starnix does not need to do so. `info` will be refreshed with
    /// the timestamps from the filesystem by calling `fetch_and_refresh_info(..)` on the FsNode.
    fn manages_timestamps(&self) -> bool {
        false
    }

    /// Returns the crypt service associated with this filesystem, if any.
    fn crypt_service(&self) -> Option<Arc<CryptService>> {
        None
    }

    fn sync(&self, _fs: &FileSystem, _current_task: &CurrentTask) -> Result<(), Errno> {
        Ok(())
    }

    fn drop_caches(&self, _fs: &FileSystem) -> Result<(), Errno> {
        Ok(())
    }

    /// Returns any underlying filesystems wrapped by this filesystem (e.g. OverlayFs lower/upper).
    fn sub_filesystems(&self) -> Vec<Arc<FileSystem>> {
        Vec::new()
    }
}

impl Drop for FileSystem {
    fn drop(&mut self) {
        self.ops.unmount();
    }
}

pub type FileSystemHandle = Arc<FileSystem>;
