// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::security;
use crate::task::CurrentTask;
use crate::vfs::{
    CheckAccessReason, DirectoryMode, FileHandle, FileObject, FsLockDepType, FsNodeHandle,
    FsNodeLinkBehavior, FsStr, FsString, LookupVec, MountInfo, Mounts, NamespaceNode, UnlinkKind,
    inotify_hook, path,
};
use atomic_bitflags::atomic_bitflags;
use bitflags::bitflags;
use bstr::ByteSlice;
use fuchsia_rcu::{RcuArc, RcuReadScope};
use fuchsia_sync::ResetDependencies;
use fxfs_unicode::{CasefoldStr, utf8_bytes};
use smallvec::SmallVec;
use starnix_rcu::RcuString;
use starnix_sync::{
    DirEntryChildrenLevel, DirEntryChildrenRecursiveLevel, DynamicLockDepRwLock,
    FuseDirEntryChildrenLevel, LockDepWriteGuard, allow_subclass,
};
use starnix_uapi::auth::FsCred;
use starnix_uapi::errors::{ENOENT, Errno};
use starnix_uapi::file_mode::{Access, FileMode};
use starnix_uapi::inotify_mask::InotifyMask;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::{NAME_MAX, RENAME_EXCHANGE, RENAME_NOREPLACE, RENAME_WHITEOUT, error};
use std::collections::BTreeMap;
use std::fmt;
use std::ops::Deref;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};

bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct RenameFlags: u32 {
        // Exchange the entries.
        const EXCHANGE = RENAME_EXCHANGE;

        // Don't overwrite an existing DirEntry.
        const NOREPLACE = RENAME_NOREPLACE;

        // Create a "whiteout" object to replace the file.
        const WHITEOUT = RENAME_WHITEOUT;

        // Allow replacing any file with a directory. This is an internal flag used only
        // internally inside Starnix for OverlayFS.
        const REPLACE_ANY = 1 << 31;

        // Internal flags that cannot be passed to `sys_rename()`
        const INTERNAL = Self::REPLACE_ANY.bits();
    }
}

pub trait DirEntryOps: Send + Sync + 'static {
    /// Revalidate the [`DirEntry`], if needed.
    ///
    /// Most filesystems don't need to do any revalidations because they are "local"
    /// and all changes to nodes go through the kernel. However some filesystems
    /// allow changes to happen through other means (e.g. NFS, FUSE) and these
    /// filesystems need a way to let the kernel know it may need to refresh its
    /// cached metadata. This method provides that hook for such filesystems.
    ///
    /// For more details, see:
    ///  - https://www.halolinux.us/kernel-reference/the-dentry-cache.html
    ///  - https://www.kernel.org/doc/html/latest/filesystems/path-lookup.html#revalidation-and-automounts
    ///  - https://lwn.net/Articles/649115/
    ///  - https://www.infradead.org/~mchehab/kernel_docs/filesystems/path-walking.html
    ///
    /// Returns `Ok(valid)` where `valid` indicates if the `DirEntry` is still valid,
    /// or an error.
    fn revalidate(&self, _: &CurrentTask, _: &DirEntry) -> Result<bool, Errno> {
        Ok(true)
    }
}

atomic_bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct DirEntryFlags: u8 {
        /// Whether this directory entry has been removed from the tree.
        const IS_DEAD = 1 << 0;

        /// Whether the entry has filesystems mounted on top of it.
        const HAS_MOUNTS = 1 << 1;
    }
}

pub struct DefaultDirEntryOps;

impl DirEntryOps for DefaultDirEntryOps {}

/// An entry in a directory.
///
/// This structure assigns a name to an FsNode in a given file system. An
/// FsNode might have multiple directory entries, for example if there are more
/// than one hard link to the same FsNode. In those cases, each hard link will
/// have a different parent and a different local_name because each hard link
/// has its own DirEntry object.
///
/// A directory cannot have more than one hard link, which means there is a
/// single DirEntry for each Directory FsNode. That invariant lets us store the
/// children for a directory in the DirEntry rather than in the FsNode.
pub struct DirEntry {
    /// The FsNode referenced by this DirEntry.
    ///
    /// A given FsNode can be referenced by multiple DirEntry objects, for
    /// example if there are multiple hard links to a given FsNode.
    pub node: FsNodeHandle,

    /// The [`DirEntryOps`] for this `DirEntry`.
    ///
    /// The `DirEntryOps` are implemented by the individual file systems to provide
    /// specific behaviours for this `DirEntry`.
    ops: Box<dyn DirEntryOps>,

    /// The parent DirEntry.
    ///
    /// The DirEntry tree has strong references from child-to-parent and weak
    /// references from parent-to-child. This design ensures that the parent
    /// chain is always populated in the cache, but some children might be
    /// missing from the cache.
    parent: RcuArc<DirEntry>,

    /// The [`DirEntryFlags`] for this `DirEntry`.
    flags: AtomicDirEntryFlags,

    /// The name that this parent calls this child.
    ///
    /// This name might not be reflected in the full path in the namespace that
    /// contains this DirEntry. For example, this DirEntry might be the root of
    /// a chroot.
    ///
    /// Most callers that want to work with names for DirEntries should use the
    /// NamespaceNodes.
    local_name: RcuString,

    /// A partial cache of the children of this DirEntry.
    ///
    /// DirEntries are added to this cache when they are looked up and removed
    /// when they are no longer referenced.
    ///
    // FIXME(b/379929394): The lock ordering here assumes parent-to-child lock acquisition, which
    // a number of algorithms in the DirEntry operations also assume. This assumption can be broken
    // by the rename operation, which can move nodes around the hierarchy. See the referenced bug
    // for more details, the current mitigations, and potentials for long-term solutions.
    children: DynamicLockDepRwLock<DirEntryChildren>,
}

pub type DirEntryHandle = Arc<DirEntry>;

impl DirEntry {
    #[allow(clippy::let_and_return)]
    pub fn new_uncached(
        node: FsNodeHandle,
        parent: Option<DirEntryHandle>,
        local_name: FsString,
    ) -> DirEntryHandle {
        let ops = node.create_dir_entry_ops();
        let fs_lockdep_type = node.fs().fs_lockdep_type();
        let casefold = node.info().casefold;
        let initial_children = DirEntryChildren::new(casefold);
        let result = Arc::new(DirEntry {
            node,
            ops,
            parent: RcuArc::new(parent),
            flags: Default::default(),
            local_name: local_name.into(),
            children: match fs_lockdep_type {
                FsLockDepType::Normal => {
                    DynamicLockDepRwLock::new::<DirEntryChildrenLevel>(initial_children)
                }
                FsLockDepType::Recursive => {
                    DynamicLockDepRwLock::new::<DirEntryChildrenRecursiveLevel>(initial_children)
                }
                FsLockDepType::Fuse => {
                    DynamicLockDepRwLock::new::<FuseDirEntryChildrenLevel>(initial_children)
                }
            },
        });
        #[cfg(any(test, debug_assertions))]
        {
            // Taking this lock tells the lock tracing system about the parent/child ordering
            // relation.
            let _token = allow_subclass();
            let _l1 = result.children.read();
        }
        result
    }

    pub fn new(
        node: FsNodeHandle,
        parent: Option<DirEntryHandle>,
        local_name: FsString,
    ) -> DirEntryHandle {
        let result = Self::new_uncached(node, parent, local_name);
        result.node.fs().did_create_dir_entry(&result);
        result
    }

