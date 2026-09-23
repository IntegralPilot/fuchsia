// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::device::DeviceMode;
use crate::mm::PAGE_SIZE;
use crate::security::{self, Auditable, PermissionFlags};
use crate::signals::{SignalInfo, send_standard_signal};
use crate::task::{CurrentTask, WaitQueue, Waiter, register_delayed_release};
use crate::time::utc;
use crate::vfs::fsverity::FsVerityState;
use crate::vfs::pipe::{Pipe, PipeHandle};
use crate::vfs::rw_queue::{RwQueue, RwQueueReadGuard, RwQueueWriteGuard};
use crate::vfs::socket::SocketHandle;
use crate::vfs::{
    CheckAccessReason, DefaultDirEntryOps, DirEntryOps, FileObject, FileObjectState, FileOps,
    FileSystem, FileSystemHandle, FileWriteGuardState, FsLockDepType, FsStr, FsString,
    MAX_LFS_FILESIZE, MountInfo, NamespaceNode, OPathOps, OpenAccessCheck, RecordLockCommand,
    RecordLockOwner, RecordLocks, WeakFileHandle, checked_add_offset_and_length, inotify_hook,
};
use bitflags::bitflags;
use fuchsia_runtime::UtcInstant;
use linux_uapi::{XATTR_SECURITY_PREFIX, XATTR_SYSTEM_PREFIX, XATTR_TRUSTED_PREFIX};
use once_cell::race::OnceBool;
use smallvec::SmallVec;
use starnix_crypt::EncryptionKeyId;
use starnix_lifecycle::{ObjectReleaser, ReleaserAction};
use starnix_logging::{log_error, track_stub};
use starnix_sync::{
    DynamicLockDepRwLock, FsNodeAppend, FsNodeFlockInfoLock, FsNodeFsVerityLock, FsNodeInfoLevel,
    FsNodeInfoRecursiveLevel, FsNodeWriteGuardStateLock, FuseFsNodeInfoLevel, LockDepMutex,
    LockDepReadGuard, allow_subclass,
};
use starnix_types::ownership::{Releasable, ReleaseGuard};
use starnix_types::time::{NANOS_PER_SECOND, timespec_from_time};
use starnix_uapi::as_any::AsAny;
use starnix_uapi::auth::{
    CAP_CHOWN, CAP_DAC_OVERRIDE, CAP_DAC_READ_SEARCH, CAP_FOWNER, CAP_FSETID, CAP_MKNOD,
    CAP_SYS_ADMIN, CAP_SYS_RESOURCE, Credentials, FsCred,
};
use starnix_uapi::device_id::DeviceId;
use starnix_uapi::errors::{EACCES, ENOTSUP, EPERM, Errno};
use starnix_uapi::file_mode::{Access, FileMode};
use starnix_uapi::inotify_mask::InotifyMask;
use starnix_uapi::mount_flags::MountFlags;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::resource_limits::Resource;
use starnix_uapi::seal_flags::SealFlags;
use starnix_uapi::signals::SIGXFSZ;
use starnix_uapi::{
    FALLOC_FL_COLLAPSE_RANGE, FALLOC_FL_INSERT_RANGE, FALLOC_FL_KEEP_SIZE, FALLOC_FL_PUNCH_HOLE,
    FALLOC_FL_UNSHARE_RANGE, FALLOC_FL_ZERO_RANGE, LOCK_EX, LOCK_NB, LOCK_SH, LOCK_UN,
    STATX__RESERVED, STATX_ATIME, STATX_ATTR_VERITY, STATX_BASIC_STATS, STATX_BLOCKS, STATX_CTIME,
    STATX_GID, STATX_INO, STATX_MTIME, STATX_NLINK, STATX_SIZE, STATX_UID, XATTR_USER_PREFIX,
    errno, error, fsverity_descriptor, gid_t, ino_t, statx, statx_timestamp, timespec, uapi, uid_t,
};
use std::sync::atomic::Ordering;
use std::sync::{Arc, OnceLock, Weak};
use syncio::zxio_node_attr_has_t;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsNodeLinkBehavior {
    Allowed,
    Disallowed,
}

impl Default for FsNodeLinkBehavior {
    fn default() -> Self {
        FsNodeLinkBehavior::Allowed
    }
}

pub type AppendLockGuard<'a> = RwQueueReadGuard<'a, FsNodeAppend>;
pub type AppendLockWriteGuard<'a> = RwQueueWriteGuard<'a, FsNodeAppend>;

bitflags! {
    pub struct FsNodeFlags: u8 {
        const IS_PRIVATE = 1 << 0;
    }
}

pub struct FsNode {
    /// The inode number for this FsNode.
    pub ino: ino_t,

    /// Flags for this node.
    pub flags: FsNodeFlags,

    /// The FsNodeOps for this FsNode.
    ///
    /// The FsNodeOps are implemented by the individual file systems to provide
    /// specific behaviors for this FsNode.
    ops: Box<dyn FsNodeOps>,

    /// The FileSystem that owns this FsNode's tree.
    fs: Weak<FileSystem>,

    /// A RwLock to synchronize append operations for this node.
    ///
    /// FileObjects writing with O_APPEND should grab a write() lock on this
    /// field to ensure they operate sequentially. FileObjects writing without
    /// O_APPEND should grab read() lock so that they can operate in parallel.
    pub append_lock: RwQueue<FsNodeAppend>,

    /// Mutable information about this node.
    ///
    /// This data is used to populate the uapi::stat structure.
    info: DynamicLockDepRwLock<FsNodeInfo>,

    /// Data associated with an FsNode that is rarely needed.
    rare_data: OnceLock<Box<FsNodeRareData>>,

    /// Tracks lock state for this file.
    pub write_guard_state: LockDepMutex<FileWriteGuardState, FsNodeWriteGuardStateLock>,

    /// Cached FsVerity state associated with this node.
    pub fsverity: LockDepMutex<FsVerityState, FsNodeFsVerityLock>,

    /// The security state associated with this node. Must always be acquired last
    /// relative to other `FsNode` locks.
    pub security_state: security::FsNodeState,
}

#[derive(Default)]
struct FsNodeRareData {
    /// The pipe located at this node, if any.
    ///
    /// Used if, and only if, the node has a mode of FileMode::IFIFO.
    fifo: OnceLock<PipeHandle>,

    /// The UNIX domain socket bound to this node, if any.
    bound_socket: OnceLock<SocketHandle>,

    /// Information about the locking information on this node.
    ///
    /// No other lock on this object may be taken while this lock is held.
    flock_info: LockDepMutex<FlockInfo, FsNodeFlockInfoLock>,

    /// Records locks associated with this node.
    record_locks: RecordLocks,

    /// Whether this node can be linked into a directory.
    ///
    /// Only set for nodes created with `O_TMPFILE`.
    link_behavior: OnceLock<FsNodeLinkBehavior>,

    /// Inotify watchers on this node. See inotify(7).
    watchers: inotify_hook::InotifyWatchers,
}

impl FsNodeRareData {
    fn ensure_fifo(&self, current_task: &CurrentTask) -> &PipeHandle {
        self.fifo.get_or_init(|| {
            let default_pipe_capacity = (*PAGE_SIZE * 16) as usize;
            let kernel = current_task.kernel();
            let max_size = kernel.system_limits.pipe_max_size.load(Ordering::Relaxed);
            let capacity = if default_pipe_capacity <= max_size
                || security::is_task_capable_noaudit(current_task, CAP_SYS_RESOURCE)
            {
                default_pipe_capacity
            } else {
                max_size
            };
            Pipe::new(capacity)
        })
    }
}

pub enum FsNodeReleaserAction {}
impl ReleaserAction<FsNode> for FsNodeReleaserAction {
    fn release(fs_node: ReleaseGuard<FsNode>) {
        register_delayed_release(fs_node);
    }
}
pub type FsNodeReleaser = ObjectReleaser<FsNode, FsNodeReleaserAction>;
pub type FsNodeHandle = Arc<FsNodeReleaser>;
pub type WeakFsNodeHandle = Weak<FsNodeReleaser>;

#[derive(Debug, Default, Clone, PartialEq)]
pub struct FsNodeInfo {
    pub mode: FileMode,
    pub link_count: usize,
    pub uid: uid_t,
    pub gid: gid_t,
    pub rdev: DeviceId,
    pub size: usize,
    pub blksize: usize,
    pub blocks: usize,
    pub time_status_change: UtcInstant,
    pub time_access: UtcInstant,
    pub time_modify: UtcInstant,
    pub casefold: bool,

    // If this node is fscrypt encrypted, stores the id of the user wrapping key used to encrypt it.
    pub wrapping_key_id: Option<[u8; 16]>,

    // Used to indicate to filesystems that manage timestamps that an access has occurred and to
    // update the node's atime.
    // This only impacts accesses within Starnix. Most Fuchsia programs are not expected to maintain
    // access times. If the file handle is transferred out of Starnix, there may be inconsistencies.
    pub pending_time_access_update: bool,
}

impl FsNodeInfo {
    pub fn new(mode: FileMode, owner: FsCred) -> Self {
        let now = utc::utc_now();
        Self {
            mode,
            link_count: if mode.is_dir() { 2 } else { 1 },
            uid: owner.uid,
            gid: owner.gid,
            blksize: DEFAULT_BYTES_PER_BLOCK,
            time_status_change: now,
            time_access: now,
            time_modify: now,
            ..Default::default()
        }
    }

    pub fn storage_size(&self) -> usize {
        self.blksize.saturating_mul(self.blocks)
    }

    pub fn chmod(&mut self, mode: FileMode) {
        self.mode = (self.mode & !FileMode::PERMISSIONS) | (mode & FileMode::PERMISSIONS);
    }

    pub fn chown(&mut self, owner: Option<uid_t>, group: Option<gid_t>) {
        if let Some(owner) = owner {
            self.uid = owner;
        }
        if let Some(group) = group {
            self.gid = group;
        }
        // Clear the setuid and setgid bits if the file is executable and a regular file.
        if self.mode.is_reg() {
            self.mode &= !FileMode::ISUID;
            self.clear_sgid_bit();
        }
    }

    fn clear_sgid_bit(&mut self) {
        // If the group execute bit is not set, the setgid bit actually indicates mandatory
        // locking and should not be cleared.
        if self.mode.intersects(FileMode::IXGRP) {
            self.mode &= !FileMode::ISGID;
        }
    }

    fn has_suid_or_sgid_bits(&self) -> bool {
        // ISGID is only considered as setgid bit if IXGRP is present.
        self.mode.contains(FileMode::ISUID) || self.mode.contains(FileMode::ISGID | FileMode::IXGRP)
    }

    fn clear_suid_and_sgid_bits(&mut self) {
        self.mode &= !FileMode::ISUID;
        self.clear_sgid_bit();
    }

    pub fn cred(&self) -> FsCred {
        FsCred { uid: self.uid, gid: self.gid }
    }

    pub fn apply_suid_and_sgid(&self, creds: &mut Credentials) {
        if self.mode.contains(FileMode::ISUID) {
            creds.euid = self.uid;
        }

        // See <https://man7.org/linux/man-pages/man7/inode.7.html>:
        //
        //   For an executable file, the set-group-ID bit causes the
        //   effective group ID of a process that executes the file to change
        //   as described in execve(2).  For a file that does not have the
        //   group execution bit (S_IXGRP) set, the set-group-ID bit indicates
        //   mandatory file/record locking.
        if self.mode.contains(FileMode::ISGID | FileMode::IXGRP) {
            creds.egid = self.gid;
        }
    }
}

#[derive(Default)]
struct FlockInfo {
    /// Whether the node is currently locked. The meaning of the different values are:
    /// - `None`: The node is not locked.
    /// - `Some(false)`: The node is locked non exclusively.
    /// - `Some(true)`: The node is locked exclusively.
    locked_exclusive: Option<bool>,
    /// The FileObject that hold the lock.
    locking_handles: Vec<WeakFileHandle>,
    /// The queue to notify process waiting on the lock.
    wait_queue: WaitQueue,
}

impl FlockInfo {
    /// Removes all file handle not holding `predicate` from the list of object holding the lock. If
    /// this empties the list, unlocks the node and notifies all waiting processes.
    pub fn retain<F>(&mut self, predicate: F)
    where
        F: Fn(&FileObject) -> bool,
    {
        if !self.locking_handles.is_empty() {
            self.locking_handles
                .retain(|w| if let Some(fh) = w.upgrade() { predicate(&fh) } else { false });
            if self.locking_handles.is_empty() {
                self.locked_exclusive = None;
                self.wait_queue.notify_all();
            }
        }
    }
}