    /// Returns a new DirEntry for the given `node` without parent. The entry has no local name and
    /// is not cached.
    pub fn new_unrooted(node: FsNodeHandle) -> DirEntryHandle {
        Self::new_uncached(node, None, FsString::default())
    }

    /// Returns a new `DirEntry` that is ready marked as having been deleted.
    pub fn new_deleted(
        node: FsNodeHandle,
        parent: Option<DirEntryHandle>,
        local_name: FsString,
    ) -> DirEntryHandle {
        let entry = DirEntry::new_uncached(node, parent, local_name);
        entry.raise_flags(DirEntryFlags::IS_DEAD);
        entry
    }

    /// Returns a file handle to this entry, associated with an anonymous namespace.
    pub fn open_anonymous(
        self: &DirEntryHandle,
        current_task: &CurrentTask,
        flags: OpenFlags,
    ) -> Result<FileHandle, Errno> {
        let ops = self.node.create_file_ops(current_task, flags)?;
        FileObject::new(current_task, ops, NamespaceNode::new_anonymous(self.clone()), flags)
    }

    /// Set the children of this DirEntry to the given `children`. This should only ever be called
    /// when children is empty.
    pub fn set_children(self: &DirEntryHandle, children: BTreeMap<FsString, DirEntryHandle>) {
        let mut dir_entry_children = self.lock_children();
        assert!(dir_entry_children.children.is_empty());
        for (name, child) in children.into_iter() {
            child.set_parent(self.clone());
            dir_entry_children.children.insert(name.as_ref(), Arc::downgrade(&child));
        }
    }