/// `st_blksize` is measured in units of 512 bytes.
pub const DEFAULT_BYTES_PER_BLOCK: usize = 512;

pub struct FlockOperation {
    operation: u32,
}

impl FlockOperation {
    pub fn from_flags(operation: u32) -> Result<Self, Errno> {
        if operation & !(LOCK_SH | LOCK_EX | LOCK_UN | LOCK_NB) != 0 {
            return error!(EINVAL);
        }
        if [LOCK_SH, LOCK_EX, LOCK_UN].iter().filter(|&&o| operation & o == o).count() != 1 {
            return error!(EINVAL);
        }
        Ok(Self { operation })
    }

    pub fn is_unlock(&self) -> bool {
        self.operation & LOCK_UN > 0
    }

    pub fn is_lock_exclusive(&self) -> bool {
        self.operation & LOCK_EX > 0
    }

    pub fn is_blocking(&self) -> bool {
        self.operation & LOCK_NB == 0
    }
}

impl FileObject {
    /// Advisory locking.
    ///
    /// See flock(2).
    pub fn flock(
        &self,
        current_task: &CurrentTask,
        operation: FlockOperation,
    ) -> Result<(), Errno> {
        if self.flags().contains(OpenFlags::PATH) {
            return error!(EBADF);
        }
        security::check_file_lock_access(current_task, self)?;
        loop {
            let mut flock_info = self.name.entry.node.ensure_rare_data().flock_info.lock();
            if operation.is_unlock() {
                flock_info.retain(|fh| !std::ptr::eq(fh, self));
                return Ok(());
            }
            // Operation is a locking operation.
            // 1. File is not locked
            if flock_info.locked_exclusive.is_none() {
                flock_info.locked_exclusive = Some(operation.is_lock_exclusive());
                flock_info.locking_handles.push(self.weak_handle.clone());
                return Ok(());
            }

            let file_lock_is_exclusive = flock_info.locked_exclusive == Some(true);
            let fd_has_lock = flock_info
                .locking_handles
                .iter()
                .find_map(|w| {
                    w.upgrade().and_then(|fh| {
                        if std::ptr::eq(&fh as &FileObject, self) { Some(()) } else { None }
                    })
                })
                .is_some();

            // 2. File is locked, but fd already have a lock
            if fd_has_lock {
                if operation.is_lock_exclusive() == file_lock_is_exclusive {
                    // Correct lock is already held, return.
                    return Ok(());
                } else {
                    // Incorrect lock is held. Release the lock and loop back to try to reacquire
                    // it. flock doesn't guarantee atomic lock type switching.
                    flock_info.retain(|fh| !std::ptr::eq(fh, self));
                    continue;
                }
            }

            // 3. File is locked, and fd doesn't have a lock.
            if !file_lock_is_exclusive && !operation.is_lock_exclusive() {
                // The lock is not exclusive, let's grab it.
                flock_info.locking_handles.push(self.weak_handle.clone());
                return Ok(());
            }

            // 4. The operation cannot be done at this time.
            if !operation.is_blocking() {
                return error!(EAGAIN);
            }

            // Register a waiter to be notified when the lock is released. Release the lock on
            // FlockInfo, and wait.
            let waiter = Waiter::new();
            flock_info.wait_queue.wait_async(&waiter);
            std::mem::drop(flock_info);
            waiter.wait(current_task)?;
        }
    }
}

// The inner mod is required because bitflags cannot pass the attribute through to the single
// variant, and attributes cannot be applied to macro invocations.
mod inner_flags {
    // Part of the code for the AT_STATX_SYNC_AS_STAT case that's produced by the macro triggers the
    // lint, but as a whole, the produced code is still correct.
    #![allow(clippy::bad_bit_mask)] // TODO(b/303500202) Remove once addressed in bitflags.
    use super::{bitflags, uapi};

    bitflags! {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct StatxFlags: u32 {
            const AT_SYMLINK_NOFOLLOW = uapi::AT_SYMLINK_NOFOLLOW;
            const AT_EMPTY_PATH = uapi::AT_EMPTY_PATH;
            const AT_NO_AUTOMOUNT = uapi::AT_NO_AUTOMOUNT;
            const AT_STATX_SYNC_AS_STAT = uapi::AT_STATX_SYNC_AS_STAT;
            const AT_STATX_FORCE_SYNC = uapi::AT_STATX_FORCE_SYNC;
            const AT_STATX_DONT_SYNC = uapi::AT_STATX_DONT_SYNC;
            const STATX_ATTR_VERITY = uapi::STATX_ATTR_VERITY;
        }
    }
}

pub use inner_flags::StatxFlags;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum UnlinkKind {
    /// Unlink a directory.
    Directory,

    /// Unlink a non-directory.
    NonDirectory,
}

pub enum SymlinkTarget {
    Path(FsString),
    Node(NamespaceNode),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum XattrOp {
    /// Set the value of the extended attribute regardless of whether it exists.
    Set,
    /// Create a new extended attribute. Fail if it already exists.
    Create,
    /// Replace the value of the extended attribute. Fail if it doesn't exist.
    Replace,
}

impl XattrOp {
    pub fn into_flags(self) -> u32 {
        match self {
            Self::Set => 0,
            Self::Create => uapi::XATTR_CREATE,
            Self::Replace => uapi::XATTR_REPLACE,
        }
    }
}

/// Returns a value, or the size required to contains it.
#[derive(Clone, Debug, PartialEq)]
pub enum ValueOrSize<T> {
    Value(T),
    Size(usize),
}

impl<T> ValueOrSize<T> {
    pub fn map<F, U>(self, f: F) -> ValueOrSize<U>
    where
        F: FnOnce(T) -> U,
    {
        match self {
            Self::Size(s) => ValueOrSize::Size(s),
            Self::Value(v) => ValueOrSize::Value(f(v)),
        }
    }

    #[cfg(test)]
    pub fn unwrap(self) -> T {
        match self {
            Self::Size(_) => panic!("Unwrap ValueOrSize that is a Size"),
            Self::Value(v) => v,
        }
    }
}

impl<T> From<T> for ValueOrSize<T> {
    fn from(t: T) -> Self {
        Self::Value(t)
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum FallocMode {
    Allocate { keep_size: bool },
    PunchHole,
    Collapse,
    Zero { keep_size: bool },
    InsertRange,
    UnshareRange,
}

impl FallocMode {
    pub fn from_bits(mode: u32) -> Option<Self> {
        // `fallocate()` allows only the following values for `mode`.
        if mode == 0 {
            Some(Self::Allocate { keep_size: false })
        } else if mode == FALLOC_FL_KEEP_SIZE {
            Some(Self::Allocate { keep_size: true })
        } else if mode == FALLOC_FL_PUNCH_HOLE | FALLOC_FL_KEEP_SIZE {
            Some(Self::PunchHole)
        } else if mode == FALLOC_FL_COLLAPSE_RANGE {
            Some(Self::Collapse)
        } else if mode == FALLOC_FL_ZERO_RANGE {
            Some(Self::Zero { keep_size: false })
        } else if mode == FALLOC_FL_ZERO_RANGE | FALLOC_FL_KEEP_SIZE {
            Some(Self::Zero { keep_size: true })
        } else if mode == FALLOC_FL_INSERT_RANGE {
            Some(Self::InsertRange)
        } else if mode == FALLOC_FL_UNSHARE_RANGE {
            Some(Self::UnshareRange)
        } else {
            None
        }
    }
}

pub type LookupVec<T> = SmallVec<[T; 8]>;

pub trait FsNodeOps: Send + Sync + AsAny + 'static {
    /// Delegate the access check to the node.
    fn check_access(
        &self,
        node: &FsNode,
        current_task: &CurrentTask,
        access: security::PermissionFlags,
        info: &DynamicLockDepRwLock<FsNodeInfo>,
        reason: CheckAccessReason,
        audit_context: security::Auditable<'_>,
    ) -> Result<(), Errno> {
        node.default_check_access_impl(current_task, access, reason, info.read(), audit_context)
    }

    /// Build the [`DirEntryOps`] for a new [`DirEntry`] that will be associated
    /// to this node.
    fn create_dir_entry_ops(&self) -> Box<dyn DirEntryOps> {
        Box::new(DefaultDirEntryOps)
    }

    /// Build the `FileOps` for the file associated to this node.
    ///
    /// The returned FileOps will be used to create a FileObject, which might
    /// be assigned an FdNumber.
    fn create_file_ops(
        &self,
        node: &FsNode,
        _current_task: &CurrentTask,
        flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno>;

    /// Find an existing child node and populate the child parameter. Return the node.
    ///
    /// The child parameter is an empty node. Operations other than initialize may panic before
    /// initialize is called.
    fn lookup(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        name: &FsStr,
    ) -> Result<FsNodeHandle, Errno> {
        // The default implementation here is suitable for filesystems that have permanent entries;
        // entries that already exist will get found in the cache and shouldn't get this far.
        error!(ENOENT, format!("looking for {name}"))
    }

    /// Returns whether this node supports pipelined lookups.
    fn has_lookup_pipelined(&self) -> bool {
        false
    }

    /// Find multiple children nodes in sequence.
    ///
    /// This can be used to pipeline lookups in filesystems that support it.
    fn lookup_pipelined(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _names: &[&FsStr],
    ) -> LookupVec<Result<FsNodeHandle, Errno>> {
        panic!("has_lookup_pipelined should be false");
    }

    /// Create and return the given child node.
    ///
    /// The mode field of the FsNodeInfo indicates what kind of child to
    /// create.
    ///
    /// This function is never called with FileMode::IFDIR. The mkdir function
    /// is used to create directories instead.
    fn mknod(
        &self,
        node: &FsNode,
        _current_task: &CurrentTask,
        _name: &FsStr,
        _mode: FileMode,
        _dev: DeviceId,
        _owner: FsCred,
    ) -> Result<FsNodeHandle, Errno>;

    /// Create and return the given child node as a subdirectory.
    fn mkdir(
        &self,
        node: &FsNode,
        _current_task: &CurrentTask,
        _name: &FsStr,
        _mode: FileMode,
        _owner: FsCred,
    ) -> Result<FsNodeHandle, Errno>;

    /// Creates a symlink with the given `target` path.
    fn create_symlink(
        &self,
        node: &FsNode,
        _current_task: &CurrentTask,
        _name: &FsStr,
        _target: &FsStr,
        _owner: FsCred,
    ) -> Result<FsNodeHandle, Errno>;

    /// Creates an anonymous file.
    ///
    /// The FileMode::IFMT of the FileMode is always FileMode::IFREG.
    ///
    /// Used by O_TMPFILE.
    fn create_tmpfile(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _mode: FileMode,
        _owner: FsCred,
    ) -> Result<FsNodeHandle, Errno> {
        error!(EOPNOTSUPP)
    }

    /// Reads the symlink from this node.
    fn readlink(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
    ) -> Result<SymlinkTarget, Errno> {
        error!(EINVAL)
    }

    /// Create a hard link with the given name to the given child.
    fn link(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _name: &FsStr,
        _child: &FsNodeHandle,
    ) -> Result<(), Errno> {
        error!(EPERM)
    }

    /// Remove the child with the given name, if the child exists.
    ///
    /// The UnlinkKind parameter indicates whether the caller intends to unlink
    /// a directory or a non-directory child.
    fn unlink(
        &self,
        node: &FsNode,
        _current_task: &CurrentTask,
        _name: &FsStr,
        _child: &FsNodeHandle,
    ) -> Result<(), Errno>;

    /// Acquire the necessary append lock for the operations that depend on them.
    /// Should be done before calling `allocate` or `truncate` to avoid lock ordering issues.
    fn append_lock_read<'a>(
        &'a self,
        node: &'a FsNode,
        current_task: &CurrentTask,
    ) -> Result<AppendLockGuard<'a>, Errno> {
        return node.append_lock.read(current_task);
    }