    fn lock_children<'a>(self: &'a DirEntryHandle) -> DirEntryLockedChildren<'a> {
        DirEntryLockedChildren { entry: self, children: self.children.write() }
    }

    /// Sets whether this directory is casefolded.
    ///
    /// # Errors
    ///
    /// * [`ENOTDIR`]: If this entry is not a directory.
    /// * [`ENOTSUP`]: If enabling casefolding on a filesystem without casefold support.
    /// * [`ENOTEMPTY`]: If the directory is not empty.
    /// * Propagates errors from [`FsNode::update_attributes`].
    pub fn set_casefold(&self, current_task: &CurrentTask, casefold: bool) -> Result<(), Errno> {
        if self.node.info().casefold == casefold {
            return Ok(());
        }
        if !self.node.is_dir() {
            return error!(ENOTDIR);
        }
        if casefold && !self.node.fs().has_casefold_support() {
            return error!(ENOTSUP);
        }

        // It is important that the `children` write lock is held across the call to
        // `update_attributes` below because it acts as the guard preventing the directory's
        // case-sensitivity from changing concurrently. Elsewhere, we make decisions based on the
        // state of `children` to determine whether a directory is case-folded or not.
        let mut children = self.children.write();
        if children.is_casefold() == casefold {
            return Ok(());
        }

        self.node.update_attributes(current_task, |info| {
            info.casefold = casefold;
            Ok(())
        })?;

        children.set_casefold(casefold);
        Ok(())
    }

    /// The parent DirEntry.
    pub fn parent(&self) -> Option<DirEntryHandle> {
        self.parent.upgrade()
    }

    /// Set the parent of this DirEntry.
    pub fn set_parent(&self, parent: DirEntryHandle) {
        self.parent.update(Some(parent));
    }

    /// The parent DirEntry object or this DirEntry if this entry is the root.
    ///
    /// Useful when traversing up the tree if you always want to find a parent
    /// (e.g., for "..").
    ///
    /// Be aware that the root of one file system might be mounted as a child
    /// in another file system. For that reason, consider walking the
    /// NamespaceNode tree (which understands mounts) rather than the DirEntry
    /// tree.
    pub fn parent_or_self(self: &DirEntryHandle) -> DirEntryHandle {
        self.parent().unwrap_or_else(|| self.clone())
    }

    /// The name that this parent calls this child.
    ///
    /// The reference is only valid for the duration of the RCU read scope.
    pub fn local_name<'a>(&self, scope: &'a RcuReadScope) -> &'a FsStr {
        self.local_name.read(scope)
    }

    /// Whether the given name has special semantics as a directory entry.
    ///
    /// Specifically, whether the name is empty (which means "self"), dot
    /// (which also means "self"), or dot dot (which means "parent").
    pub fn is_reserved_name(name: &FsStr) -> bool {
        name.is_empty() || name == "." || name == ".."
    }

    /// Returns the flags of this DirEntry.
    pub fn flags(&self) -> DirEntryFlags {
        self.flags.load(Ordering::Acquire)
    }

    /// Raises the flags of this DirEntry.
    ///
    /// Returns the flags of this DirEntry before the flags were raised.
    pub fn raise_flags(&self, flags: DirEntryFlags) -> DirEntryFlags {
        self.flags.fetch_or(flags, Ordering::AcqRel)
    }

    /// Lowers the flags of this DirEntry.
    ///
    /// Returns the flags of this DirEntry before the flags were lowered.
    pub fn lower_flags(&self, flags: DirEntryFlags) -> DirEntryFlags {
        self.flags.fetch_and(!flags, Ordering::AcqRel)
    }

    /// Returns true if this DirEntry is dead.
    pub fn is_dead(&self) -> bool {
        self.flags().contains(DirEntryFlags::IS_DEAD)
    }

    /// Look up a directory entry with the given name as direct child of this
    /// entry.
    pub fn component_lookup(
        self: &DirEntryHandle,
        current_task: &CurrentTask,
        mount: &MountInfo,
        name: &FsStr,
    ) -> Result<DirEntryHandle, Errno> {
        let (node, _) = self.get_or_create_child(current_task, mount, name, |d, mount, name| {
            d.lookup(current_task, mount, name)
        })?;
        Ok(node)
    }

    pub fn get_children_pipelined(
        self: &DirEntryHandle,
        current_task: &CurrentTask,
        mount: &MountInfo,
        names: &[&FsStr],
    ) -> LookupVec<Result<DirEntryHandle, Errno>> {
        let mut nodes = LookupVec::new();
        let mut results = LookupVec::new();
        let mut current_parent = self.clone();
        for i in 0..names.len() {
            let next_node = nodes.pop();
            match current_parent.get_or_create_child(
                current_task,
                mount,
                names[i],
                |parent_node, _mount, _name| {
                    if let Some(node) = next_node {
                        return node;
                    }
                    nodes =
                        parent_node.ops().lookup_pipelined(parent_node, current_task, &names[i..]);
                    nodes.reverse();
                    nodes.pop().unwrap()
                },
            ) {
                Ok((entry, _)) => {
                    results.push(Ok(entry.clone()));
                    current_parent = entry;
                }
                Err(e) => {
                    results.push(Err(e));
                    break;
                }
            }
        }
        results
    }

    /// Creates a new DirEntry
    ///
    /// The create_node_fn function is called to create the underlying FsNode
    /// for the DirEntry.
    ///
    /// If the entry already exists, create_node_fn is not called, and EEXIST is
    /// returned.
    pub fn create_entry(
        self: &DirEntryHandle,
        current_task: &CurrentTask,
        mount: &MountInfo,
        name: &FsStr,
        create_node_fn: impl FnOnce(&FsNodeHandle, &MountInfo, &FsStr) -> Result<FsNodeHandle, Errno>,
    ) -> Result<DirEntryHandle, Errno> {
        let (entry, exists) =
            self.create_entry_internal(current_task, mount, name, create_node_fn)?;
        if exists {
            return error!(EEXIST);
        }
        Ok(entry)
    }

    /// Creates a new DirEntry. Works just like create_entry, except if the entry already exists,
    /// it is returned.
    pub fn get_or_create_entry(
        self: &DirEntryHandle,
        current_task: &CurrentTask,
        mount: &MountInfo,
        name: &FsStr,
        create_node_fn: impl FnOnce(&FsNodeHandle, &MountInfo, &FsStr) -> Result<FsNodeHandle, Errno>,
    ) -> Result<DirEntryHandle, Errno> {
        let (entry, _exists) =
            self.create_entry_internal(current_task, mount, name, create_node_fn)?;
        Ok(entry)
    }

    fn create_entry_internal(
        self: &DirEntryHandle,
        current_task: &CurrentTask,
        mount: &MountInfo,
        name: &FsStr,
        create_node_fn: impl FnOnce(&FsNodeHandle, &MountInfo, &FsStr) -> Result<FsNodeHandle, Errno>,
    ) -> Result<(DirEntryHandle, bool), Errno> {
        if DirEntry::is_reserved_name(name) {
            return error!(EEXIST);
        }
        // TODO: Do we need to check name for embedded NUL characters?
        if name.len() > NAME_MAX as usize {
            return error!(ENAMETOOLONG);
        }
        if name.contains(&path::SEPARATOR) {
            return error!(EINVAL);
        }
        let (entry, exists) =
            self.get_or_create_child(current_task, mount, name, create_node_fn)?;
        if !exists {
            // An entry was created. Update the ctime and mtime of this directory.
            self.node.update_ctime_mtime();
            entry.notify_creation();
        }
        Ok((entry, exists))
    }

    // This is marked as test-only because it sets the owner/group to root instead of the current
    // user to save a bit of typing in tests, but this shouldn't happen silently in production.
    #[cfg(test)]
    pub fn create_dir(
        self: &DirEntryHandle,
        current_task: &CurrentTask,
        name: &FsStr,
    ) -> Result<DirEntryHandle, Errno> {
        self.create_dir_for_testing(current_task, name)
    }

    // This function is for testing because it sets the owner/group to root instead of the current
    // user to save a bit of typing in tests, but this shouldn't happen silently in production.
    pub fn create_dir_for_testing(
        self: &DirEntryHandle,
        current_task: &CurrentTask,
        name: &FsStr,
    ) -> Result<DirEntryHandle, Errno> {
        // TODO: apply_umask
        self.create_entry(current_task, &MountInfo::detached(), name, |dir, mount, name| {
            dir.create_node(
                current_task,
                mount,
                name,
                starnix_uapi::file_mode::mode!(IFDIR, 0o777),
                starnix_uapi::device_id::DeviceId::NONE,
                FsCred::root(),
            )
        })
    }

    /// Creates an anonymous file.
    ///
    /// The FileMode::IFMT of the FileMode is always FileMode::IFREG.
    ///
    /// Used by O_TMPFILE.
    pub fn create_tmpfile(
        self: &DirEntryHandle,
        current_task: &CurrentTask,
        mount: &MountInfo,
        mode: FileMode,
        owner: FsCred,
        flags: OpenFlags,
    ) -> Result<DirEntryHandle, Errno> {
        // Only directories can have children.
        if !self.node.is_dir() {
            return error!(ENOTDIR);
        }
        assert!(mode.is_reg());

        // From <https://man7.org/linux/man-pages/man2/open.2.html>:
        //
        //   Specifying O_EXCL in conjunction with O_TMPFILE prevents a
        //   temporary file from being linked into the filesystem in
        //   the above manner.  (Note that the meaning of O_EXCL in
        //   this case is different from the meaning of O_EXCL
        //   otherwise.)
        let link_behavior = if flags.contains(OpenFlags::EXCL) {
            FsNodeLinkBehavior::Disallowed
        } else {
            FsNodeLinkBehavior::Allowed
        };

        let node = self.node.create_tmpfile(current_task, mount, mode, owner, link_behavior)?;
        let local_name = format!("#{}", node.ino).into();
        Ok(DirEntry::new_deleted(node, Some(self.clone()), local_name))
    }

    pub fn unlink(
        self: &DirEntryHandle,
        current_task: &CurrentTask,
        mount: &MountInfo,
        name: &FsStr,
        kind: UnlinkKind,
        directory_mode: DirectoryMode,
    ) -> Result<(), Errno> {
        assert!(!DirEntry::is_reserved_name(name));

        // child_to_unlink *must* be dropped after self_children (even in the error paths).
        let child_to_unlink;

        let mut self_children = self.lock_children();
        child_to_unlink = self_children.component_lookup(current_task, mount, name)?;
        child_to_unlink.require_no_mounts(mount)?;

        // Check that this filesystem entry must be a directory. This can
        // happen if the path terminates with a trailing slash.
        //
        // Example: If we're unlinking a symlink `/foo/bar/`, this would
        // result in `ENOTDIR` because of the trailing slash, even if
        // `UnlinkKind::NonDirectory` was used.
        if directory_mode == DirectoryMode::MustBeDirectory && !child_to_unlink.node.is_dir() {
            return error!(ENOTDIR);
        }

        match kind {
            UnlinkKind::Directory => {
                if !child_to_unlink.node.is_dir() {
                    return error!(ENOTDIR);
                }
            }
            UnlinkKind::NonDirectory => {
                if child_to_unlink.node.is_dir() {
                    return error!(EISDIR);
                }
            }
        }

        self.node.unlink(current_task, mount, name, &child_to_unlink.node)?;
        self_children.children.remove(name);

        std::mem::drop(self_children);
        child_to_unlink.destroy(&current_task.kernel().mounts);

        Ok(())
    }

    /// Destroy this directory entry.
    ///
    /// Notice that this method takes `self` by value to destroy this reference.
    fn destroy(self: DirEntryHandle, mounts: &Mounts) {
        let was_already_dead =
            self.raise_flags(DirEntryFlags::IS_DEAD).contains(DirEntryFlags::IS_DEAD);
        if was_already_dead {
            return;
        }
        let unmount =
            self.lower_flags(DirEntryFlags::HAS_MOUNTS).contains(DirEntryFlags::HAS_MOUNTS);
        self.node.fs().will_destroy_dir_entry(&self);
        if unmount {
            mounts.unmount(&self);
        }
        self.notify_deletion();
    }

    /// Returns whether this entry is a descendant of |other|.
    pub fn is_descendant_of(self: &DirEntryHandle, other: &DirEntryHandle) -> bool {
        let mut current = self.clone();
        loop {
            if Arc::ptr_eq(&current, other) {
                // We found |other|.
                return true;
            }
            if let Some(parent) = current.parent() {
                current = parent;
            } else {
                // We reached the root of the file system.
                return false;
            }
        }
    }

    /// Rename the file with old_basename in old_parent to new_basename in
    /// new_parent.
    ///
    /// old_parent and new_parent must belong to the same file system.
    pub fn rename(
        current_task: &CurrentTask,
        old_parent: &DirEntryHandle,
        old_mount: &MountInfo,
        old_basename: &FsStr,
        new_parent: &DirEntryHandle,
        new_mount: &MountInfo,
        new_basename: &FsStr,
        flags: RenameFlags,
    ) -> Result<(), Errno> {
        // The nodes we are touching must be part of the same mount.
        if old_mount != new_mount {
            return error!(EXDEV);
        }

        // The mounts are equals, choose one.
        let mount = old_mount;

        // If either the old_basename or the new_basename is a reserved name
        // (e.g., "." or ".."), then we cannot do the rename.
        if DirEntry::is_reserved_name(old_basename) || DirEntry::is_reserved_name(new_basename) {
            if flags.contains(RenameFlags::NOREPLACE) {
                return error!(EEXIST);
            }
            return error!(EBUSY);
        }

        // If the names and parents are the same, then there's nothing to do
        // and we can report success.
        if Arc::ptr_eq(&old_parent.node, &new_parent.node) && old_basename == new_basename {
            if flags.contains(RenameFlags::NOREPLACE) {
                return error!(EEXIST);
            }
            return Ok(());
        }

        // This task must have write access to the old and new parent nodes.
        old_parent.node.check_access(
            current_task,
            mount,
            Access::WRITE,
            CheckAccessReason::InternalPermissionChecks,
            old_parent,
        )?;
        new_parent.node.check_access(
            current_task,
            mount,
            Access::WRITE,
            CheckAccessReason::InternalPermissionChecks,
            new_parent,
        )?;

        // The mount check ensures that the nodes we're touching are part of the
        // same file system. It doesn't matter where we grab the FileSystem reference from.
        let fs = old_parent.node.fs();

        // We need to hold these DirEntryHandles until after we drop all the
        // locks so that we do not deadlock when we drop them.
        let renamed;
        let mut maybe_replaced = None;

        {
            // Before we take any locks, we need to take the rename mutex on
            // the file system. This lock ensures that no other rename
            // operations are happening in this file system while we're
            // analyzing this rename operation.
            //
            // For example, we grab writer locks on both old_parent and
            // new_parent. If there was another rename operation in flight with
            // old_parent and new_parent reversed, then we could deadlock while
            // trying to acquire these locks.
            let _lock = fs.rename_mutex.lock();

            // We cannot simply grab the locks on old_parent and new_parent
            // independently because old_parent and new_parent might be the
            // same directory entry. Instead, we use the RenameGuard helper to
            // grab the appropriate locks.
            let mut state = RenameGuard::lock(old_parent, new_parent);

            // Now that we know the old_parent child list cannot change, we
            // establish the DirEntry that we are going to try to rename.
            renamed =
                state.old_parent_children().component_lookup(current_task, mount, old_basename)?;

            // We need to check if there is already a DirEntry with
            // new_basename in new_parent. If so, there are additional checks
            // we need to perform.
            // This must be done BEFORE locking info to avoid self-deadlock.
            let lookup_replaced =
                state.new_parent_children().component_lookup(current_task, mount, new_basename);

            // If the target entry is an ancestor of the source parent, the
            // rename would create a cycle (for EXCHANGE) or attempt to
            // overwrite a non-empty directory.
            // We check this before acquiring child locks to avoid
            // deadlocks from bottom-up locking (locking ancestor after
            // descendant).
            // This is done early (before parent locks) as a fail-fast
            // optimization since we already have the lookup result.
            if let Ok(replaced) = &lookup_replaced {
                if old_parent.is_descendant_of(replaced) {
                    if flags.contains(RenameFlags::EXCHANGE) {
                        return error!(EINVAL);
                    } else {
                        return error!(ENOTEMPTY);
                    }
                }
            }

            // Lock the info for the parents to ensure that subsequent checks on their
            // state (e.g., sticky bit checks) and the actual rename operation are not racy.
            let mut state =
                state.lock_info(old_parent, new_parent, &renamed, lookup_replaced.as_ref().ok());

            // If new_parent is a descendant of renamed, the operation would
            // create a cycle. That's disallowed.
            if new_parent.is_descendant_of(&renamed) {
                return error!(EINVAL);
            }

            // Check whether the sticky bit on the old parent prevents us from
            // removing this child.
            {
                // Safe because the parent is locked first, and then we check the
                // sticky bit of the child. This parent -> child acquisition follows
                // the hierarchical lock ordering.
                let _token = allow_subclass();
                old_parent.node.check_sticky_bit(
                    current_task,
                    &renamed.node,
                    state.old_parent_info(),
                )?;
            }

            // Check whether the renamed entry is a mountpoint.
            // TODO: We should hold a read lock on the mount points for this
            //       namespace to prevent the child from becoming a mount point
            //       while this function is executing.
            renamed.require_no_mounts(mount)?;

            // We lookup the replaced entry before locking info to avoid deadlock, but we match
            // on the result here under the parent info locks. This ensures that checks on the
            // replaced entry (e.g., existence, directory status, identity) are consistent and
            // do not race with concurrent operations.
            match &lookup_replaced {
                Ok(replaced) => {
                    // Set `maybe_replaced` now to ensure it gets dropped in the right order.
                    let replaced = maybe_replaced.insert(replaced.clone());

                    if flags.contains(RenameFlags::NOREPLACE) {
                        // From <https://man7.org/linux/man-pages/man2/rename.2.html>:
                        //
                        //   RENAME_NOREPLACE
                        //          Don't overwrite newpath of the rename. Return an error
                        //          if newpath already exists.
                        return error!(EEXIST);
                    }

                    if Arc::ptr_eq(&renamed.node, &replaced.node) {
                        if !Arc::ptr_eq(&renamed, &replaced) {
                            // From <https://man7.org/linux/man-pages/man2/rename.2.html>:
                            //
                            // "If oldpath and newpath are existing hard links referring to the
                            // same file, then rename() does nothing, and returns a success
                            // status."
                            return Ok(());
                        }

                        // Same DirEntry (e.g. case-only rename in a casefolded directory).
                        // With EXCHANGE, swapping an entry with itself is a no-op that succeeds.
                        // Without EXCHANGE, we must proceed to rename on the underlying filesystem
                        // to update the on-disk casing.
                        if flags.contains(RenameFlags::EXCHANGE) {
                            return Ok(());
                        }
                    }

                    // Sayeth https://man7.org/linux/man-pages/man2/rename.2.html:
                    //
                    // "oldpath can specify a directory.  In this case, newpath must"
                    // either not exist, or it must specify an empty directory."
                    if state.replaced_is_dir() {
                        // Check whether the replaced entry is a mountpoint.
                        // TODO: We should hold a read lock on the mount points for this
                        //       namespace to prevent the child from becoming a mount point
                        //       while this function is executing.
                        replaced.require_no_mounts(mount)?;
                    }

                    if !flags.intersects(RenameFlags::EXCHANGE | RenameFlags::REPLACE_ANY) {
                        let renamed_is_dir = state.renamed_is_dir();
                        let replaced_is_dir = state.replaced_is_dir();
                        if renamed_is_dir && !replaced_is_dir {
                            return error!(ENOTDIR);
                        } else if !renamed_is_dir && replaced_is_dir {
                            return error!(EISDIR);
                        }
                    }
                }
                // It's fine for the lookup to fail to find a child.
                Err(errno) if *errno == ENOENT => {
                    if flags.contains(RenameFlags::EXCHANGE) {
                        return error!(ENOENT);
                    }
                }
                // However, other errors are fatal.
                Err(e) => return Err(e.clone()),
            }

            security::check_fs_node_rename_access(
                current_task,
                &old_parent.node,
                &renamed.node,
                &new_parent.node,
                maybe_replaced.as_ref().map(|dir_entry| dir_entry.node.deref().as_ref()),
                old_basename,
                new_basename,
            )?;

            if let Some(replaced) = maybe_replaced.as_ref() {
                // Safe because the parent is locked first, and then we check the
                // sticky bit of the child. This parent -> child acquisition
                // follows the hierarchical lock ordering.
                let _token = allow_subclass();
                new_parent.node.check_sticky_bit(
                    current_task,
                    &replaced.node,
                    state.new_parent_info().unwrap_or_else(|| state.old_parent_info()),
                )?;
            }

            // In a casefolded directory, renaming an entry to a case variant of itself resolves
            // `lookup_replaced` to the same DirEntry (`renamed`). At the filesystem level, no
            // distinct directory entry is being overwritten or replaced, so clear `state.replaced`
            // to `None` before invoking the filesystem operation.
            if state.replaced.is_some_and(|r| Arc::ptr_eq(&renamed, r)) {
                state.replaced = None;
            }

            if flags.contains(RenameFlags::EXCHANGE) {
                fs.exchange(current_task, &mut state, old_basename, new_basename)?;
            } else {
                fs.rename(current_task, &mut state, old_basename, new_basename)?;
            }

            if flags.contains(RenameFlags::EXCHANGE) {
                // Reparent `replaced` when exchanging.
                let replaced =
                    maybe_replaced.as_ref().expect("replaced expected with RENAME_EXCHANGE");
                replaced.set_parent(old_parent.clone());
                replaced.local_name.update(old_basename.to_owned());
                state.old_parent_children().children.insert(old_basename, Arc::downgrade(replaced));
            } else {
                // Remove the renamed child from the old_parent's child list.
                // In casefolded directories, removing old_basename before inserting new_basename
                // prevents removal from clobbering the freshly inserted entry when old and new
                // names canonicalize to the same key.
                state.old_parent_children().children.remove(old_basename);
            }

            // We need to update the parent and local name for the DirEntry
            // we are renaming to reflect its new parent and its new name.
            renamed.set_parent(new_parent.clone());
            renamed.local_name.update(new_basename.to_owned());

            // Actually add the renamed child to the new_parent's child list.
            // This operation implicitly removes the replaced child (if any)
            // from the child list.
            state.new_parent_children().children.insert(new_basename, Arc::downgrade(&renamed));

            // Lock ordering is enforced from parent-to-child, and therefore we need to
            // reset the lock ordering constraints when we reorder the tree nodes.
            // SAFETY: We manually clear the dependency graph for these locks.
            // This is safe because `fs.rename_mutex` is held during this operation, which
            // prevents the tree topology from changing concurrently. This allows us to safely
            // dynamically enforce a sound locking order (e.g. by memory address in `RenameGuard`)
            // to avoid deadlocks. Clearing the graph prevents false-positive cycle panics from
            // `tracing-mutex` after the node is reparented.
            unsafe {
                if let Some(replaced) =
                    maybe_replaced.as_ref().filter(|_| flags.contains(RenameFlags::EXCHANGE))
                {
                    replaced.children.reset_dependencies();
                    replaced.node.info_lock().reset_dependencies();
                }
                renamed.children.reset_dependencies();
                renamed.node.info_lock().reset_dependencies();
                old_parent.children.reset_dependencies();
                old_parent.node.info_lock().reset_dependencies();
                new_parent.children.reset_dependencies();
                new_parent.node.info_lock().reset_dependencies();
            }
        };

        fs.purge_old_entries();

        if let Some(replaced) = maybe_replaced {
            if !flags.contains(RenameFlags::EXCHANGE) && !Arc::ptr_eq(&renamed, &replaced) {
                replaced.destroy(&current_task.kernel().mounts);
            }
        }

        // Renaming a file updates its ctime.
        renamed.node.update_ctime();

        let mode = renamed.node.info().mode;
        if let Some(hook) =
            current_task.kernel().expando.peek::<Arc<dyn inotify_hook::NotifyHook>>()
        {
            let cookie = hook.get_next_cookie();
            old_parent.node.notify(InotifyMask::MOVE_FROM, cookie, old_basename, mode, false);
            new_parent.node.notify(InotifyMask::MOVE_TO, cookie, new_basename, mode, false);
            renamed.node.notify(InotifyMask::MOVE_SELF, 0, Default::default(), mode, false);
        }

        Ok(())
    }

    pub(crate) fn get_children<F, T>(&self, callback: F) -> T
    where
        F: FnOnce(&DirEntryChildren) -> T,
    {
        let children = self.children.read();
        callback(&children)
    }

    /// Remove the child with the given name from the children cache.  The child must not have any
    /// mounts.
    pub fn remove_child(&self, name: &FsStr, mounts: &Mounts) {
        let mut children = self.children.write();
        let child = children.remove(name).and_then(|weak| weak.upgrade());
        if let Some(child) = child {
            std::mem::drop(children);
            child.destroy(mounts);
        }
    }

    fn get_or_create_child(
        self: &DirEntryHandle,
        current_task: &CurrentTask,
        mount: &MountInfo,
        name: &FsStr,
        create_fn: impl FnOnce(&FsNodeHandle, &MountInfo, &FsStr) -> Result<FsNodeHandle, Errno>,
    ) -> Result<(DirEntryHandle, bool), Errno> {
        assert!(!DirEntry::is_reserved_name(name));
        // Only directories can have children.
        if !self.node.is_dir() {
            return error!(ENOTDIR);
        }
        // The user must be able to search the directory (requires the EXEC permission)
        self.node.check_access(
            current_task,
            mount,
            Access::EXEC,
            CheckAccessReason::InternalPermissionChecks,
            self,
        )?;

        // Check if the child is already in children. In that case, we can
        // simply return the child and we do not need to call init_fn.
        let child = self.children.read().get(name).and_then(Weak::upgrade);
        let (child, create_result) = if let Some(child) = child {
            // Do not cache a child in a locked directory
            if self.node.fail_if_locked(current_task, &self.node.info()).is_ok() {
                child.node.fs().did_access_dir_entry(&child);
            }
            (child, CreationResult::Existed { create_fn })
        } else {
            let (child, create_result) =
                self.lock_children().get_or_create_child(current_task, mount, name, create_fn)?;
            child.node.fs().purge_old_entries();
            (child, create_result)
        };

        let (child, exists) = match create_result {
            CreationResult::Created => (child, false),
            CreationResult::Existed { create_fn } => {
                if child.ops.revalidate(current_task, &child)? {
                    (child, true)
                } else {
                    self.internal_remove_child(&child);
                    child.destroy(&current_task.kernel().mounts);

                    let (child, create_result) = self.lock_children().get_or_create_child(
                        current_task,
                        mount,
                        name,
                        create_fn,
                    )?;
                    child.node.fs().purge_old_entries();
                    (child, matches!(create_result, CreationResult::Existed { .. }))
                }
            }
        };

        Ok((child, exists))
    }

    // This function is only useful for tests and has some oddities.
    //
    // For example, not all the children might have been looked up yet, which
    // means the returned vector could be missing some names.
    //
    // Also, the vector might have "extra" names that are in the process of
    // being looked up. If the lookup fails, they'll be removed.
    #[cfg(test)]
    pub fn copy_child_names(&self) -> Vec<FsString> {
        let scope = RcuReadScope::new();
        self.children.read().copy_child_names(&scope)
    }

    fn internal_remove_child(&self, child: &DirEntry) {
        let mut children = self.children.write();
        let scope = RcuReadScope::new();
        let local_name = child.local_name.read(&scope);
        children.remove_if_child_matches(local_name, child);
    }

    /// Notifies watchers on the current node and its parent about an event.
    pub fn notify(&self, event_mask: InotifyMask) {
        self.notify_watchers(event_mask, self.is_dead());
    }

    /// Notifies watchers on the current node and its parent about an event.
    ///
    /// Used for FSNOTIFY_EVENT_INODE events, which ignore IN_EXCL_UNLINK.
    pub fn notify_ignoring_excl_unlink(&self, event_mask: InotifyMask) {
        // We pretend that this directory entry is not dead to ignore IN_EXCL_UNLINK.
        self.notify_watchers(event_mask, false);
    }

    fn notify_watchers(&self, event_mask: InotifyMask, is_dead: bool) {
        let mode = self.node.info().mode;
        {
            let scope = RcuReadScope::new();
            if let Some(parent) = self.parent() {
                let local_name = self.local_name.read(&scope);
                parent.node.notify(event_mask, 0, local_name, mode, is_dead);
            }
        }
        self.node.notify(event_mask, 0, Default::default(), mode, is_dead);
    }

    /// Notifies parents about creation, and notifies current node about link_count change.
    fn notify_creation(&self) {
        let mode = self.node.info().mode;
        if Arc::strong_count(&self.node) > 1 {
            // Notify about link change only if there is already a hardlink.
            self.node.notify(InotifyMask::ATTRIB, 0, Default::default(), mode, false);
        }
        let scope = RcuReadScope::new();
        if let Some(parent) = self.parent() {
            let local_name = self.local_name.read(&scope);
            parent.node.notify(InotifyMask::CREATE, 0, local_name, mode, false);
        }
    }

    /// Notifies watchers on the current node about deletion if this is the
    /// last hardlink, and drops the DirEntryHandle kept by Inotify.
    /// Parent is also notified about deletion.
    fn notify_deletion(&self) {
        let mode = self.node.info().mode;
        if !mode.is_dir() {
            // Linux notifies link count change for non-directories.
            self.node.notify(InotifyMask::ATTRIB, 0, Default::default(), mode, false);
        }

        // This check is incorrect if there's another hard link to this FsNode that isn't in
        // memory at the moment.
        if Arc::strong_count(&self.node) == 1 {
            self.node.notify(InotifyMask::DELETE_SELF, 0, Default::default(), mode, false);
        }

        let scope = RcuReadScope::new();
        if let Some(parent) = self.parent() {
            let local_name = self.local_name.read(&scope);
            parent.node.notify(InotifyMask::DELETE, 0, local_name, mode, false);
        }
    }

    /// Returns true if this entry has mounts.
    pub fn has_mounts(&self) -> bool {
        self.flags().contains(DirEntryFlags::HAS_MOUNTS)
    }

    /// Records whether or not the entry has mounts.
    pub fn set_has_mounts(&self, v: bool) {
        if v {
            self.raise_flags(DirEntryFlags::HAS_MOUNTS);
        } else {
            self.lower_flags(DirEntryFlags::HAS_MOUNTS);
        }
    }

    /// Verifies this directory has nothing mounted on it.
    fn require_no_mounts(self: &Arc<Self>, parent_mount: &MountInfo) -> Result<(), Errno> {
        if self.has_mounts() {
            if let Some(mount) = parent_mount.as_ref() {
                if mount.has_submount(self) {
                    return error!(EBUSY);
                }
            }
        }
        Ok(())
    }
}