    /// Acquire the necessary append lock for operations that need exclusive access (e.g., write append).
    fn append_lock_write<'a>(
        &'a self,
        node: &'a FsNode,
        current_task: &CurrentTask,
    ) -> Result<AppendLockWriteGuard<'a>, Errno> {
        return node.append_lock.write(current_task);
    }

    /// Change the length of the file.
    fn truncate(
        &self,
        _guard: &AppendLockWriteGuard<'_>,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _length: u64,
    ) -> Result<(), Errno> {
        error!(EINVAL)
    }

    /// Manipulate allocated disk space for the file.
    fn allocate(
        &self,
        _guard: &AppendLockWriteGuard<'_>,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _mode: FallocMode,
        _offset: u64,
        _length: u64,
    ) -> Result<(), Errno> {
        error!(EINVAL)
    }

    /// Update the supplied info with initial state (e.g. size) for the node.
    ///
    /// FsNode calls this method when created, to allow the FsNodeOps to
    /// set appropriate initial values in the FsNodeInfo.
    fn initial_info(&self, _info: &mut FsNodeInfo) {}

    /// Update node.info as needed.
    ///
    /// FsNode calls this method before converting the FsNodeInfo struct into
    /// the uapi::stat struct to give the file system a chance to update this data
    /// before it is used by clients.
    ///
    /// File systems that keep the FsNodeInfo up-to-date do not need to
    /// override this function.
    ///
    /// Return a read guard for the updated information.
    fn fetch_and_refresh_info<'a>(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        info: &'a DynamicLockDepRwLock<FsNodeInfo>,
    ) -> Result<LockDepReadGuard<'a, FsNodeInfo>, Errno> {
        Ok(info.read())
    }

    /// Syncs cached data to persistent storage.
    fn sync(&self, _node: &FsNode, _current_task: &CurrentTask) -> Result<(), Errno> {
        Ok(())
    }

    /// Update node attributes persistently.
    ///
    /// Implementations must enforce prerequisites for attribute changes (for example,
    /// enabling casefold on a non-empty directory must return [`ENOTEMPTY`]).
    fn update_attributes(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _info: &FsNodeInfo,
        _has: zxio_node_attr_has_t,
    ) -> Result<(), Errno> {
        Ok(())
    }

    /// Get an extended attribute on the node.
    ///
    /// An implementation can systematically return a value. Otherwise, if `max_size` is 0, it can
    /// instead return the size of the attribute, and can return an ERANGE error if max_size is not
    /// 0, and lesser than the required size.
    fn get_xattr(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _name: &FsStr,
        _max_size: usize,
    ) -> Result<ValueOrSize<FsString>, Errno> {
        error!(ENOTSUP)
    }

    /// Set an extended attribute on the node.
    fn set_xattr(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _name: &FsStr,
        _value: &FsStr,
        _op: XattrOp,
    ) -> Result<(), Errno> {
        error!(ENOTSUP)
    }

    fn remove_xattr(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _name: &FsStr,
    ) -> Result<(), Errno> {
        error!(ENOTSUP)
    }

    /// An implementation can systematically return a value. Otherwise, if `max_size` is 0, it can
    /// instead return the size of the 0 separated string needed to represent the value, and can
    /// return an ERANGE error if max_size is not 0, and lesser than the required size.
    fn list_xattrs(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _max_size: usize,
    ) -> Result<ValueOrSize<Vec<FsString>>, Errno> {
        error!(ENOTSUP)
    }

    /// Called when the FsNode is freed by the Kernel.
    fn forget(
        self: Box<Self>,
        _current_task: &CurrentTask,
        _info: FsNodeInfo,
    ) -> Result<(), Errno> {
        Ok(())
    }

    ////////////////////
    // FS-Verity operations

    /// Marks that FS-Verity is being built. Writes fsverity descriptor and merkle tree, the latter
    /// computed by the filesystem.
    /// This should ensure there are no writable file handles. Returns EEXIST if the file was
    /// already fsverity-enabled. Returns EBUSY if this ioctl was already running on this file.
    fn enable_fsverity(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _descriptor: &fsverity_descriptor,
    ) -> Result<(), Errno> {
        error!(ENOTSUP)
    }

    /// Read fsverity descriptor, if the node is fsverity-enabled. Else returns ENODATA.
    fn get_fsverity_descriptor(&self, _log_blocksize: u8) -> Result<fsverity_descriptor, Errno> {
        error!(ENOTSUP)
    }

    /// The key used to identify this node in the file system's node cache.
    ///
    /// For many file systems, this will be the same as the inode number. However, some file
    /// systems, such as FUSE, sometimes use different `node_key` and inode numbers.
    fn node_key(&self, node: &FsNode) -> ino_t {
        node.ino
    }

    /// Returns the size of the file.
    fn get_size(&self, node: &FsNode, current_task: &CurrentTask) -> Result<usize, Errno> {
        let info = node.fetch_and_refresh_info(current_task)?;
        Ok(info.size.try_into().map_err(|_| errno!(EINVAL))?)
    }
}

impl<T> From<T> for Box<dyn FsNodeOps>
where
    T: FsNodeOps,
{
    fn from(ops: T) -> Box<dyn FsNodeOps> {
        Box::new(ops)
    }
}

/// Implements [`FsNodeOps`] methods in a way that makes sense for symlinks.
/// You must implement [`FsNodeOps::readlink`].
#[macro_export]
macro_rules! fs_node_impl_symlink {
    () => {
        $crate::vfs::fs_node_impl_not_dir!();

        fn create_file_ops(
            &self,
            node: &$crate::vfs::FsNode,
            _current_task: &CurrentTask,
            _flags: starnix_uapi::open_flags::OpenFlags,
        ) -> Result<Box<dyn $crate::vfs::FileOps>, starnix_uapi::errors::Errno> {
            assert!(node.is_lnk());
            unreachable!("Symlink nodes cannot be opened.");
        }
    };
}

#[macro_export]
macro_rules! fs_node_impl_dir_readonly {
    () => {
        fn check_access(
            &self,
            node: &$crate::vfs::FsNode,
            current_task: &$crate::task::CurrentTask,
            permission_flags: $crate::security::PermissionFlags,
            info: &starnix_sync::DynamicLockDepRwLock<$crate::vfs::FsNodeInfo>,
            reason: $crate::vfs::CheckAccessReason,
            audit_context: $crate::security::Auditable<'_>,
        ) -> Result<(), starnix_uapi::errors::Errno> {
            let access = permission_flags.as_access();
            if access.contains(starnix_uapi::file_mode::Access::WRITE) {
                return starnix_uapi::error!(
                    EROFS,
                    format!("check_access failed: read-only directory")
                );
            }
            node.default_check_access_impl(
                current_task,
                permission_flags,
                reason,
                info.read(),
                audit_context,
            )
        }

        fn mkdir(
            &self,
            _node: &$crate::vfs::FsNode,
            _current_task: &$crate::task::CurrentTask,
            name: &$crate::vfs::FsStr,
            _mode: starnix_uapi::file_mode::FileMode,
            _owner: starnix_uapi::auth::FsCred,
        ) -> Result<$crate::vfs::FsNodeHandle, starnix_uapi::errors::Errno> {
            starnix_uapi::error!(EROFS, format!("mkdir failed: {:?}", name))
        }

        fn mknod(
            &self,
            _node: &$crate::vfs::FsNode,
            _current_task: &$crate::task::CurrentTask,
            name: &$crate::vfs::FsStr,
            _mode: starnix_uapi::file_mode::FileMode,
            _dev: starnix_uapi::device_id::DeviceId,
            _owner: starnix_uapi::auth::FsCred,
        ) -> Result<$crate::vfs::FsNodeHandle, starnix_uapi::errors::Errno> {
            starnix_uapi::error!(EROFS, format!("mknod failed: {:?}", name))
        }

        fn create_symlink(
            &self,
            _node: &$crate::vfs::FsNode,
            _current_task: &$crate::task::CurrentTask,
            name: &$crate::vfs::FsStr,
            _target: &$crate::vfs::FsStr,
            _owner: starnix_uapi::auth::FsCred,
        ) -> Result<$crate::vfs::FsNodeHandle, starnix_uapi::errors::Errno> {
            starnix_uapi::error!(EROFS, format!("symlink failed: {:?}", name))
        }

        fn link(
            &self,
            _node: &$crate::vfs::FsNode,
            _current_task: &$crate::task::CurrentTask,
            name: &$crate::vfs::FsStr,
            _child: &$crate::vfs::FsNodeHandle,
        ) -> Result<(), starnix_uapi::errors::Errno> {
            starnix_uapi::error!(EROFS, format!("link failed: {:?}", name))
        }

        fn unlink(
            &self,
            _node: &$crate::vfs::FsNode,
            _current_task: &$crate::task::CurrentTask,
            name: &$crate::vfs::FsStr,
            _child: &$crate::vfs::FsNodeHandle,
        ) -> Result<(), starnix_uapi::errors::Errno> {
            starnix_uapi::error!(EROFS, format!("unlink failed: {:?}", name))
        }
    };
}

/// Trait that objects can implement if they need to handle extended attribute storage. Allows
/// delegating extended attribute operations in [`FsNodeOps`] to another object.
///
/// See [`fs_node_impl_xattr_delegate`] for usage details.
pub trait XattrStorage {
    /// Delegate for [`FsNodeOps::get_xattr`].
    fn get_xattr(&self, name: &FsStr) -> Result<FsString, Errno>;

    /// Delegate for [`FsNodeOps::set_xattr`].
    fn set_xattr(&self, name: &FsStr, value: &FsStr, op: XattrOp) -> Result<(), Errno>;

    /// Delegate for [`FsNodeOps::remove_xattr`].
    fn remove_xattr(&self, name: &FsStr) -> Result<(), Errno>;

    /// Delegate for [`FsNodeOps::list_xattrs`].
    fn list_xattrs(&self) -> Result<Vec<FsString>, Errno>;
}

/// Implements extended attribute ops for [`FsNodeOps`] by delegating to another object which
/// implements the [`XattrStorage`] trait or a similar interface. For example:
///
/// ```
/// struct Xattrs {}
///
/// impl XattrStorage for Xattrs {
///     // implement XattrStorage
/// }
///
/// struct Node {
///     xattrs: Xattrs
/// }
///
/// impl FsNodeOps for Node {
///     // Delegate extended attribute ops in FsNodeOps to self.xattrs
///     fs_node_impl_xattr_delegate!(self, self.xattrs);
///
///     // add other FsNodeOps impls here
/// }
/// ```
#[macro_export]
macro_rules! fs_node_impl_xattr_delegate {
    ($self:ident, $delegate:expr) => {
        fn get_xattr(
            &$self,
            _node: &FsNode,
            _current_task: &CurrentTask,
            name: &$crate::vfs::FsStr,
            _size: usize,
        ) -> Result<$crate::vfs::ValueOrSize<$crate::vfs::FsString>, starnix_uapi::errors::Errno> {
            Ok($delegate.get_xattr(name)?.into())
        }

        fn set_xattr(
            &$self,
            _node: &FsNode,
            _current_task: &CurrentTask,
            name: &$crate::vfs::FsStr,
            value: &$crate::vfs::FsStr,
            op: $crate::vfs::XattrOp,
        ) -> Result<(), starnix_uapi::errors::Errno> {
            $delegate.set_xattr(name, value, op)
        }

        fn remove_xattr(
            &$self,
            _node: &FsNode,
            _current_task: &CurrentTask,
            name: &$crate::vfs::FsStr,
        ) -> Result<(), starnix_uapi::errors::Errno> {
            $delegate.remove_xattr(name)
        }

        fn list_xattrs(
            &$self,
            _node: &FsNode,
            _current_task: &CurrentTask,
            _size: usize,
        ) -> Result<$crate::vfs::ValueOrSize<Vec<$crate::vfs::FsString>>, starnix_uapi::errors::Errno> {
            Ok($delegate.list_xattrs()?.into())
        }
    };
}

/// Stubs out [`FsNodeOps`] methods that only apply to directories.
#[macro_export]
macro_rules! fs_node_impl_not_dir {
    () => {
        fn lookup(
            &self,
            _node: &$crate::vfs::FsNode,
            _current_task: &$crate::task::CurrentTask,
            _name: &$crate::vfs::FsStr,
        ) -> Result<$crate::vfs::FsNodeHandle, starnix_uapi::errors::Errno> {
            starnix_uapi::error!(ENOTDIR)
        }

        fn mknod(
            &self,
            _node: &$crate::vfs::FsNode,
            _current_task: &$crate::task::CurrentTask,
            _name: &$crate::vfs::FsStr,
            _mode: starnix_uapi::file_mode::FileMode,
            _dev: starnix_uapi::device_id::DeviceId,
            _owner: starnix_uapi::auth::FsCred,
        ) -> Result<$crate::vfs::FsNodeHandle, starnix_uapi::errors::Errno> {
            starnix_uapi::error!(ENOTDIR)
        }

        fn mkdir(
            &self,
            _node: &$crate::vfs::FsNode,
            _current_task: &$crate::task::CurrentTask,
            _name: &$crate::vfs::FsStr,
            _mode: starnix_uapi::file_mode::FileMode,
            _owner: starnix_uapi::auth::FsCred,
        ) -> Result<$crate::vfs::FsNodeHandle, starnix_uapi::errors::Errno> {
            starnix_uapi::error!(ENOTDIR)
        }

        fn create_symlink(
            &self,
            _node: &$crate::vfs::FsNode,
            _current_task: &$crate::task::CurrentTask,
            _name: &$crate::vfs::FsStr,
            _target: &$crate::vfs::FsStr,
            _owner: starnix_uapi::auth::FsCred,
        ) -> Result<$crate::vfs::FsNodeHandle, starnix_uapi::errors::Errno> {
            starnix_uapi::error!(ENOTDIR)
        }

        fn unlink(
            &self,
            _node: &$crate::vfs::FsNode,
            _current_task: &$crate::task::CurrentTask,
            _name: &$crate::vfs::FsStr,
            _child: &$crate::vfs::FsNodeHandle,
        ) -> Result<(), starnix_uapi::errors::Errno> {
            starnix_uapi::error!(ENOTDIR)
        }
    };
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TimeUpdateType {
    Now,
    Omit,
    Time(UtcInstant),
}

// Public re-export of macros allows them to be used like regular rust items.
pub use fs_node_impl_dir_readonly;
pub use fs_node_impl_not_dir;
pub use fs_node_impl_symlink;
pub use fs_node_impl_xattr_delegate;

pub struct SpecialNode;

impl FsNodeOps for SpecialNode {
    fs_node_impl_not_dir!();

    fn create_file_ops(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        unreachable!("Special nodes cannot be opened.");
    }
}

impl FsNode {
    /// Returns true if the `fs_node` is private to the `Kernel`/`FileSystem`, in which
    /// case both MAC and DAC checks should be skipped.
    pub fn is_private(&self) -> bool {
        self.flags.contains(FsNodeFlags::IS_PRIVATE)
    }

    /// Create a node without inserting it into the FileSystem node cache.
    ///
    /// This is usually not what you want!
    /// Only use if you're also using get_or_create_node, like ext4.
    pub fn new_uncached(
        ino: ino_t,
        ops: impl Into<Box<dyn FsNodeOps>>,
        fs: &FileSystemHandle,
        info: FsNodeInfo,
        flags: FsNodeFlags,
    ) -> FsNodeHandle {
        let ops = ops.into();
        FsNodeHandle::new(Self::new_internal(ino, ops, Arc::downgrade(fs), info, flags).into())
    }

    fn new_internal(
        ino: ino_t,
        ops: Box<dyn FsNodeOps>,
        fs: Weak<FileSystem>,
        info: FsNodeInfo,
        flags: FsNodeFlags,
    ) -> Self {
        // Allow the FsNodeOps to populate initial info.
        let mut info = info;
        ops.initial_info(&mut info);

        let fs_lockdep_type =
            fs.upgrade().map(|fs| fs.fs_lockdep_type()).unwrap_or(FsLockDepType::Normal);
        let info_lock = match fs_lockdep_type {
            FsLockDepType::Normal => DynamicLockDepRwLock::new::<FsNodeInfoLevel>(info),
            FsLockDepType::Fuse => DynamicLockDepRwLock::new::<FuseFsNodeInfoLevel>(info),
            FsLockDepType::Recursive => DynamicLockDepRwLock::new::<FsNodeInfoRecursiveLevel>(info),
        };

        // The linter will fail in non test mode as it will not see the lock check.
        #[allow(clippy::let_and_return)]
        {
            let result = Self {
                ino,
                flags,
                ops,
                fs,
                info: info_lock,
                append_lock: Default::default(),
                rare_data: Default::default(),
                write_guard_state: Default::default(),
                fsverity: Default::default(),
                security_state: Default::default(),
            };
            #[cfg(any(test, debug_assertions))]
            {
                let _l1 = result.append_lock.read_for_lock_ordering();
                let _l2 = result.info.read();
                let _l3 = result.write_guard_state.lock();
                let _l4 = result.fsverity.lock();
            }
            result
        }
    }

    pub fn fs(&self) -> FileSystemHandle {
        self.fs.upgrade().expect("FileSystem did not live long enough")
    }

    pub fn ops(&self) -> &dyn FsNodeOps {
        self.ops.as_ref()
    }

    /// Returns an error if this node is encrypted and locked. Does not require
    /// fetch_and_refresh_info because FS_IOC_SET_ENCRYPTION_POLICY updates info and once a node is
    /// encrypted, it remains encrypted forever.
    pub fn fail_if_locked(
        &self,
        _current_task: &CurrentTask,
        node_info: &FsNodeInfo,
    ) -> Result<(), Errno> {
        if let Some(wrapping_key_id) = node_info.wrapping_key_id {
            let crypt_service = self.fs().crypt_service().ok_or_else(|| errno!(ENOKEY))?;
            if !crypt_service.contains_key(EncryptionKeyId::from(wrapping_key_id)) {
                return error!(ENOKEY);
            }
        }
        Ok(())
    }

    /// Returns the `FsNode`'s `FsNodeOps` as a `&T`, or `None` if the downcast fails.
    pub fn downcast_ops<T>(&self) -> Option<&T>
    where
        T: 'static,
    {
        self.ops().as_any().downcast_ref::<T>()
    }

    pub fn on_file_closed(&self, file: &FileObjectState) {
        if let Some(rare_data) = self.rare_data.get() {
            let mut flock_info = rare_data.flock_info.lock();
            // This function will drop the flock from `file` because the `WeakFileHandle` for
            // `file` will no longer upgrade to an `FileHandle`.
            flock_info.retain(|_| true);
        }
        self.record_lock_release(RecordLockOwner::FileObject(file.id));
    }

    pub fn record_lock(
        &self,
        current_task: &CurrentTask,
        file: &FileObject,
        cmd: RecordLockCommand,
        flock: uapi::flock,
    ) -> Result<Option<uapi::flock>, Errno> {
        self.ensure_rare_data().record_locks.lock(current_task, file, cmd, flock)
    }

    /// Release all record locks acquired by the given owner.
    pub fn record_lock_release(&self, owner: RecordLockOwner) {
        if let Some(rare_data) = self.rare_data.get() {
            rare_data.record_locks.release_locks(owner);
        }
    }

    pub fn create_dir_entry_ops(&self) -> Box<dyn DirEntryOps> {
        self.ops().create_dir_entry_ops()
    }

    pub fn create_file_ops(
        &self,
        current_task: &CurrentTask,
        flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        self.ops().create_file_ops(self, current_task, flags)
    }

    pub fn open(
        &self,
        current_task: &CurrentTask,
        namespace_node: &NamespaceNode,
        open_check: impl Into<OpenAccessCheck>,
    ) -> Result<Box<dyn FileOps>, Errno> {
        let open_check = open_check.into();
        let flags = open_check.open_flags();
        // If O_PATH is set, there is no need to create a real FileOps because
        // most file operations are disabled.
        if flags.contains(OpenFlags::PATH) {
            return Ok(Box::new(OPathOps::new()));
        }

        let access_check = open_check.access_check();
        let permission_flags = access_check.permission_flags();
        if !permission_flags.is_empty() {
            if flags.contains(OpenFlags::NOATIME) {
                self.check_o_noatime_allowed(current_task)?;
            }

            self.check_access(
                current_task,
                &namespace_node.mount,
                permission_flags,
                access_check.reason(),
                namespace_node,
            )?;
        }

        let (mode, rdev) = {
            // Don't hold the info lock while calling into open_device or self.ops().
            // TODO: The mode and rdev are immutable and shouldn't require a lock to read.
            let info = self.info();
            (info.mode, info.rdev)
        };

        match mode & FileMode::IFMT {
            FileMode::IFCHR => {
                if namespace_node.mount.flags().contains(MountFlags::NODEV) {
                    return error!(EACCES);
                }
                current_task.kernel().open_device(
                    current_task,
                    namespace_node,
                    flags,
                    rdev,
                    DeviceMode::Char,
                )
            }
            FileMode::IFBLK => {
                if namespace_node.mount.flags().contains(MountFlags::NODEV) {
                    return error!(EACCES);
                }
                current_task.kernel().open_device(
                    current_task,
                    namespace_node,
                    flags,
                    rdev,
                    DeviceMode::Block,
                )
            }
            FileMode::IFIFO => Pipe::open(current_task, self.fifo(current_task), flags),
            // UNIX domain sockets can't be opened.
            FileMode::IFSOCK => error!(ENXIO),
            _ => self.create_file_ops(current_task, flags),
        }
    }

    pub fn lookup(
        &self,
        current_task: &CurrentTask,
        mount: &MountInfo,
        name: &FsStr,
    ) -> Result<FsNodeHandle, Errno> {
        self.check_access(
            current_task,
            mount,
            Access::EXEC,
            CheckAccessReason::InternalPermissionChecks,
            &[Auditable::Name(name), std::panic::Location::caller().into()],
        )?;
        self.ops().lookup(self, current_task, name)
    }

    pub fn create_node(
        &self,
        current_task: &CurrentTask,
        mount: &MountInfo,
        name: &FsStr,
        mut mode: FileMode,
        dev: DeviceId,
        mut owner: FsCred,
    ) -> Result<FsNodeHandle, Errno> {
        assert!(
            !matches!(mode.fmt(), FileMode::EMPTY | FileMode::IFLNK),
            "create_node with missing or symlink node type"
        );

        self.check_access(
            current_task,
            mount,
            Access::WRITE,
            CheckAccessReason::InternalPermissionChecks,
            security::Auditable::Name(name),
        )?;

        if mode.is_dir() {
            // Even though the man page for mknod(2) says that mknod "cannot be used to create
            // directories" in starnix the mkdir syscall (`sys_mkdirat`) ends up calling
            // create_node.
            security::check_fs_node_mkdir_access(current_task, self, mode, name)?;
        } else {
            // https://man7.org/linux/man-pages/man2/mknod.2.html says on error EPERM:
            //
            //   mode requested creation of something other than a regular
            //   file, FIFO (named pipe), or UNIX domain socket, and the
            //   caller is not privileged (Linux: does not have the
            //   CAP_MKNOD capability); also returned if the filesystem
            //   containing pathname does not support the type of node
            //   requested.
            match mode.fmt() {
                FileMode::IFREG | FileMode::IFIFO | FileMode::IFSOCK => (),
                FileMode::IFCHR if dev == DeviceId::NONE => (),
                _ => security::check_task_capable(current_task, CAP_MKNOD)?,
            }

            if mode.is_reg() {
                security::check_fs_node_create_access(current_task, self, mode, name)?;
            } else {
                security::check_fs_node_mknod_access(current_task, self, mode, name, dev)?;
            }
        }

        // Propagate sticky bit(s) from parent directory to the child.
        self.update_metadata_for_child(current_task, &mut mode, &mut owner);

        // Delegate to the `ops` implementation to actually create the node.
        let new_node = if mode.is_dir() {
            self.ops().mkdir(self, current_task, name, mode, owner)?
        } else {
            self.ops().mknod(self, current_task, name, mode, dev, owner)?
        };

        // Allow the LSM to apply a security label to the new node.
        self.init_new_node_security_on_create(current_task, &new_node, name)?;

        Ok(new_node)
    }

    pub fn create_symlink(
        &self,
        current_task: &CurrentTask,
        mount: &MountInfo,
        name: &FsStr,
        target: &FsStr,
        owner: FsCred,
    ) -> Result<FsNodeHandle, Errno> {
        self.check_access(
            current_task,
            mount,
            Access::WRITE,
            CheckAccessReason::InternalPermissionChecks,
            security::Auditable::Name(name),
        )?;
        security::check_fs_node_symlink_access(current_task, self, name, target)?;

        let new_node = self.ops().create_symlink(self, current_task, name, target, owner)?;

        self.init_new_node_security_on_create(current_task, &new_node, name)?;

        Ok(new_node)
    }

    /// Requests that the LSM initialise a security label for the `new_node`, and optionally provide
    /// an extended attribute to write to the file to persist it.  If no LSM is enabled, no extended
    /// attribute returned, or if the filesystem does not support extended attributes, then the call
    /// returns success. All other failure modes return an `Errno` that should be early-returned.
    fn init_new_node_security_on_create(
        &self,
        current_task: &CurrentTask,
        new_node: &FsNode,
        name: &FsStr,
    ) -> Result<(), Errno> {
        security::fs_node_init_on_create(current_task, &new_node, self, name)?
            .map(|xattr| {
                match new_node.ops().set_xattr(
                    &new_node,
                    current_task,
                    xattr.name,
                    xattr.value.as_slice().into(),
                    XattrOp::Create,
                ) {
                    Err(e) => {
                        if e.code == ENOTSUP {
                            // This should only occur if a task has an "fscreate" context set, and
                            // creates a new file in a filesystem that does not support xattrs.
                            Ok(())
                        } else {
                            Err(e)
                        }
                    }
                    result => result,
                }
            })
            .unwrap_or_else(|| Ok(()))
    }

    pub fn create_tmpfile(
        &self,
        current_task: &CurrentTask,
        mount: &MountInfo,
        mut mode: FileMode,
        mut owner: FsCred,
        link_behavior: FsNodeLinkBehavior,
    ) -> Result<FsNodeHandle, Errno> {
        self.check_access(
            current_task,
            mount,
            Access::WRITE,
            CheckAccessReason::InternalPermissionChecks,
            security::Auditable::Location(std::panic::Location::caller()),
        )?;
        self.update_metadata_for_child(current_task, &mut mode, &mut owner);
        let node = self.ops().create_tmpfile(self, current_task, mode, owner)?;
        self.init_new_node_security_on_create(current_task, &node, "".into())?;
        if link_behavior == FsNodeLinkBehavior::Disallowed {
            node.ensure_rare_data().link_behavior.set(link_behavior).unwrap();
        }
        Ok(node)
    }

    // This method does not attempt to update the atime of the node.
    // Use `NamespaceNode::readlink` which checks the mount flags and updates the atime accordingly.
    pub fn readlink(&self, current_task: &CurrentTask) -> Result<SymlinkTarget, Errno> {
        // TODO: 378864856 - Is there a permission check here other than security checks?
        security::check_fs_node_read_link_access(current_task, self)?;
        self.ops().readlink(self, current_task)
    }

    pub fn link(
        &self,
        current_task: &CurrentTask,
        mount: &MountInfo,
        name: &FsStr,
        child: &FsNodeHandle,
    ) -> Result<FsNodeHandle, Errno> {
        self.check_access(
            current_task,
            mount,
            Access::WRITE,
            CheckAccessReason::InternalPermissionChecks,
            security::Auditable::Location(std::panic::Location::caller()),
        )?;

        if child.is_dir() {
            return error!(EPERM);
        }

        if let Some(child_rare_data) = child.rare_data.get() {
            if matches!(child_rare_data.link_behavior.get(), Some(FsNodeLinkBehavior::Disallowed)) {
                return error!(ENOENT);
            }
        }

        // Check that `current_task` has permission to create the hard link.
        //
        // See description of /proc/sys/fs/protected_hardlinks in
        // https://man7.org/linux/man-pages/man5/proc.5.html for details of the security
        // vulnerabilities.
        //
        let (child_uid, mode) = {
            let info = child.info();
            (info.uid, info.mode)
        };
        // Check that the the filesystem UID of the calling process (`current_task`) is the same as
        // the UID of the existing file. The check can be bypassed if the calling process has
        // `CAP_FOWNER` capability.
        if child_uid != current_task.current_creds().fsuid
            && !security::is_task_capable_noaudit(current_task, CAP_FOWNER)
        {
            // If current_task is not the user of the existing file, it needs to have read and write
            // access to the existing file.
            child
                .check_access(
                    current_task,
                    mount,
                    Access::READ | Access::WRITE,
                    CheckAccessReason::InternalPermissionChecks,
                    security::Auditable::Name(name),
                )
                .map_err(|e| {
                    // `check_access(..)` returns EACCES when the access rights doesn't match - change
                    // it to EPERM to match Linux standards.
                    if e == EACCES { errno!(EPERM) } else { e }
                })?;
            // There are also security issues that may arise when users link to setuid, setgid, or
            // special files.
            if mode.contains(FileMode::ISGID | FileMode::IXGRP) {
                return error!(EPERM);
            };
            if mode.contains(FileMode::ISUID) {
                return error!(EPERM);
            };
            if !mode.contains(FileMode::IFREG) {
                return error!(EPERM);
            };
        }

        security::check_fs_node_link_access(current_task, self, child)?;

        self.ops().link(self, current_task, name, child)?;
        Ok(child.clone())
    }

    pub fn unlink(
        &self,
        current_task: &CurrentTask,
        mount: &MountInfo,
        name: &FsStr,
        child: &FsNodeHandle,
    ) -> Result<(), Errno> {
        // The user must be able to search and write to the directory.
        self.check_access(
            current_task,
            mount,
            Access::EXEC | Access::WRITE,
            CheckAccessReason::InternalPermissionChecks,
            security::Auditable::Name(name),
        )?;
        {
            let parent_info = self.info();
            // Safe because we acquire the parent directory lock first, and then the child lock
            // inside check_sticky_bit. This parent -> child acquisition follows the
            // hierarchical lock ordering.
            let _token = allow_subclass();
            self.check_sticky_bit(current_task, child, &parent_info)?;
        }
        if child.is_dir() {
            security::check_fs_node_rmdir_access(current_task, self, child, name)?;
        } else {
            security::check_fs_node_unlink_access(current_task, self, child, name)?;
        }
        self.ops().unlink(self, current_task, name, child)?;
        self.update_ctime_mtime();
        Ok(())
    }

    pub fn truncate(
        &self,
        current_task: &CurrentTask,
        mount: &MountInfo,
        length: u64,
    ) -> Result<(), Errno> {
        if self.is_dir() {
            return error!(EISDIR);
        }
        self.check_access(
            current_task,
            mount,
            Access::WRITE,
            CheckAccessReason::InternalPermissionChecks,
            security::Auditable::Location(std::panic::Location::caller()),
        )?;

        let guard = self.ops().append_lock_write(self, current_task)?;
        self.truncate_locked(&guard, current_task, length)
    }

    /// Avoid calling this method directly. You probably want to call `FileObject::ftruncate()`
    /// which will also perform all file-descriptor based verifications.
    pub fn ftruncate(&self, current_task: &CurrentTask, length: u64) -> Result<(), Errno> {
        if self.is_dir() {
            // When truncating a file descriptor, if the descriptor references a directory,
            // return EINVAL. This is different from the truncate() syscall which returns EISDIR.
            //
            // See https://man7.org/linux/man-pages/man2/ftruncate.2.html#ERRORS
            return error!(EINVAL);
        }

        // For ftruncate, we do not need to check that the file node is writable.
        //
        // The file object that calls this method must verify that the file was opened
        // with write permissions.
        //
        // This matters because a file could be opened with O_CREAT + O_RDWR + 0444 mode.
        // The file descriptor returned from such an operation can be truncated, even
        // though the file was created with a read-only mode.
        //
        // See https://man7.org/linux/man-pages/man2/ftruncate.2.html#DESCRIPTION
        // which says:
        //
        // "With ftruncate(), the file must be open for writing; with truncate(),
        // the file must be writable."

        let guard = self.ops().append_lock_write(self, current_task)?;
        self.truncate_locked(&guard, current_task, length)
    }

    // Called by `truncate` and `ftruncate` above.
    pub fn truncate_locked(
        &self,
        guard: &AppendLockWriteGuard<'_>,
        current_task: &CurrentTask,
        length: u64,
    ) -> Result<(), Errno> {
        if length > MAX_LFS_FILESIZE as u64 {
            return error!(EINVAL);
        }
        if length > current_task.thread_group().get_rlimit(Resource::FSIZE) {
            send_standard_signal(current_task, SignalInfo::kernel(SIGXFSZ));
            return error!(EFBIG);
        }
        self.clear_suid_and_sgid_bits(current_task)?;

        self.ops().truncate(guard, self, current_task, length)?;
        self.update_ctime_mtime();
        Ok(())
    }

    /// Avoid calling this method directly. You probably want to call `FileObject::fallocate()`
    /// which will also perform additional verifications.
    pub fn fallocate(
        &self,
        current_task: &CurrentTask,
        mode: FallocMode,
        offset: u64,
        length: u64,
    ) -> Result<(), Errno> {
        let guard = self.ops().append_lock_write(self, current_task)?;
        self.fallocate_locked(&guard, current_task, mode, offset, length)
    }

    pub fn fallocate_locked(
        &self,
        guard: &AppendLockWriteGuard<'_>,
        current_task: &CurrentTask,
        mode: FallocMode,
        offset: u64,
        length: u64,
    ) -> Result<(), Errno> {
        let allocate_size = checked_add_offset_and_length(offset as usize, length as usize)
            .map_err(|_| errno!(EFBIG))? as u64;
        if allocate_size > current_task.thread_group().get_rlimit(Resource::FSIZE) {
            send_standard_signal(current_task, SignalInfo::kernel(SIGXFSZ));
            return error!(EFBIG);
        }

        self.clear_suid_and_sgid_bits(current_task)?;

        self.ops().allocate(guard, self, current_task, mode, offset, length)?;
        self.update_ctime_mtime();
        Ok(())
    }

    fn update_metadata_for_child(
        &self,
        current_task: &CurrentTask,
        mode: &mut FileMode,
        owner: &mut FsCred,
    ) {
        // The setgid bit on a directory causes the gid to be inherited by new children and the
        // setgid bit to be inherited by new child directories. See SetgidDirTest in gvisor.
        {
            let self_info = self.info();
            if self_info.mode.contains(FileMode::ISGID) {
                owner.gid = self_info.gid;
                if mode.is_dir() {
                    *mode |= FileMode::ISGID;
                }
            }
        }

        if !mode.is_dir() {
            // https://man7.org/linux/man-pages/man7/inode.7.html says:
            //
            //   For an executable file, the set-group-ID bit causes the
            //   effective group ID of a process that executes the file to change
            //   as described in execve(2).
            //
            // We need to check whether the current task has permission to create such a file.
            // See a similar check in `FsNode::chmod`.
            let current_creds = current_task.current_creds();
            if owner.gid != current_creds.fsgid
                && !current_creds.is_in_group(owner.gid)
                && !security::is_task_capable_noaudit(current_task, CAP_FOWNER)
            {
                *mode &= !FileMode::ISGID;
            }
        }
    }

    /// Checks if O_NOATIME is allowed,
    pub fn check_o_noatime_allowed(&self, current_task: &CurrentTask) -> Result<(), Errno> {
        // Per open(2),
        //
        //   O_NOATIME (since Linux 2.6.8)
        //      ...
        //
        //      This flag can be employed only if one of the following
        //      conditions is true:
        //
        //      *  The effective UID of the process matches the owner UID
        //         of the file.
        //
        //      *  The calling process has the CAP_FOWNER capability in
        //         its user namespace and the owner UID of the file has a
        //         mapping in the namespace.
        if current_task.current_creds().fsuid != self.info().uid {
            security::check_task_capable(current_task, CAP_FOWNER)?;
        }
        Ok(())
    }

    pub fn default_check_access_impl(
        &self,
        current_task: &CurrentTask,
        permission_flags: security::PermissionFlags,
        reason: CheckAccessReason,
        info: LockDepReadGuard<'_, FsNodeInfo>,
        audit_context: Auditable<'_>,
    ) -> Result<(), Errno> {
        let (node_uid, node_gid, mode) = (info.uid, info.gid, info.mode);
        std::mem::drop(info);
        if let CheckAccessReason::ChangeTimestamps { now } = reason {
            // To set the timestamps to the current time the caller must either have write access to
            // the file, be the file owner, or hold the CAP_DAC_OVERRIDE or CAP_FOWNER capability.
            // To set the timestamps to other values the caller must either be the file owner or hold
            // the CAP_FOWNER capability.
            if current_task.current_creds().fsuid == node_uid {
                return Ok(());
            }
            if now {
                if security::is_task_capable_noaudit(current_task, CAP_FOWNER) {
                    return Ok(());
                }
            } else {
                security::check_task_capable(current_task, CAP_FOWNER)?;
                return Ok(());
            }
        }
        check_access(self, current_task, permission_flags, node_uid, node_gid, mode)?;
        security::fs_node_permission(current_task, self, permission_flags, audit_context)
    }

    /// Check whether the node can be accessed in the current context with the specified access
    /// flags (read, write, or exec). Accounts for capabilities and whether the current user is the
    /// owner or is in the file's group.
    pub fn check_access<'a>(
        &self,
        current_task: &CurrentTask,
        mount: &MountInfo,
        access: impl Into<security::PermissionFlags>,
        reason: CheckAccessReason,
        audit_context: impl Into<security::Auditable<'a>>,
    ) -> Result<(), Errno> {
        let mut permission_flags = access.into();
        if permission_flags.contains(security::PermissionFlags::WRITE)
            && !self.info().mode.is_special()
        {
            mount.check_readonly_filesystem()?;
        }
        if permission_flags.contains(security::PermissionFlags::EXEC) && !self.is_dir() {
            mount.check_noexec_filesystem()?;
        }
        if reason == CheckAccessReason::Access {
            permission_flags |= PermissionFlags::ACCESS;
        }
        self.ops().check_access(
            self,
            current_task,
            permission_flags,
            &self.info,
            reason,
            audit_context.into(),
        )
    }

    /// Check whether the stick bit, `S_ISVTX`, forbids the `current_task` from removing the given
    /// `child`. If this node has `S_ISVTX`, then either the child must be owned by the `fsuid` of
    /// `current_task` or `current_task` must have `CAP_FOWNER`.
    pub fn check_sticky_bit(
        &self,
        current_task: &CurrentTask,
        child: &FsNodeHandle,
        self_info: &FsNodeInfo,
    ) -> Result<(), Errno> {
        if self_info.mode.contains(FileMode::ISVTX)
            && child.info().uid != current_task.current_creds().fsuid
        {
            security::check_task_capable(current_task, CAP_FOWNER)?;
        }
        Ok(())
    }

    pub fn fifo(&self, current_task: &CurrentTask) -> &PipeHandle {
        assert!(self.is_fifo());
        self.ensure_rare_data().ensure_fifo(current_task)
    }

    /// Returns the UNIX domain socket bound to this node, if any.
    pub fn bound_socket(&self) -> Option<&SocketHandle> {
        if let Some(rare_data) = self.rare_data.get() { rare_data.bound_socket.get() } else { None }
    }

    /// Register the provided socket as the UNIX domain socket bound to this node.
    ///
    /// It is a fatal error to call this method again if it has already been called on this node.
    pub fn set_bound_socket(&self, socket: SocketHandle) {
        assert!(self.ensure_rare_data().bound_socket.set(socket).is_ok());
    }

    /// Updates node attributes and persists changes via [`FsNodeOps::update_attributes`].
    ///
    /// For directory-level attributes like `casefold` and `wrapping_key_id`, callers should
    /// use directory helpers (such as [`DirEntry::set_casefold`]) to synchronize child caches
    /// and directory locks.
    pub fn update_attributes<F>(&self, current_task: &CurrentTask, mutator: F) -> Result<(), Errno>
    where
        F: FnOnce(&mut FsNodeInfo) -> Result<(), Errno>,
    {
        let mut info = self.info.write();
        let mut new_info = info.clone();
        mutator(&mut new_info)?;

        let new_access = new_info.mode.user_access()
            | new_info.mode.group_access()
            | new_info.mode.other_access();

        if new_access.intersects(Access::EXEC) {
            let write_guard_state = self.write_guard_state.lock();
            if let Ok(seals) = write_guard_state.get_seals() {
                if seals.contains(SealFlags::NO_EXEC) {
                    return error!(EPERM);
                }
            }
        }

        // `mutator`s should not update the attribute change time, which is managed by this API.
        assert_eq!(info.time_status_change, new_info.time_status_change);
        if *info == new_info {
            return Ok(());
        }
        new_info.time_status_change = utc::utc_now();

        let mut has = zxio_node_attr_has_t { ..Default::default() };
        has.modification_time = info.time_modify != new_info.time_modify;
        has.access_time = info.time_access != new_info.time_access;
        has.mode = info.mode != new_info.mode;
        has.uid = info.uid != new_info.uid;
        has.gid = info.gid != new_info.gid;
        has.rdev = info.rdev != new_info.rdev;
        has.casefold = info.casefold != new_info.casefold;
        has.wrapping_key_id = info.wrapping_key_id != new_info.wrapping_key_id;

        if has.casefold && !self.fs().has_casefold_support() {
            return error!(ENOTSUP);
        }
        security::check_fs_node_setattr_access(current_task, &self, &has)?;

        // Call `update_attributes(..)` to persist the changes for the following fields.
        if has.modification_time
            || has.access_time
            || has.mode
            || has.uid
            || has.gid
            || has.rdev
            || has.casefold
            || has.wrapping_key_id
        {
            self.ops().update_attributes(self, current_task, &new_info, has)?;
        }

        *info = new_info;
        Ok(())
    }

    /// Set the permissions on this FsNode to the given values.
    ///
    /// Does not change the IFMT of the node.
    pub fn chmod(
        &self,
        current_task: &CurrentTask,
        mount: &MountInfo,
        mut mode: FileMode,
    ) -> Result<(), Errno> {
        mount.check_readonly_filesystem()?;
        self.update_attributes(current_task, |info| {
            let current_creds = current_task.current_creds();
            if info.uid != current_creds.euid {
                security::check_task_capable(current_task, CAP_FOWNER)?;
            } else if info.gid != current_creds.egid
                && !current_creds.is_in_group(info.gid)
                && mode.intersects(FileMode::ISGID)
                && !security::is_task_capable_noaudit(current_task, CAP_FOWNER)
            {
                mode &= !FileMode::ISGID;
            }
            info.chmod(mode);
            Ok(())
        })
    }

    /// Sets the owner and/or group on this FsNode.
    pub fn chown(
        &self,
        current_task: &CurrentTask,
        mount: &MountInfo,
        owner: Option<uid_t>,
        group: Option<gid_t>,
    ) -> Result<(), Errno> {
        mount.check_readonly_filesystem()?;
        self.update_attributes(current_task, |info| {
            if security::is_task_capable_noaudit(current_task, CAP_CHOWN) {
                info.chown(owner, group);
                return Ok(());
            }

            // Nobody can change the owner.
            if let Some(uid) = owner {
                if info.uid != uid {
                    return error!(EPERM);
                }
            }

            let (euid, is_in_group) = {
                let current_creds = current_task.current_creds();
                (current_creds.euid, group.map(|gid| current_creds.is_in_group(gid)))
            };

            // The owner can change the group.
            if info.uid == euid {
                // To a group that it belongs.
                if let Some(is_in_group) = is_in_group {
                    if !is_in_group {
                        return error!(EPERM);
                    }
                }
                info.chown(None, group);
                return Ok(());
            }

            // Any other user can call chown(file, -1, -1)
            if owner.is_some() || group.is_some() {
                return error!(EPERM);
            }

            // But not on set-user-ID or set-group-ID files.
            // If we were to chown them, they would drop the set-ID bit.
            if info.mode.is_reg()
                && (info.mode.contains(FileMode::ISUID)
                    || info.mode.contains(FileMode::ISGID | FileMode::IXGRP))
            {
                return error!(EPERM);
            }

            info.chown(None, None);
            Ok(())
        })
    }

    /// Forcefully change the owner and group of this node.
    ///
    /// # Safety
    ///
    /// This function skips all the security checks and just updates the owner and group. Also, does
    /// not check if the filesystem is read-only and does not update the attribute change time.
    ///
    /// This function is used to set the owner and group of /proc/pid to the credentials of the
    /// current task. Please consider carefully whether you want to use this function for another
    /// purpose.
    pub unsafe fn force_chown(&self, creds: FsCred) {
        self.update_info(|info| {
            info.chown(Some(creds.uid), Some(creds.gid));
        });
    }

    /// Whether this node is a regular file.
    pub fn is_reg(&self) -> bool {
        self.info().mode.is_reg()
    }

    /// Whether this node is a directory.
    pub fn is_dir(&self) -> bool {
        self.info().mode.is_dir()
    }

    /// Whether this node is a socket.
    pub fn is_sock(&self) -> bool {
        self.info().mode.is_sock()
    }

    /// Whether this node is a FIFO.
    pub fn is_fifo(&self) -> bool {
        self.info().mode.is_fifo()
    }

    /// Whether this node is a symbolic link.
    pub fn is_lnk(&self) -> bool {
        self.info().mode.is_lnk()
    }

    pub fn dev(&self) -> DeviceId {
        self.fs().dev_id
    }

    pub fn stat(&self, current_task: &CurrentTask) -> Result<uapi::stat, Errno> {
        security::check_fs_node_getattr_access(current_task, self)?;

        let info = self.fetch_and_refresh_info(current_task)?;

        let time_to_kernel_timespec_pair = |t| {
            let timespec { tv_sec, tv_nsec } = timespec_from_time(t);
            let time = tv_sec.try_into().map_err(|_| errno!(EINVAL))?;
            let time_nsec = tv_nsec.try_into().map_err(|_| errno!(EINVAL))?;
            Ok((time, time_nsec))
        };

        let (st_atime, st_atime_nsec) = time_to_kernel_timespec_pair(info.time_access)?;
        let (st_mtime, st_mtime_nsec) = time_to_kernel_timespec_pair(info.time_modify)?;
        let (st_ctime, st_ctime_nsec) = time_to_kernel_timespec_pair(info.time_status_change)?;

        Ok(uapi::stat {
            st_dev: self.dev().bits(),
            st_ino: self.ino,
            st_nlink: info.link_count.try_into().map_err(|_| errno!(EINVAL))?,
            st_mode: info.mode.bits(),
            st_uid: info.uid,
            st_gid: info.gid,
            st_rdev: info.rdev.bits(),
            st_size: info.size.try_into().map_err(|_| errno!(EINVAL))?,
            st_blksize: info.blksize.try_into().map_err(|_| errno!(EINVAL))?,
            st_blocks: info.blocks.try_into().map_err(|_| errno!(EINVAL))?,
            st_atime,
            st_atime_nsec,
            st_mtime,
            st_mtime_nsec,
            st_ctime,
            st_ctime_nsec,
            ..Default::default()
        })
    }

    /// Returns the current size of the file.  This is inherently racy, so any caller that
    /// might want to use the value returned should hold their own locks if necessary.  For
    /// example, if using the value here to implement append (which is the case at the time
    /// of writing this comment), locks must be held to prevent the file size being changed
    /// concurrently.
    // TODO(https://fxbug.dev/454730248): This is probably the wrong way to implement O_APPEND.
    pub fn get_size(&self, current_task: &CurrentTask) -> Result<usize, Errno> {
        self.ops().get_size(self, current_task)
    }

    fn statx_timestamp_from_time(time: UtcInstant) -> statx_timestamp {
        let nanos = time.into_nanos();
        statx_timestamp {
            tv_sec: nanos / NANOS_PER_SECOND,
            tv_nsec: (nanos % NANOS_PER_SECOND) as u32,
            ..Default::default()
        }
    }

    pub fn statx(
        &self,
        current_task: &CurrentTask,
        flags: StatxFlags,
        mask: u32,
    ) -> Result<statx, Errno> {
        security::check_fs_node_getattr_access(current_task, self)?;

        // Ignore mask for now and fill in all of the fields.
        let info = if flags.contains(StatxFlags::AT_STATX_DONT_SYNC) {
            self.info()
        } else {
            self.fetch_and_refresh_info(current_task)?
        };
        if mask & STATX__RESERVED == STATX__RESERVED {
            return error!(EINVAL);
        }

        track_stub!(TODO("https://fxbug.dev/302594110"), "statx attributes");
        let stx_mnt_id = 0;
        let mut stx_attributes = 0;
        let stx_attributes_mask = STATX_ATTR_VERITY as u64;

        if matches!(*self.fsverity.lock(), FsVerityState::FsVerity) {
            stx_attributes |= STATX_ATTR_VERITY as u64;
        }

        Ok(statx {
            stx_mask: STATX_NLINK
                | STATX_UID
                | STATX_GID
                | STATX_ATIME
                | STATX_MTIME
                | STATX_CTIME
                | STATX_INO
                | STATX_SIZE
                | STATX_BLOCKS
                | STATX_BASIC_STATS,
            stx_blksize: info.blksize.try_into().map_err(|_| errno!(EINVAL))?,
            stx_attributes,
            stx_nlink: info.link_count.try_into().map_err(|_| errno!(EINVAL))?,
            stx_uid: info.uid,
            stx_gid: info.gid,
            stx_mode: info.mode.bits().try_into().map_err(|_| errno!(EINVAL))?,
            stx_ino: self.ino,
            stx_size: info.size.try_into().map_err(|_| errno!(EINVAL))?,
            stx_blocks: info.blocks.try_into().map_err(|_| errno!(EINVAL))?,
            stx_attributes_mask,
            stx_ctime: Self::statx_timestamp_from_time(info.time_status_change),
            stx_mtime: Self::statx_timestamp_from_time(info.time_modify),
            stx_atime: Self::statx_timestamp_from_time(info.time_access),

            stx_rdev_major: info.rdev.major(),
            stx_rdev_minor: info.rdev.minor(),

            stx_dev_major: self.fs().dev_id.major(),
            stx_dev_minor: self.fs().dev_id.minor(),
            stx_mnt_id,
            ..Default::default()
        })
    }

    /// Checks whether `current_task` has capabilities required for the specified `access` to the
    /// extended attribute `name`.
    fn check_xattr_access(
        &self,
        current_task: &CurrentTask,
        mount: &MountInfo,
        name: &FsStr,
        access: Access,
    ) -> Result<(), Errno> {
        assert!(access == Access::READ || access == Access::WRITE);

        let enodata_if_read =
            |e: Errno| if access == Access::READ && e.code == EPERM { errno!(ENODATA) } else { e };

        // man xattr(7) describes the different access checks applied to each extended attribute
        // namespace.
        if name.starts_with(XATTR_USER_PREFIX.to_bytes()) {
            {
                let info = self.info();
                if !info.mode.is_reg() && !info.mode.is_dir() {
                    return Err(enodata_if_read(errno!(EPERM)));
                }
            }

            // TODO: https://fxbug.dev/460734830 - Perform capability check(s) if file has sticky
            // bit set.

            self.check_access(
                current_task,
                mount,
                access,
                CheckAccessReason::InternalPermissionChecks,
                security::Auditable::Name(name),
            )?;
        } else if name.starts_with(XATTR_TRUSTED_PREFIX.to_bytes()) {
            // Trusted extended attributes require `CAP_SYS_ADMIN` to read or write.
            security::check_task_capable(current_task, CAP_SYS_ADMIN).map_err(enodata_if_read)?;
        } else if name.starts_with(XATTR_SYSTEM_PREFIX.to_bytes()) {
            // System extended attributes have attribute-specific access policy.
            // TODO: https://fxbug.dev/460734830 -  Revise how system extended attributes are
            // access-controlled.
            security::check_task_capable(current_task, CAP_SYS_ADMIN).map_err(enodata_if_read)?;
        } else if name.starts_with(XATTR_SECURITY_PREFIX.to_bytes()) {
            if access == Access::WRITE {
                // Writes require `CAP_SYS_ADMIN`, unless the LSM owning `name` specifies to skip.
                if !security::fs_node_xattr_skipcap(name) {
                    security::check_task_capable(current_task, CAP_SYS_ADMIN)
                        .map_err(enodata_if_read)?;
                }
            }
        } else {
            panic!("Unknown extended attribute prefix: {}", name);
        }
        Ok(())
    }

    pub fn get_xattr(
        &self,
        current_task: &CurrentTask,
        mount: &MountInfo,
        name: &FsStr,
        max_size: usize,
    ) -> Result<ValueOrSize<FsString>, Errno> {
        // Perform discretionary capability & access checks appropriate to the xattr prefix.
        self.check_xattr_access(current_task, mount, name, Access::READ)?;

        // LSM access checks must be performed after discretionary checks.
        security::check_fs_node_getxattr_access(current_task, self, name)?;

        if name.starts_with(XATTR_SECURITY_PREFIX.to_bytes()) {
            // If the attribute is in the security.* domain then allow the LSM to handle the
            // request, or to delegate to `FsNodeOps::get_xattr()`.
            security::fs_node_getsecurity(current_task, self, name, max_size)
        } else {
            // If the attribute is outside security.*, delegate the read to the `FsNodeOps`.
            self.ops().get_xattr(self, current_task, name, max_size)
        }
    }

    pub fn set_xattr(
        &self,
        current_task: &CurrentTask,
        mount: &MountInfo,
        name: &FsStr,
        value: &FsStr,
        op: XattrOp,
    ) -> Result<(), Errno> {
        // Perform discretionary capability & access checks appropriate to the xattr prefix.
        self.check_xattr_access(current_task, mount, name, Access::WRITE)?;

        // LSM access checks must be performed after discretionary checks.
        security::check_fs_node_setxattr_access(current_task, self, name, value, op)?;

        if name.starts_with(XATTR_SECURITY_PREFIX.to_bytes()) {
            // If the attribute is in the security.* domain then allow the LSM to handle the
            // request, or to delegate to `FsNodeOps::set_xattr()`.
            security::fs_node_setsecurity(current_task, self, name, value, op)
        } else {
            // If the attribute is outside security.*, delegate the read to the `FsNodeOps`.
            self.ops().set_xattr(self, current_task, name, value, op)
        }
    }

    pub fn remove_xattr(
        &self,
        current_task: &CurrentTask,
        mount: &MountInfo,
        name: &FsStr,
    ) -> Result<(), Errno> {
        // Perform discretionary capability & access checks appropriate to the xattr prefix.
        self.check_xattr_access(current_task, mount, name, Access::WRITE)?;

        // LSM access checks must be performed after discretionary checks.
        security::check_fs_node_removexattr_access(current_task, self, name)?;
        self.ops().remove_xattr(self, current_task, name)
    }

    pub fn list_xattrs(
        &self,
        current_task: &CurrentTask,
        max_size: usize,
    ) -> Result<ValueOrSize<Vec<FsString>>, Errno> {
        security::check_fs_node_listxattr_access(current_task, self)?;
        Ok(self.ops().list_xattrs(self, current_task, max_size)?.map(|mut v| {
            // Extended attributes may be listed even if the caller would not be able to read
            // (or modify) the attribute's value.
            // trusted.* attributes are only accessible with CAP_SYS_ADMIN and are omitted by
            // `listxattr()` unless the caller holds CAP_SYS_ADMIN.
            if !security::is_task_capable_noaudit(current_task, CAP_SYS_ADMIN) {
                v.retain(|name| !name.starts_with(XATTR_TRUSTED_PREFIX.to_bytes()));
            }
            v
        }))
    }

    /// Returns current `FsNodeInfo`.
    pub fn info(&self) -> LockDepReadGuard<'_, FsNodeInfo> {
        self.info.read()
    }

    /// Returns a reference to the `info` lock itself.
    ///
    /// This should ONLY be used by `RenameGuard` to perform ordered write locking on independent
    /// nodes.
    pub(super) fn info_lock(&self) -> &DynamicLockDepRwLock<FsNodeInfo> {
        &self.info
    }

    /// Refreshes the `FsNodeInfo` if necessary and returns a read guard.
    pub fn fetch_and_refresh_info(
        &self,
        current_task: &CurrentTask,
    ) -> Result<LockDepReadGuard<'_, FsNodeInfo>, Errno> {
        self.ops().fetch_and_refresh_info(self, current_task, &self.info)
    }

    pub fn update_info<F, T>(&self, mutator: F) -> T
    where
        F: FnOnce(&mut FsNodeInfo) -> T,
    {
        let mut info = self.info.write();
        mutator(&mut info)
    }

    /// Clear the SUID and SGID bits unless the `current_task` has `CAP_FSETID`
    pub fn clear_suid_and_sgid_bits(&self, current_task: &CurrentTask) -> Result<(), Errno> {
        if !self.info().has_suid_or_sgid_bits() {
            return Ok(());
        }
        self.update_attributes(current_task, |info| {
            if info.has_suid_or_sgid_bits()
                && !security::is_task_capable_noaudit(current_task, CAP_FSETID)
            {
                info.clear_suid_and_sgid_bits();
            }
            Ok(())
        })
    }

    /// Update the ctime and mtime of a file to now.
    pub fn update_ctime_mtime(&self) {
        if self.fs().manages_timestamps() {
            return;
        }
        self.update_info(|info| {
            let now = utc::utc_now();
            info.time_status_change = now;
            info.time_modify = now;
        });
    }

    /// Update the ctime of a file to now.
    pub fn update_ctime(&self) {
        if self.fs().manages_timestamps() {
            return;
        }
        self.update_info(|info| {
            let now = utc::utc_now();
            info.time_status_change = now;
        });
    }

    /// Update the atime and mtime if the `current_task` has write access, is the file owner, or
    /// holds either the CAP_DAC_OVERRIDE or CAP_FOWNER capability.
    pub fn update_atime_mtime(
        &self,
        current_task: &CurrentTask,
        mount: &MountInfo,
        atime: TimeUpdateType,
        mtime: TimeUpdateType,
    ) -> Result<(), Errno> {
        // If the filesystem is read-only, this always fail.
        mount.check_readonly_filesystem()?;

        let now = matches!((atime, mtime), (TimeUpdateType::Now, TimeUpdateType::Now));
        self.check_access(
            current_task,
            mount,
            Access::WRITE,
            CheckAccessReason::ChangeTimestamps { now },
            security::Auditable::Location(std::panic::Location::caller()),
        )?;

        if !matches!((atime, mtime), (TimeUpdateType::Omit, TimeUpdateType::Omit)) {
            // This function is called by `utimes(..)` which will update the access and
            // modification time. We need to call `update_attributes()` to update the mtime of
            // filesystems that manages file timestamps.
            self.update_attributes(current_task, |info| {
                let now = utc::utc_now();
                let get_time = |time: TimeUpdateType| match time {
                    TimeUpdateType::Now => Some(now),
                    TimeUpdateType::Time(t) => Some(t),
                    TimeUpdateType::Omit => None,
                };
                if let Some(time) = get_time(atime) {
                    info.time_access = time;
                }
                if let Some(time) = get_time(mtime) {
                    info.time_modify = time;
                }
                Ok(())
            })?;
        }
        Ok(())
    }

    /// The key used to identify this node in the file system's node cache.
    ///
    /// For many file systems, this will be the same as the inode number. However, some file
    /// systems, such as FUSE, sometimes use different `node_key` and inode numbers.
    pub fn node_key(&self) -> ino_t {
        self.ops().node_key(self)
    }

    fn ensure_rare_data(&self) -> &FsNodeRareData {
        self.rare_data.get_or_init(|| Box::new(FsNodeRareData::default()))
    }

    /// Returns the set of watchers for this node.
    ///
    /// Only call this function if you require this node to actually store a list of watchers. If
    /// you just wish to notify any watchers that might exist, please use `notify` instead.
    pub fn ensure_watchers(&self) -> &inotify_hook::InotifyWatchers {
        &self.ensure_rare_data().watchers
    }

    /// Notify the watchers of the given event.
    pub fn notify(
        &self,
        event_mask: InotifyMask,
        cookie: u32,
        name: &FsStr,
        mode: FileMode,
        is_dead: bool,
    ) {
        if let Some(rare_data) = self.rare_data.get() {
            let kernel = self.fs().kernel.upgrade().expect("kernel is dead");
            if let Some(hook) = kernel.expando.peek::<Arc<dyn inotify_hook::NotifyHook>>() {
                hook.notify(&rare_data.watchers, event_mask, cookie, name, mode, is_dead);
            }
        }
    }

    /// Calls through to the filesystem to enable fs-verity on this file.
    pub fn enable_fsverity(
        &self,
        current_task: &CurrentTask,
        descriptor: &fsverity_descriptor,
    ) -> Result<(), Errno> {
        self.ops().enable_fsverity(self, current_task, descriptor)
    }
}