/// Canonicalizes a filename into its casefolded, NFD-normalized byte representation on the stack.
/// Non-UTF-8 filenames are preserved as raw bytes.
#[inline]
fn canonicalize_name(name: impl AsRef<[u8]>) -> SmallVec<[u8; NAME_MAX as usize]> {
    let name = name.as_ref();
    match std::str::from_utf8(name) {
        Ok(valid) => {
            CasefoldStr::new(valid).casefold_normalized_chars().flat_map(utf8_bytes).collect()
        }
        Err(_) => SmallVec::from_slice(name),
    }
}

/// A canonicalized key stored in the directory entry cache.
///
/// Stores pre-casefolded, NFD-normalized UTF-8 bytes (for casefolded directories) or raw bytes
/// (for case-sensitive directories and non-UTF-8 filenames).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct DirEntryChildKey(FsString);

impl DirEntryChildKey {
    pub fn new(name: &FsStr, casefold: bool) -> Self {
        if casefold {
            Self(canonicalize_name(name).into_vec().into())
        } else {
            Self(name.to_owned())
        }
    }
}

impl std::borrow::Borrow<FsStr> for DirEntryChildKey {
    fn borrow(&self) -> &FsStr {
        self.0.as_bstr()
    }
}

/// The cached child directory entries of a directory.
///
/// Keys stored in this map are strictly internal to the VFS cache and are never exposed back to
/// userspace. Directory listings (`readdir`) query the underlying filesystem directly for canonical
/// on-disk names, while individual child entries track their associated lookup/creation name in
/// [`DirEntry::local_name`].
#[derive(Default, Debug)]
pub(crate) struct DirEntryChildren {
    entries: BTreeMap<DirEntryChildKey, Weak<DirEntry>>,
    casefold: bool,
}

impl std::ops::Deref for DirEntryChildren {
    type Target = BTreeMap<DirEntryChildKey, Weak<DirEntry>>;

    fn deref(&self) -> &Self::Target {
        &self.entries
    }
}

impl DirEntryChildren {
    pub fn new(casefold: bool) -> Self {
        Self { entries: BTreeMap::new(), casefold }
    }

    pub fn is_casefold(&self) -> bool {
        self.casefold
    }

    pub fn set_casefold(&mut self, casefold: bool) {
        self.entries.clear();
        self.casefold = casefold;
    }

    /// Looks up a child entry by name.
    pub fn get(&self, name: &FsStr) -> Option<&Weak<DirEntry>> {
        if self.casefold {
            let key = canonicalize_name(name);
            self.entries.get(FsStr::new(&key))
        } else {
            self.entries.get(name)
        }
    }

    /// Inserts a child into the directory cache under `name`, returning the weak reference
    /// previously stored under this name, if any.
    pub fn insert(&mut self, name: &FsStr, child: Weak<DirEntry>) -> Option<Weak<DirEntry>> {
        self.entries.insert(DirEntryChildKey::new(name, self.casefold), child)
    }

    /// Removes a child from the directory cache by name, returning the weak reference
    /// previously stored under this name, if any.
    pub fn remove(&mut self, name: &FsStr) -> Option<Weak<DirEntry>> {
        if self.casefold {
            let key = canonicalize_name(name);
            self.entries.remove(FsStr::new(&key))
        } else {
            self.entries.remove(name)
        }
    }