impl std::fmt::Debug for FsNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FsNode")
            .field("fs", &self.fs().name())
            .field("info", &*self.info())
            .field("ops_ty", &self.ops().type_name())
            .finish()
    }
}

impl Releasable for FsNode {
    type Context<'a> = &'a CurrentTask;

    fn release<'a>(self, context: &'a CurrentTask) {
        let current_task = context;
        if let Some(fs) = self.fs.upgrade() {
            fs.remove_node(&self);
        }
        if let Err(err) = self.ops.forget(current_task, self.info.into_inner()) {
            log_error!("Error on FsNodeOps::forget: {err:?}");
        }
    }
}

fn check_access(
    fs_node: &FsNode,
    current_task: &CurrentTask,
    permission_flags: security::PermissionFlags,
    node_uid: uid_t,
    node_gid: gid_t,
    mode: FileMode,
) -> Result<(), Errno> {
    // Determine which of the access bits apply to the `current_task`.
    let (fsuid, is_in_group) = {
        let current_creds = current_task.current_creds();
        (current_creds.fsuid, current_creds.is_in_group(node_gid))
    };
    let granted = if fsuid == node_uid {
        mode.user_access()
    } else if is_in_group {
        mode.group_access()
    } else {
        mode.other_access()
    };

    let access = permission_flags.as_access();
    if granted.contains(access) {
        return Ok(());
    }

    // Callers with CAP_DAC_READ_SEARCH override can read files & directories, and traverse
    // directories to which they lack permission.
    let mut requested = access & !granted;

    // If this check was triggered by `access()`, or a variant, then check for a `dontaudit`
    // statement for the `audit_access` permission for this caller & file.
    let have_dont_audit = OnceBool::new();
    let has_capability = move |current_task, capability| {
        let dont_audit = have_dont_audit.get_or_init(|| {
            permission_flags.contains(PermissionFlags::ACCESS)
                && security::has_dontaudit_access(current_task, fs_node)
        });
        if dont_audit {
            security::is_task_capable_noaudit(current_task, capability)
        } else {
            security::check_task_capable(current_task, capability).is_ok()
        }
    };

    // CAP_DAC_READ_SEARCH allows bypass of read checks, and directory traverse (eXecute) checks.
    let dac_read_search_access =
        if mode.is_dir() { Access::READ | Access::EXEC } else { Access::READ };
    if dac_read_search_access.intersects(requested)
        && has_capability(current_task, CAP_DAC_READ_SEARCH)
    {
        requested.remove(dac_read_search_access);
    }
    if requested.is_empty() {
        return Ok(());
    }

    // CAP_DAC_OVERRIDE allows bypass of all checks (though see the comment for file-execute).
    let mut dac_override_access = Access::READ | Access::WRITE;
    dac_override_access |= if mode.is_dir() {
        Access::EXEC
    } else {
        // File execute access checks may not be bypassed unless at least one executable bit is set.
        (mode.user_access() | mode.group_access() | mode.other_access()) & Access::EXEC
    };
    if dac_override_access.intersects(requested) && has_capability(current_task, CAP_DAC_OVERRIDE) {
        requested.remove(dac_override_access);
    }
    if requested.is_empty() {
        return Ok(());
    }

    return error!(EACCES);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::mem::mem_device_init;
    use crate::testing::*;
    use crate::vfs::buffers::VecOutputBuffer;
    use starnix_uapi::auth::Credentials;
    use starnix_uapi::file_mode::mode;

    #[::fuchsia::test]
    async fn open_device_file() {
        spawn_kernel_and_run(async |current_task| {
            mem_device_init(current_task.kernel()).expect("mem_device_init");

            // Create a device file that points to the `zero` device (which is automatically
            // registered in the kernel).
            current_task
                .fs()
                .root()
                .create_node(&current_task, "zero".into(), mode!(IFCHR, 0o666), DeviceId::ZERO)
                .expect("create_node");

            const CONTENT_LEN: usize = 10;
            let mut buffer = VecOutputBuffer::new(CONTENT_LEN);

            // Read from the zero device.
            let device_file =
                current_task.open_file("zero".into(), OpenFlags::RDONLY).expect("open device file");
            device_file.read(&current_task, &mut buffer).expect("read from zero");

            // Assert the contents.
            assert_eq!(&[0; CONTENT_LEN], buffer.data());
        })
        .await;
    }

    #[::fuchsia::test]
    async fn node_info_is_reflected_in_stat() {
        spawn_kernel_and_run(async |current_task| {
            // Create a node.
            let node = &current_task
                .fs()
                .root()
                .create_node(&current_task, "zero".into(), FileMode::IFCHR, DeviceId::ZERO)
                .expect("create_node")
                .entry
                .node;
            node.update_info(|info| {
                info.mode = FileMode::IFSOCK;
                info.size = 1;
                info.blocks = 2;
                info.blksize = 4;
                info.uid = 9;
                info.gid = 10;
                info.link_count = 11;
                info.time_status_change = UtcInstant::from_nanos(1);
                info.time_access = UtcInstant::from_nanos(2);
                info.time_modify = UtcInstant::from_nanos(3);
                info.rdev = DeviceId::new(13, 13);
            });
            let stat = node.stat(&current_task).expect("stat");

            assert_eq!(stat.st_mode, FileMode::IFSOCK.bits());
            assert_eq!(stat.st_size, 1);
            assert_eq!(stat.st_blksize, 4);
            assert_eq!(stat.st_blocks, 2);
            assert_eq!(stat.st_uid, 9);
            assert_eq!(stat.st_gid, 10);
            assert_eq!(stat.st_nlink, 11);
            assert_eq!(stat.st_ctime, 0);
            assert_eq!(stat.st_ctime_nsec, 1);
            assert_eq!(stat.st_atime, 0);
            assert_eq!(stat.st_atime_nsec, 2);
            assert_eq!(stat.st_mtime, 0);
            assert_eq!(stat.st_mtime_nsec, 3);
            assert_eq!(stat.st_rdev, DeviceId::new(13, 13).bits());
        })
        .await;
    }

    #[::fuchsia::test]
    fn test_flock_operation() {
        assert!(FlockOperation::from_flags(0).is_err());
        assert!(FlockOperation::from_flags(u32::MAX).is_err());

        let operation1 = FlockOperation::from_flags(LOCK_SH).expect("from_flags");
        assert!(!operation1.is_unlock());
        assert!(!operation1.is_lock_exclusive());
        assert!(operation1.is_blocking());

        let operation2 = FlockOperation::from_flags(LOCK_EX | LOCK_NB).expect("from_flags");
        assert!(!operation2.is_unlock());
        assert!(operation2.is_lock_exclusive());
        assert!(!operation2.is_blocking());

        let operation3 = FlockOperation::from_flags(LOCK_UN).expect("from_flags");
        assert!(operation3.is_unlock());
        assert!(!operation3.is_lock_exclusive());
        assert!(operation3.is_blocking());
    }

    #[::fuchsia::test]
    async fn test_check_access() {
        spawn_kernel_and_run(async |current_task| {
            let mut creds = Credentials::with_ids(1, 2);
            creds.groups = vec![3, 4];
            current_task.set_creds(creds);

            // Create a node.
            let node = &current_task
                .fs()
                .root()
                .create_node(&current_task, "foo".into(), FileMode::IFREG, DeviceId::NONE)
                .expect("create_node")
                .entry
                .node;
            let check_access = |uid: uid_t, gid: gid_t, perm: u32, access: Access| {
                node.update_info(|info| {
                    info.mode = mode!(IFREG, perm);
                    info.uid = uid;
                    info.gid = gid;
                });
                node.check_access(
                    &current_task,
                    &MountInfo::detached(),
                    access,
                    CheckAccessReason::InternalPermissionChecks,
                    security::Auditable::Location(std::panic::Location::caller()),
                )
            };

            assert_eq!(check_access(0, 0, 0o700, Access::EXEC), error!(EACCES));
            assert_eq!(check_access(0, 0, 0o700, Access::READ), error!(EACCES));
            assert_eq!(check_access(0, 0, 0o700, Access::WRITE), error!(EACCES));

            assert_eq!(check_access(0, 0, 0o070, Access::EXEC), error!(EACCES));
            assert_eq!(check_access(0, 0, 0o070, Access::READ), error!(EACCES));
            assert_eq!(check_access(0, 0, 0o070, Access::WRITE), error!(EACCES));

            assert_eq!(check_access(0, 0, 0o007, Access::EXEC), Ok(()));
            assert_eq!(check_access(0, 0, 0o007, Access::READ), Ok(()));
            assert_eq!(check_access(0, 0, 0o007, Access::WRITE), Ok(()));

            assert_eq!(check_access(1, 0, 0o700, Access::EXEC), Ok(()));
            assert_eq!(check_access(1, 0, 0o700, Access::READ), Ok(()));
            assert_eq!(check_access(1, 0, 0o700, Access::WRITE), Ok(()));

            assert_eq!(check_access(1, 0, 0o100, Access::EXEC), Ok(()));
            assert_eq!(check_access(1, 0, 0o100, Access::READ), error!(EACCES));
            assert_eq!(check_access(1, 0, 0o100, Access::WRITE), error!(EACCES));

            assert_eq!(check_access(1, 0, 0o200, Access::EXEC), error!(EACCES));
            assert_eq!(check_access(1, 0, 0o200, Access::READ), error!(EACCES));
            assert_eq!(check_access(1, 0, 0o200, Access::WRITE), Ok(()));

            assert_eq!(check_access(1, 0, 0o400, Access::EXEC), error!(EACCES));
            assert_eq!(check_access(1, 0, 0o400, Access::READ), Ok(()));
            assert_eq!(check_access(1, 0, 0o400, Access::WRITE), error!(EACCES));

            assert_eq!(check_access(0, 2, 0o700, Access::EXEC), error!(EACCES));
            assert_eq!(check_access(0, 2, 0o700, Access::READ), error!(EACCES));
            assert_eq!(check_access(0, 2, 0o700, Access::WRITE), error!(EACCES));

            assert_eq!(check_access(0, 2, 0o070, Access::EXEC), Ok(()));
            assert_eq!(check_access(0, 2, 0o070, Access::READ), Ok(()));
            assert_eq!(check_access(0, 2, 0o070, Access::WRITE), Ok(()));

            assert_eq!(check_access(0, 3, 0o070, Access::EXEC), Ok(()));
            assert_eq!(check_access(0, 3, 0o070, Access::READ), Ok(()));
            assert_eq!(check_access(0, 3, 0o070, Access::WRITE), Ok(()));
        })
        .await;
    }

    #[::fuchsia::test]
    async fn set_security_xattr_fails_without_security_module_or_root() {
        spawn_kernel_and_run(async |current_task| {
            let mut creds = Credentials::with_ids(1, 2);
            creds.groups = vec![3, 4];
            current_task.set_creds(creds);

            // Create a node.
            let node = &current_task
                .fs()
                .root()
                .create_node(&current_task, "foo".into(), FileMode::IFREG, DeviceId::NONE)
                .expect("create_node")
                .entry
                .node;

            // Give read-write-execute access.
            node.update_info(|info| info.mode = mode!(IFREG, 0o777));

            // Without a security module, and without CAP_SYS_ADMIN capabilities, setting the xattr
            // should fail.
            assert_eq!(
                node.set_xattr(
                    &current_task,
                    &MountInfo::detached(),
                    "security.name".into(),
                    "security_label".into(),
                    XattrOp::Create,
                ),
                error!(EPERM)
            );
        })
        .await;
    }

    #[::fuchsia::test]
    async fn set_non_user_xattr_fails_without_security_module_or_root() {
        spawn_kernel_and_run(async |current_task| {
            let mut creds = Credentials::with_ids(1, 2);
            creds.groups = vec![3, 4];
            current_task.set_creds(creds);

            // Create a node.
            let node = &current_task
                .fs()
                .root()
                .create_node(&current_task, "foo".into(), FileMode::IFREG, DeviceId::NONE)
                .expect("create_node")
                .entry
                .node;

            // Give read-write-execute access.
            node.update_info(|info| info.mode = mode!(IFREG, 0o777));

            // Without a security module, and without CAP_SYS_ADMIN capabilities, setting the xattr
            // should fail.
            assert_eq!(
                node.set_xattr(
                    &current_task,
                    &MountInfo::detached(),
                    "trusted.name".into(),
                    "some data".into(),
                    XattrOp::Create,
                ),
                error!(EPERM)
            );
        })
        .await;
    }

    #[::fuchsia::test]
    async fn get_security_xattr_succeeds_without_read_access() {
        spawn_kernel_and_run(async |current_task| {
            let mut creds = Credentials::with_ids(1, 2);
            creds.groups = vec![3, 4];
            current_task.set_creds(creds);

            // Create a node.
            let node = &current_task
                .fs()
                .root()
                .create_node(&current_task, "foo".into(), FileMode::IFREG, DeviceId::NONE)
                .expect("create_node")
                .entry
                .node;

            // Only give read access to the root and give root access to the current task.
            node.update_info(|info| info.mode = mode!(IFREG, 0o100));
            current_task.set_creds(Credentials::with_ids(0, 0));

            // Setting the label should succeed even without write access to the file.
            assert_eq!(
                node.set_xattr(
                    &current_task,
                    &MountInfo::detached(),
                    "security.name".into(),
                    "security_label".into(),
                    XattrOp::Create,
                ),
                Ok(())
            );

            // Remove root access from the current task.
            current_task.set_creds(Credentials::with_ids(1, 1));

            // Getting the label should succeed even without read access to the file.
            assert_eq!(
                node.get_xattr(&current_task, &MountInfo::detached(), "security.name".into(), 4096),
                Ok(ValueOrSize::Value("security_label".into()))
            );
        })
        .await;
    }

    #[fuchsia::test]
    async fn test_casefold_not_supported_by_default() {
        spawn_kernel_and_run(async |current_task| {
            let file_node = &current_task
                .fs()
                .root()
                .create_node(&current_task, "foo".into(), FileMode::IFREG, DeviceId::NONE)
                .expect("create_node")
                .entry;

            assert!(!file_node.node.fs().has_casefold_support());
            assert_eq!(
                file_node.node.update_attributes(&current_task, |info| {
                    info.casefold = true;
                    Ok(())
                }),
                error!(ENOTSUP)
            );
            // Calling set_casefold on a regular file returns Ok(()) when casefold is already false,
            // and ENOTDIR when attempting to enable casefolding.
            assert_eq!(file_node.set_casefold(&current_task, false), Ok(()));
            assert_eq!(file_node.set_casefold(&current_task, true), error!(ENOTDIR));

            // Calling set_casefold on a directory in an unsupported filesystem returns ENOTSUP.
            let dir_entry = &current_task
                .fs()
                .root()
                .create_node(&current_task, "dir".into(), FileMode::IFDIR, DeviceId::NONE)
                .expect("create_dir")
                .entry;
            assert_eq!(dir_entry.set_casefold(&current_task, true), error!(ENOTSUP));
            assert_eq!(dir_entry.set_casefold(&current_task, false), Ok(()));
        })
        .await;
    }
}