    /// Removes a child entry if the cached weak reference matches `expected_child`.
    /// Returns `true` if the entry was found and removed.
    pub fn remove_if_child_matches(&mut self, name: &FsStr, expected_child: &DirEntry) -> bool {
        if let Some(weak_child) = self.get(name) {
            if std::ptr::eq(weak_child.as_ptr(), expected_child) {
                let _ = self.remove(name);
                return true;
            }
        }
        false
    }

    #[cfg(test)]
    /// Extracts a snapshot of all active child names, retaining the exact casing currently
    /// stored on each child node and filtering out dropped entries.
    fn copy_child_names(&self, scope: &RcuReadScope) -> Vec<FsString> {
        self.entries
            .values()
            .filter_map(|child| Weak::upgrade(child).map(|c| c.local_name.read(scope).to_owned()))
            .collect()
    }
}

struct DirEntryLockedChildren<'a> {
    entry: &'a DirEntryHandle,
    children: LockDepWriteGuard<'a, DirEntryChildren>,
}

enum CreationResult<F> {
    Created,
    Existed { create_fn: F },
}

impl<'a> DirEntryLockedChildren<'a> {
    fn component_lookup(
        &mut self,
        current_task: &CurrentTask,
        mount: &MountInfo,
        name: &FsStr,
    ) -> Result<DirEntryHandle, Errno> {
        assert!(!DirEntry::is_reserved_name(name));
        let (node, _) =
            self.get_or_create_child(current_task, mount, name, |_, _, _| error!(ENOENT))?;
        Ok(node)
    }

    fn get_or_create_child<
        F: FnOnce(&FsNodeHandle, &MountInfo, &FsStr) -> Result<FsNodeHandle, Errno>,
    >(
        &mut self,
        current_task: &CurrentTask,
        mount: &MountInfo,
        name: &FsStr,
        create_fn: F,
    ) -> Result<(DirEntryHandle, CreationResult<F>), Errno> {
        let create_child = |create_fn: F| {
            // Before creating the child, check for existence.
            let (node, create_result) = match self.entry.node.lookup(current_task, mount, name) {
                Ok(node) => (node, CreationResult::Existed { create_fn }),
                Err(e) if e == ENOENT => {
                    (create_fn(&self.entry.node, mount, name)?, CreationResult::Created)
                }
                Err(e) => return Err(e),
            };

            assert!(
                node.info().mode & FileMode::IFMT != FileMode::EMPTY,
                "FsNode initialization did not populate the FileMode in FsNodeInfo."
            );

            let entry = DirEntry::new(node, Some(self.entry.clone()), name.to_owned());

            if let Err(err) = security::fs_node_init_with_dentry(current_task, &entry) {
                // Null out the `parent` reference from `entry` otherwise dropping `entry` will
                // attempt to remove itself from `parent`, triggering a deadlock with `self`.
                entry.parent.update(None);
                return Err(err);
            }

            Ok((entry, create_result))
        };

        if let Some(child) = self.children.get(name).and_then(Weak::upgrade) {
            // Do not cache a child in a locked directory
            if self.entry.node.fail_if_locked(current_task, &self.entry.node.info()).is_ok() {
                child.node.fs().did_access_dir_entry(&child);
            }
            return Ok((child, CreationResult::Existed { create_fn }));
        }

        let (child, create_result) = create_child(create_fn)?;
        // Do not cache a child in a locked directory
        if self.entry.node.fail_if_locked(current_task, &self.entry.node.info()).is_ok() {
            self.children.insert(name, Arc::downgrade(&child));
        }

        Ok((child, create_result))
    }
}

impl fmt::Debug for DirEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let scope = RcuReadScope::new();
        let mut parents = vec![];
        let mut maybe_parent = self.parent();
        while let Some(parent) = maybe_parent {
            parents.push(parent.local_name.read(&scope));
            maybe_parent = parent.parent();
        }
        let mut builder = f.debug_struct("DirEntry");
        builder.field("id", &(self as *const DirEntry));
        builder.field("local_name", &self.local_name.read(&scope).to_owned());
        if !parents.is_empty() {
            builder.field("parents", &parents);
        }
        builder.finish()
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum RenameRelationship {
    Same,
    NewIsDescendant,
    OldIsDescendant,
    Independent,
}

struct RenameGuard<'a> {
    old_parent_guard: DirEntryLockedChildren<'a>,
    new_parent_guard: Option<DirEntryLockedChildren<'a>>,
    relationship: RenameRelationship,
}

impl<'a> RenameGuard<'a> {
    fn lock(old_parent: &'a DirEntryHandle, new_parent: &'a DirEntryHandle) -> Self {
        if Arc::ptr_eq(old_parent, new_parent) {
            let old_parent_guard = old_parent.lock_children();
            Self {
                old_parent_guard,
                new_parent_guard: None,
                relationship: RenameRelationship::Same,
            }
        } else if new_parent.is_descendant_of(old_parent) {
            let old_parent_guard = old_parent.lock_children();
            let _token = allow_subclass();
            let new_parent_guard = new_parent.lock_children();
            Self {
                old_parent_guard,
                new_parent_guard: Some(new_parent_guard),
                relationship: RenameRelationship::NewIsDescendant,
            }
        } else if old_parent.is_descendant_of(new_parent) {
            let new_parent_guard = new_parent.lock_children();
            let _token = allow_subclass();
            let old_parent_guard = old_parent.lock_children();
            Self {
                old_parent_guard,
                new_parent_guard: Some(new_parent_guard),
                relationship: RenameRelationship::OldIsDescendant,
            }
        } else {
            // Independent directories can be locked in address order.
            let (g1, g2) =
                starnix_sync::ordered_write_lock(&old_parent.children, &new_parent.children);
            let old_parent_guard = DirEntryLockedChildren { entry: old_parent, children: g1 };
            let new_parent_guard = DirEntryLockedChildren { entry: new_parent, children: g2 };
            Self {
                old_parent_guard,
                new_parent_guard: Some(new_parent_guard),
                relationship: RenameRelationship::Independent,
            }
        }
    }

    /// Consumes the `RenameGuard` (which only holds children locks) and locks the `info`
    /// of the parent directories in a safe order to prevent deadlocks. Returns a
    /// `RenameGuardLocked` which encapsulates all acquired locks (both children and info).
    fn lock_info(
        self,
        old_parent: &'a DirEntryHandle,
        new_parent: &'a DirEntryHandle,
        renamed: &'a DirEntryHandle,
        replaced: Option<&'a DirEntryHandle>,
    ) -> RenameContext<'a> {
        let (g1, g2) = match self.relationship {
            RenameRelationship::Same => (old_parent.node.info_lock().write(), None),
            RenameRelationship::NewIsDescendant => {
                let g1 = old_parent.node.info_lock().write();
                let _token = allow_subclass();
                let g2 = new_parent.node.info_lock().write();
                (g1, Some(g2))
            }
            RenameRelationship::OldIsDescendant => {
                let g2 = new_parent.node.info_lock().write();
                let _token = allow_subclass();
                let g1 = old_parent.node.info_lock().write();
                (g1, Some(g2))
            }
            RenameRelationship::Independent => {
                let (g1, g2) = starnix_sync::ordered_write_lock(
                    old_parent.node.info_lock(),
                    new_parent.node.info_lock(),
                );
                (g1, Some(g2))
            }
        };

        RenameContext {
            renamed,
            replaced,
            old_parent_guard: self.old_parent_guard,
            new_parent_guard: self.new_parent_guard,
            old_parent_info_guard: g1,
            new_parent_info_guard: g2,
        }
    }

    fn old_parent_children(&mut self) -> &mut DirEntryLockedChildren<'a> {
        &mut self.old_parent_guard
    }

    fn new_parent_children(&mut self) -> &mut DirEntryLockedChildren<'a> {
        if let Some(new_guard) = self.new_parent_guard.as_mut() {
            new_guard
        } else {
            &mut self.old_parent_guard
        }
    }
}

/// A context that holds the locked children and info of the parents during a
/// rename operation.
///
/// The context is constructed by locking the parent directories
/// (`old_parent`, `new_parent`) for write. These parent locks are held for
/// the duration of the context's lifetime, preventing concurrent
/// modifications to the parents.
///
/// Child nodes (`renamed` and `replaced`) are *not* locked upon
/// construction. To query properties of the children (like whether they are
/// directories) without risking deadlocks, use the provided helper methods
/// (`renamed_is_dir`, `replaced_is_dir`) instead of locking them directly.
pub struct RenameContext<'a> {
    pub renamed: &'a DirEntryHandle,
    pub replaced: Option<&'a DirEntryHandle>,
    old_parent_guard: DirEntryLockedChildren<'a>,
    new_parent_guard: Option<DirEntryLockedChildren<'a>>,
    old_parent_info_guard: LockDepWriteGuard<'a, crate::vfs::FsNodeInfo>,
    new_parent_info_guard: Option<LockDepWriteGuard<'a, crate::vfs::FsNodeInfo>>,
}

impl<'a> RenameContext<'a> {
    /// Returns whether the renamed child node is a directory.
    ///
    /// This method safely handles child info locking under the parent locks.
    /// If the renamed node is same as a parent (which is already locked),
    /// it uses the parent's guard to avoid self-deadlock. Otherwise, it
    /// locks the child info using `allow_subclass`.
    pub fn renamed_is_dir(&self) -> bool {
        self.is_dir(&self.renamed.node)
    }

    /// Returns whether the replaced child node is a directory.
    ///
    /// Returns `false` if `replaced` is `None`.
    ///
    /// This method safely handles child info locking under the parent locks.
    /// If the replaced node is same as a parent (which is already locked),
    /// it uses the parent's guard to avoid self-deadlock. Otherwise, it
    /// locks the child info using `allow_subclass`.
    pub fn replaced_is_dir(&self) -> bool {
        self.replaced.map(|r| self.is_dir(&r.node)).unwrap_or(false)
    }

    /// Returns the old parent directory entry handle.
    ///
    /// The old parent's child list is write-locked for the lifetime of the
    /// context.
    pub fn old_parent(&self) -> &DirEntryHandle {
        self.old_parent_guard.entry
    }

    /// Returns the new parent directory entry handle.
    ///
    /// The new parent's child list is write-locked for the lifetime of the
    /// context.
    pub fn new_parent(&self) -> &DirEntryHandle {
        self.new_parent_guard.as_ref().map(|g| g.entry).unwrap_or(self.old_parent_guard.entry)
    }

    /// Returns mutable references to the `FsNodeInfo` of both parent
    /// directories.
    ///
    /// The references are returned as a tuple to allow them to be borrowed
    /// mutably at the same time (e.g., to update link counts in both).
    ///
    /// If `new_parent` is the same as `old_parent`, the second element of the
    /// tuple will be `None` to prevent mutable aliasing of the same guard.
    pub fn parent_infos_mut(
        &mut self,
    ) -> (&mut crate::vfs::FsNodeInfo, Option<&mut crate::vfs::FsNodeInfo>) {
        let old = &mut *self.old_parent_info_guard;
        let new = self.new_parent_info_guard.as_mut().map(|g| &mut **g);
        (old, new)
    }

    /// Returns a shared reference to the `FsNodeInfo` of the old parent
    /// directory.
    pub fn old_parent_info(&self) -> &crate::vfs::FsNodeInfo {
        &self.old_parent_info_guard
    }
    /// Returns a shared reference to the `FsNodeInfo` of the new parent
    /// directory.
    ///
    /// Returns `None` if `new_parent` is the same as `old_parent`.
    pub fn new_parent_info(&self) -> Option<&crate::vfs::FsNodeInfo> {
        self.new_parent_info_guard.as_deref()
    }

    fn new_parent_children(&mut self) -> &mut DirEntryLockedChildren<'a> {
        self.new_parent_guard.as_mut().unwrap_or(&mut self.old_parent_guard)
    }

    fn old_parent_children(&mut self) -> &mut DirEntryLockedChildren<'a> {
        &mut self.old_parent_guard
    }

    fn is_dir(&self, node: &FsNodeHandle) -> bool {
        if Arc::ptr_eq(node, &self.old_parent().node) {
            self.old_parent_info_guard.mode.is_dir()
        } else if Arc::ptr_eq(node, &self.new_parent().node) {
            self.new_parent_info_guard
                .as_ref()
                .map(|g| g.mode.is_dir())
                .unwrap_or_else(|| self.old_parent_info_guard.mode.is_dir())
        } else {
            let _token = allow_subclass();
            node.is_dir()
        }
    }
}

/// The Drop trait for DirEntry removes the entry from the child list of the
/// parent entry, which means we cannot drop DirEntry objects while holding a
/// lock on the parent's child list.
impl Drop for DirEntry {
    fn drop(&mut self) {
        let maybe_parent = self.parent();
        self.parent.update(None);
        if let Some(parent) = maybe_parent {
            parent.internal_remove_child(self);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_canonicalize_name() {
        let upper = canonicalize_name("FooBar.TXT");
        let lower = canonicalize_name("foobar.txt");
        let mixed = canonicalize_name("FOOBAR.TXT");
        let diff = canonicalize_name("other.txt");

        assert_eq!(upper.as_slice(), lower.as_slice());
        assert_eq!(upper.as_slice(), mixed.as_slice());
        assert_ne!(upper.as_slice(), diff.as_slice());

        // Unicode decomposed vs composed
        let composed = canonicalize_name("\u{03AA}");
        let decomposed = canonicalize_name("\u{0399}\u{0308}");
        assert_eq!(composed.as_slice(), decomposed.as_slice());

        // Unicode accent folding (e\u{0301} vs \u{00c9})
        let e_accent_lower = canonicalize_name("e\u{0301}");
        let e_accent_upper = canonicalize_name("\u{00c9}");
        assert_eq!(e_accent_lower.as_slice(), e_accent_upper.as_slice());

        // Unicode German sharp S (straße vs STRASSE)
        let strasse_lower = canonicalize_name("straße");
        let strasse_upper = canonicalize_name("STRASSE");
        assert_eq!(strasse_lower.as_slice(), strasse_upper.as_slice());

        // Non-UTF-8 preserved as-is
        let non_utf8 = canonicalize_name(b"foo\x80bar");
        assert_eq!(non_utf8.as_slice(), b"foo\x80bar");
    }

    #[test]
    fn test_dir_entry_children_casefold() {
        let mut children = DirEntryChildren::new(true);
        assert!(children.is_casefold());

        // 1. Insert and lookup casefolded UTF-8
        children.insert("FooBar.TXT".into(), Weak::new());
        assert!(children.get("foobar.txt".into()).is_some());
        assert!(children.get("FOOBAR.TXT".into()).is_some());
        assert!(children.get("FooBar.TXT".into()).is_some());
        assert!(children.get("other.txt".into()).is_none());

        // 2. Insert valid Unicode with decomposed/composed characters
        children.insert("\u{03AA}".into(), Weak::new());
        assert!(children.get("\u{0399}\u{0308}".into()).is_some());

        // 3. Insert non-UTF-8
        children.insert(b"foo\x80bar".into(), Weak::new());
        assert!(children.get(b"foo\x80bar".into()).is_some());
        assert!(children.get(b"FOO\x80bar".into()).is_none());

        // 4. Removal
        assert!(children.remove("FOOBAR.TXT".into()).is_some());
        assert!(children.get("foobar.txt".into()).is_none());
        assert!(children.remove("\u{0399}\u{0308}".into()).is_some());
        assert!(children.get("\u{03AA}".into()).is_none());
        assert!(children.remove(b"foo\x80bar".into()).is_some());
        assert!(children.get(b"foo\x80bar".into()).is_none());
        assert!(children.is_empty());

        // 5. Dynamic casefold toggling clears the cache
        children.insert("Foo.TXT".into(), Weak::new());
        assert!(!children.is_empty());
        children.set_casefold(false);
        assert!(!children.is_casefold());
        assert!(children.is_empty());
        children.set_casefold(true);
        assert!(children.is_casefold());
    }
}
