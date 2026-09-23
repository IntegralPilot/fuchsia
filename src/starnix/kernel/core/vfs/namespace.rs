// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::security;
use crate::task::{
    CurrentTask, EventHandler, Kernel, MountsWriteToken, Pid, WaitCanceler, Waiter,
    register_delayed_call,
};
use crate::time::utc;
use crate::vfs::fs_registry::FsRegistry;
use crate::vfs::pseudo::dynamic_file::{DynamicFile, DynamicFileBuf, DynamicFileSource};
use crate::vfs::pseudo::simple_file::SimpleFileNode;
use crate::vfs::socket::{SocketAddress, SocketHandle, UnixSocket};
use crate::vfs::{
    AccessCheck, DirEntry, DirEntryHandle, FileHandle, FileObject, FileOps, FileSystem,
    FileSystemHandle, FileSystemOptions, FileWriteGuardMode, FsContext, FsNode, FsNodeHandle,
    FsNodeOps, FsStr, FsString, OpenAccessCheck, PathBuilder, RenameFlags, SymlinkTarget,
    UnlinkKind, fileops_impl_dataless, fileops_impl_delegate_read_write_and_seek,
    fileops_impl_nonseekable, fileops_impl_noop_sync, fs_node_impl_not_dir,
};
use fuchsia_rcu::{RcuArc, RcuBox, RcuReadScope};
use fuchsia_rcu_collections::rcu_raw_hash_map::RcuRawHashMap;
use fuchsia_sync::Mutex;
use starnix_logging::log_warn;
use starnix_rcu::RcuHashMap;
use starnix_sync::{LockDepMutex, NamespaceFlagsLock};
use starnix_uapi::arc_key::{ArcKey, PtrKey, WeakKey};
use starnix_uapi::auth::Credentials;
use starnix_uapi::device_id::DeviceId;
use starnix_uapi::errors::Errno;
use starnix_uapi::file_mode::FileMode;
use starnix_uapi::inotify_mask::InotifyMask;
use starnix_uapi::mount_flags::{
    AtomicMountpointFlags, FileSystemFlags, MountFlags, MountpointFlags,
};
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::unmount_flags::UnmountFlags;
use starnix_uapi::vfs::{FdEvents, ResolveFlags};
use starnix_uapi::{NAME_MAX, errno, error};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::{Deref, DerefMut};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};

/// A mount namespace.
///
/// The namespace records at which entries filesystems are mounted.
#[derive(Debug)]
pub struct Namespace {
    root_mount: MountHandle,

    // Unique ID of this namespace.
    pub id: u64,
}

impl Namespace {
    pub fn new(fs: FileSystemHandle) -> Arc<Namespace> {
        Self::new_with_flags(fs, MountpointFlags::empty())
    }

    pub fn new_with_flags(fs: FileSystemHandle, flags: MountpointFlags) -> Arc<Namespace> {
        let kernel = fs.kernel.upgrade().expect("can't create namespace without a kernel");
        let mounts_guard = kernel.mounts_lock();

        let root_mount = Mount::new(&mounts_guard, WhatToMount::Fs(fs), flags)
            .expect("creating root mount for new filesystem");
        Arc::new(Self { root_mount, id: kernel.get_next_namespace_id() })
    }

    pub fn root(&self) -> NamespaceNode {
        self.root_mount.root()
    }

    pub fn kernel(&self) -> Arc<Kernel> {
        self.root_mount.kernel()
    }

    pub fn clone_namespace(&self, mounts_guard: &MountsWriteToken) -> Arc<Namespace> {
        Arc::new(Self {
            root_mount: self.root_mount.clone_mount_recursive(mounts_guard),
            id: self.kernel().get_next_namespace_id(),
        })
    }

    /// Assuming new_ns is a clone of the namespace that node is from, return the equivalent of
    /// node in new_ns. If this assumption is violated, returns None.
    pub fn translate_node(
        mut node: NamespaceNode,
        new_ns: &Namespace,
        _mounts_guard: &MountsWriteToken,
    ) -> Option<NamespaceNode> {
        // Collect the list of mountpoints that leads to this node's mount
        let mut mountpoints = vec![];
        let mut mount = node.mount;
        while let Some(mountpoint) = mount.as_ref().and_then(|m| m.mountpoint()) {
            mountpoints.push(mountpoint.entry);
            mount = mountpoint.mount;
        }

        // Follow the same path in the new namespace
        let scope = RcuReadScope::new();
        let mut mount = Arc::clone(&new_ns.root_mount);
        for mountpoint in mountpoints.iter().rev() {
            let next_mount = mount.relations.get_submount(&scope, &PtrKey::from(mountpoint))?;
            mount = next_mount;
        }
        node.mount = Some(mount).into();
        Some(node)
    }

    pub fn pivot_root(
        self: &Arc<Namespace>,
        current_root: &NamespaceNode,
        new_root: &NamespaceNode,
        put_old: &NamespaceNode,
    ) -> Result<(), Errno> {
        debug_assert!(new_root.entry.node.is_dir());
        debug_assert!(put_old.entry.node.is_dir());

        let new_root_mount = new_root.mount.as_ref().ok_or_else(|| errno!(EINVAL))?;
        let current_root_mount = current_root.mount.as_ref().ok_or_else(|| errno!(EINVAL))?;
        let put_old_mount = put_old.mount.as_ref().ok_or_else(|| errno!(EINVAL))?;

        let current_root_mountpoint =
            current_root_mount.mountpoint().ok_or_else(|| errno!(EINVAL))?;
        let new_root_mountpoint = new_root_mount.mountpoint().ok_or_else(|| errno!(EINVAL))?;

        if Arc::ptr_eq(new_root_mount, current_root_mount)
            || Arc::ptr_eq(put_old_mount, current_root_mount)
        {
            return error!(EBUSY);
        }

        let current_root_mount = current_root.mount_if_root()?;
        let new_root_mount = new_root.mount_if_root()?;

        if !new_root.is_descendant_of(current_root) {
            return error!(EINVAL);
        }
        if !put_old.is_descendant_of(new_root) {
            return error!(EINVAL);
        }

        let kernel = self.kernel();
        let mounts_guard = kernel.mounts_lock();

        // Retain mounts and mountpoint entries so that any drops triggered by
        // unlinking or reparenting under `mounts_guard` are deferred until after
        // the spinlock is released, preventing blocking operations while holding
        // the lock.
        let new_root_parent = mounts_guard.retain(
            new_root_mountpoint.mount.as_ref().expect("a mountpoint must be part of a mount"),
        );
        mounts_guard.retain(&new_root_mountpoint.entry);

        if new_root_mount.peer_group().is_some() || new_root_parent.peer_group().is_some() {
            return error!(EINVAL);
        }

        if put_old_mount.peer_group().is_some() {
            return error!(EINVAL);
        }

        if current_root_mount.peer_group().is_some() {
            return error!(EINVAL);
        }

        let current_root_parent = mounts_guard.retain(
            current_root_mountpoint.mount.as_ref().expect("a mountpoint must be part of a mount"),
        );
        mounts_guard.retain(&current_root_mountpoint.entry);
        if current_root_parent.peer_group().is_some() {
            return error!(EINVAL);
        }

        // Retain the remaining mounts and entries involved in the pivot before
        // detaching and attaching them below.
        mounts_guard.retain(current_root_mount);
        mounts_guard.retain(new_root_mount);
        mounts_guard.retain(put_old_mount);
        mounts_guard.retain(&put_old.entry);

        // 1. Detach new_root_mount from its parent.
        new_root_parent
            .remove_submount_internal(&mounts_guard, &new_root_mountpoint.mount_hash_key())?;
        new_root_mount.relations.set_mountpoint(&mounts_guard, None);

        // 2. Place new_root_mount where current_root_mount was.
        current_root_parent
            .remove_submount_internal(&mounts_guard, &current_root_mountpoint.mount_hash_key())?;
        current_root_mount.relations.set_mountpoint(&mounts_guard, None);
        current_root_parent.create_submount(
            &mounts_guard,
            &current_root_mountpoint.entry,
            WhatSubmount::Existing(Arc::clone(new_root_mount)),
        )?;

        // 3. Attach current_root_mount to put_old.
        put_old_mount.create_submount(
            &mounts_guard,
            &put_old.entry,
            WhatSubmount::Existing(Arc::clone(current_root_mount)),
        )?;

        Ok(())
    }
}

impl FsNodeOps for Arc<Namespace> {
    fs_node_impl_not_dir!();

    fn create_file_ops(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        Ok(Box::new(MountNamespaceFile(self.clone())))
    }
}

pub struct MountNamespaceFile(pub Arc<Namespace>);

impl FileOps for MountNamespaceFile {
    fileops_impl_nonseekable!();
    fileops_impl_dataless!();
    fileops_impl_noop_sync!();
}

/// An empty struct that we use to track the number of active clients for a mount.
///
/// Each active client takes a reference to this object. The unmount operation fails
/// if there are any active clients of the mount.
type MountClientMarker = Arc<()>;

/// An instance of a filesystem mounted in a namespace.
///
/// At a mount, path traversal switches from one filesystem to another.
/// The client sees a composed directory structure that glues together the
/// directories from the underlying FsNodes from those filesystems.
///
/// The mounts in a namespace form a mount tree, with `mountpoint` pointing to the parent and
/// `submounts` pointing to the children.
pub struct Mount {
    root: DirEntryHandle,
    fs: FileSystemHandle,

    /// Holds the flags specific to this mount of the underlying filesystem.
    flags: AtomicMountpointFlags,

    /// Lock used to serialize updates of `flags` to ensure consistency during remount operations.
    flags_lock: LockDepMutex<(), NamespaceFlagsLock>,

    /// A unique identifier for this mount reported in /proc/pid/mountinfo.
    id: u64,

    /// A count of the number of active clients.
    active_client_counter: MountClientMarker,

    /// The namespace node that this mount is mounted on. This is a tuple instead of a
    /// NamespaceNode because the Mount pointer has to be weak because this is the pointer to the
    /// parent mount, the parent has a pointer to the children too, and making both strong would be
    /// a cycle.
    /// Stores the relationships to other mounts (mountpoint and submounts).
    /// Both require the `MountsWriteToken` to mutate.
    relations: MountRelations,
    // Mount used to contain a Weak<Namespace>. It no longer does because since the mount point
    // hash was moved from Namespace to Mount, nothing actually uses it. Now that
    // Namespace::clone_namespace() is implemented in terms of Mount::clone_mount_recursive, it
    // won't be trivial to add it back. If you end up needing to find a Mount's Namespace, I
    // recommend turning the mountpoint field into an enum of Mountpoint or Namespace, maybe called
    // "parent", and then you can traverse up to the top of the tree.
}

type MountHandle = Arc<Mount>;

/// Public representation of the mount options.
#[derive(Clone, Debug)]
pub struct MountInfo {
    handle: Option<MountHandle>,
}

impl MountInfo {
    /// `MountInfo` for a element that is not tied to a given mount. Mount flags will be considered
    /// empty.
    pub fn detached() -> Self {
        None.into()
    }

    /// The mount flags of the represented mount.
    pub fn flags(&self) -> MountFlags {
        if let Some(handle) = &self.handle {
            handle.flags()
        } else {
            // Consider not mounted node have the NOATIME flags.
            MountFlags::NOATIME
        }
    }

    /// Checks whether this `MountInfo` represents a writable file system mount.
    pub fn check_readonly_filesystem(&self) -> Result<(), Errno> {
        if self.flags().contains(MountFlags::RDONLY) {
            return error!(EROFS);
        }
        Ok(())
    }

    /// Checks whether this `MountInfo` represents an executable file system mount.
    pub fn check_noexec_filesystem(&self) -> Result<(), Errno> {
        if self.flags().contains(MountFlags::NOEXEC) {
            return error!(EACCES);
        }
        Ok(())
    }
}

impl Deref for MountInfo {
    type Target = Option<MountHandle>;

    fn deref(&self) -> &Self::Target {
        &self.handle
    }
}

impl DerefMut for MountInfo {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.handle
    }
}

impl std::cmp::PartialEq for MountInfo {
    fn eq(&self, other: &Self) -> bool {
        self.handle.as_ref().map(Arc::as_ptr) == other.handle.as_ref().map(Arc::as_ptr)
    }
}

impl std::cmp::Eq for MountInfo {}

impl Into<MountInfo> for Option<MountHandle> {
    fn into(self) -> MountInfo {
        MountInfo { handle: self }
    }
}

#[derive(Default)]
struct MountRelations {
    /// The parent mount and the directory entry in the parent where this mount is mounted.
    mountpoint: RcuBox<Option<(Weak<Mount>, Weak<DirEntry>)>>,
    /// The active submounts, keyed by the directory entry in this mount where they are mounted.
    submounts: Mutex<HashMap<PtrKey<DirEntry>, Submount>>,
    /// A lock-free RCU friendly view of [submounts].
    submount_lookup: RcuRawHashMap<WeakKey<DirEntry>, Weak<Mount>>,
    /// The membership of this mount in its peer group.
    peer_group: RcuArc<PeerGroup>,
    /// The membership of this mount in a PeerGroup's downstream.
    upstream: RcuBox<Option<(Weak<PeerGroup>, PtrKey<Mount>)>>,
}

impl MountRelations {
    fn get_submount(
        &self,
        scope: &starnix_rcu::RcuReadScope,
        key: &PtrKey<DirEntry>,
    ) -> Option<MountHandle> {
        self.submount_lookup.get(scope, key).and_then(|weak| weak.upgrade())
    }

    fn insert_submount(
        &self,
        _guard: &MountsWriteToken,
        key: WeakKey<DirEntry>,
        value: Weak<Mount>,
        submount: Submount,
    ) -> Option<Submount> {
        let ptr_key = PtrKey::from(key.0.as_ptr());
        let old_submount = self.submounts.lock().insert(ptr_key, submount);
        let scope = starnix_rcu::RcuReadScope::new();
        // SAFETY: The MountsWriteToken proves we have exclusive write access.
        unsafe { self.submount_lookup.insert(&scope, key, value) };
        old_submount
    }

    fn remove_submount(
        &self,
        guard: &MountsWriteToken,
        key: &PtrKey<DirEntry>,
    ) -> Result<(), Errno> {
        // SAFETY: The MountsWriteToken proves we have exclusive write access.
        let _ = unsafe { self.submount_lookup.remove(key) };
        let submount = self.submounts.lock().remove(key);
        let submount = scopeguard::guard(submount, |submount| guard.defer_drop(submount));
        if submount.is_some() { Ok(()) } else { error!(EINVAL) }
    }

    fn iter_submounts<'a>(
        &'a self,
        scope: &'a starnix_rcu::RcuReadScope,
    ) -> impl Iterator<Item = (&'a WeakKey<DirEntry>, &'a Weak<Mount>)> {
        let mut cursor = self.submount_lookup.cursor(scope);
        std::iter::from_fn(move || {
            let current = cursor.current();
            if current.is_some() {
                cursor.advance();
            }
            current
        })
    }

    fn submounts_len(&self) -> usize {
        self.submount_lookup.len()
    }

    fn set_mountpoint(
        &self,
        _guard: &MountsWriteToken,
        mountpoint: Option<(Weak<Mount>, Weak<DirEntry>)>,
    ) {
        self.mountpoint.update(mountpoint);
    }

    fn mountpoint<'a>(
        &'a self,
        scope: &'a starnix_rcu::RcuReadScope,
    ) -> Option<&'a (Weak<Mount>, Weak<DirEntry>)> {
        self.mountpoint.as_ref(scope).as_ref()
    }

    fn take_peer_group(&self, _guard: &MountsWriteToken) -> Option<Arc<PeerGroup>> {
        let peer_group = self.peer_group.upgrade();
        self.peer_group.update(None);
        peer_group
    }

    fn take_upstream(&self, _guard: &MountsWriteToken) -> Option<(Weak<PeerGroup>, PtrKey<Mount>)> {
        let upstream = self.upstream.cloned();
        self.upstream.update(None);
        upstream
    }
}

/// A group of mounts. Setting MS_SHARED on a mount puts it in its own peer group. Any bind mounts
/// of a mount in the group are also added to the group. A mount created in any mount in a peer
/// group will be automatically propagated (recreated) in every other mount in the group.
#[derive(Default)]
struct PeerGroup {
    id: u64,
    mounts: RcuRawHashMap<WeakKey<Mount>, ()>,
    downstream: RcuRawHashMap<WeakKey<Mount>, ()>,
}

pub enum WhatToMount {
    Fs(FileSystemHandle),
    Bind(NamespaceNode),
}

enum WhatSubmount {
    New(WhatToMount, MountpointFlags),
    Existing(MountHandle),
}

impl Mount {
    pub fn new(
        mounts_guard: &MountsWriteToken,
        what: WhatToMount,
        mut flags: MountpointFlags,
    ) -> Result<MountHandle, Errno> {
        match what {
            WhatToMount::Fs(fs) => {
                // If `flags` does not explicitly specify an access-time flag then default to `RELATIME`.
                flags.default_atime_from(MountpointFlags::RELATIME);
                Ok(Self::new_with_root(fs.root().clone(), flags))
            }
            WhatToMount::Bind(node) => {
                // A target node may not have a mount point if it's an anonymous node. Modern Linux (6.15+)
                // returns ENOENT in this case, rather than the more intuitive EINVAL.
                let mount = node.mount.as_ref().ok_or_else(|| errno!(ENOENT))?;
                Ok(mount.clone_mount(mounts_guard, &node.entry, flags.into()))
            }
        }
    }

    fn new_with_root(root: DirEntryHandle, flags: MountpointFlags) -> MountHandle {
        let fs = root.node.fs();
        let kernel = fs.kernel.upgrade().expect("can't create mount without kernel");
        Arc::new(Self {
            id: kernel.get_next_mount_id(),
            flags: (flags & MountpointFlags::STORED_ON_MOUNT).into(),
            flags_lock: Default::default(),
            root,
            active_client_counter: Default::default(),
            fs,
            relations: Default::default(),
        })
    }

    /// A namespace node referring to the root of the mount.
    pub fn root(self: &MountHandle) -> NamespaceNode {
        NamespaceNode::new(Arc::clone(self), Arc::clone(&self.root))
    }

    fn kernel(&self) -> Arc<Kernel> {
        self.fs.kernel.upgrade().expect("No Kernel")
    }

    /// Create the specified mount as a child. Also propagate it to the mount's peer group.
    fn create_submount(
        self: &MountHandle,
        mounts_guard: &MountsWriteToken,
        dir: &DirEntryHandle,
        what: WhatSubmount,
    ) -> Result<(), Errno> {
        // Necessary to make a copy to prevent excess replication, see the comment on the
        // following Mount::new call.
        let peers = self.peer_group().map(|g| g.copy_propagation_targets()).unwrap_or_default();
        let peers = scopeguard::guard(peers, |peers| mounts_guard.defer_drop(peers));

        // Create the mount after copying the peer list, because in the case of creating a bind
        // mount inside itself, the new mount would get added to our peer group during the
        // Mount::new call, but we don't want to replicate into it already. For an example see
        // MountTest.QuizBRecursion.
        let mount = match what {
            WhatSubmount::Existing(mount) => mount,
            WhatSubmount::New(what, flags) => Mount::new(mounts_guard, what, flags)?,
        };

        if self.is_shared() {
            mount.make_shared(mounts_guard);
        }

        for peer in &*peers {
            if Arc::ptr_eq(self, peer) {
                continue;
            }
            let clone = mount.clone_mount_recursive(mounts_guard);
            peer.add_submount_internal(mounts_guard, dir, clone);
        }

        self.add_submount_internal(mounts_guard, dir, mount);
        Ok(())
    }

    fn remove_submount(
        self: &MountHandle,
        mounts_guard: &MountsWriteToken,
        mount_hash_key: &PtrKey<DirEntry>,
    ) -> Result<(), Errno> {
        // create_submount explains why we need to make a copy of peers.
        let peers = self.peer_group().map(|g| g.copy_propagation_targets()).unwrap_or_default();
        let peers = scopeguard::guard(peers, |peers| mounts_guard.defer_drop(peers));

        for peer in &*peers {
            if Arc::ptr_eq(self, peer) {
                continue;
            }
            // mount_namespaces(7): If B is shared, then all most-recently-mounted mounts at b on
            // mounts that receive propagation from mount B and do not have submounts under them are
            // unmounted.
            let scope = RcuReadScope::new();
            if let Some(submount) = peer.relations.get_submount(&scope, mount_hash_key) {
                if submount.relations.submounts_len() != 0 {
                    continue;
                }
            }
            let _ = peer.remove_submount_internal(mounts_guard, mount_hash_key);
        }

        self.remove_submount_internal(mounts_guard, mount_hash_key)
    }

    pub fn move_mount(
        source_mount: &MountHandle,
        target_mount: &MountHandle,
        target_dir: &DirEntryHandle,
    ) -> Result<(), Errno> {
        let kernel = target_mount.kernel();
        let mounts_guard = kernel.mounts_lock();

        let source_mountpoint = source_mount.mountpoint().ok_or_else(|| errno!(EIO))?;
        let source_parent = mounts_guard.retain(
            source_mountpoint.mount.as_ref().expect("a mountpoint must be part of a mount"),
        );
        mounts_guard.retain(&source_mountpoint.entry);

        // First, disconnect the mount from its parent.
        {
            if source_parent.peer_group().is_some() {
                // Sayeth mount(2):
                // EINVAL A move operation (MS_MOVE) was attempted, but the parent mount of source
                //        mount has propagation type MS_SHARED.
                return error!(EINVAL);
            }
            source_parent
                .remove_submount_internal(&mounts_guard, &source_mountpoint.mount_hash_key())?;
            source_mount.relations.set_mountpoint(&mounts_guard, None);
        }

        target_mount.create_submount(
            &mounts_guard,
            target_dir,
            WhatSubmount::Existing(Arc::clone(source_mount)),
        )?;
        Ok(())
    }

    /// Create a new mount with the same filesystem, flags, and peer group. Used to implement bind
    /// mounts.
    fn clone_mount(
        self: &MountHandle,
        mounts_guard: &MountsWriteToken,
        new_root: &DirEntryHandle,
        flags: MountFlags,
    ) -> MountHandle {
        assert!(new_root.is_descendant_of(&self.root));
        // According to mount(2) on bind mounts, all flags other than MS_REC are ignored when doing
        // a bind mount.
        let clone = Self::new_with_root(Arc::clone(new_root), self.mount_flags());

        if flags.contains(MountFlags::REC) {
            for (dir, submount) in self.relations.iter_submounts(&RcuReadScope::new()) {
                if let (Some(dir), Some(submount)) = (dir.0.upgrade(), submount.upgrade()) {
                    let submount = submount.clone_mount_recursive(mounts_guard);
                    clone.add_submount_internal(mounts_guard, &dir, submount);
                }
            }
        }

        // Put the clone in the same peer group
        let peer_group = self.peer_group();
        if let Some(peer_group) = peer_group {
            clone.set_peer_group(mounts_guard, peer_group);
        }

        clone
    }

    /// Do a clone of the full mount hierarchy below this mount. Used for creating mount
    /// namespaces and creating copies to use for propagation.
    fn clone_mount_recursive(self: &MountHandle, mounts_guard: &MountsWriteToken) -> MountHandle {
        self.clone_mount(mounts_guard, &self.root, MountFlags::REC)
    }

    pub fn change_propagation(
        self: &MountHandle,
        mounts_guard: &MountsWriteToken,
        flag: MountFlags,
        recursive: bool,
    ) {
        match flag {
            MountFlags::SHARED => self.make_shared(mounts_guard),
            MountFlags::PRIVATE => self.make_private(mounts_guard),
            MountFlags::DOWNSTREAM => self.make_downstream(mounts_guard),
            _ => {
                log_warn!("mount propagation {:?}", flag);
            }
        }

        if recursive {
            for (_, submount) in self.relations.iter_submounts(&starnix_rcu::RcuReadScope::new()) {
                if let Some(submount) = submount.upgrade() {
                    submount.change_propagation(mounts_guard, flag, recursive);
                }
            }
        }
    }

    /// Returns the effective flags for the `Mount`, calculated as the union of the mount flags
    /// associated with the `FileSystem`, and with the `Mount` itself.
    fn flags(&self) -> MountFlags {
        MountFlags::from(self.mount_flags()) | self.fs_flags().into()
    }

    /// Returns the mount flags stored unique to this `Mount`.
    fn mount_flags(&self) -> MountpointFlags {
        self.flags.load(Ordering::Relaxed)
    }

    /// Returns the mount flags for the `FileSystem` of this `Mount`.
    fn fs_flags(&self) -> FileSystemFlags {
        self.fs.options.flags.load(Ordering::Relaxed)
    }

    /// Updates the `Mount` with the per-mount flags specified in `flags`, while preserving the
    /// existing access-time flag if no access-time flag is set in `flags`.
    pub fn update_flags(self: &MountHandle, mut flags: MountpointFlags) {
        let _lock = self.flags_lock.lock();
        // Since Linux 3.17, if none of MS_NOATIME, MS_NODIRATIME,
        // MS_RELATIME, or MS_STRICTATIME is specified in mountflags, then
        // the remount operation preserves the existing values of these
        // flags (rather than defaulting to MS_RELATIME).
        flags.default_atime_from(self.flags.load(Ordering::Relaxed));
        flags &= MountpointFlags::STORED_ON_MOUNT;
        self.flags.store(flags, Ordering::Relaxed);
    }

    /// The number of active clients of this mount.
    ///
    /// The mount cannot be unmounted if there are any active clients.
    fn active_clients(&self) -> usize {
        // We need to subtract one for our own reference. We are not a real client.
        Arc::strong_count(&self.active_client_counter) - 1
    }

    pub fn unmount(
        &self,
        mounts_guard: &MountsWriteToken,
        flags: UnmountFlags,
    ) -> Result<(), Errno> {
        if !flags.contains(UnmountFlags::DETACH) {
            if self.active_clients() > 0 || self.relations.submounts_len() != 0 {
                return error!(EBUSY);
            }
        }

        let mountpoint = self.mountpoint().ok_or_else(|| errno!(EINVAL))?;
        let parent_mount = mountpoint.mount.as_ref().expect("a mountpoint must be part of a mount");
        parent_mount.remove_submount(mounts_guard, &mountpoint.mount_hash_key())
    }

    /// Returns the security state of the fs.
    pub fn security_state(&self) -> &security::FileSystemState {
        &self.fs.security_state
    }

    /// Returns the name of the fs.
    pub fn fs_name(&self) -> &'static FsStr {
        self.fs.name()
    }

    /// Reconfigures the flags for the `FileSystem` backing this mount point.
    pub fn reconfigure_fs(
        &self,
        current_task: &CurrentTask,
        flags: FileSystemFlags,
    ) -> Result<(), Errno> {
        self.fs.update_flags(current_task, flags)
    }

    /// Returns true if there is a submount on top of `dir_entry`.
    pub fn has_submount(&self, dir_entry: &DirEntryHandle) -> bool {
        let scope = RcuReadScope::new();
        self.relations.get_submount(&scope, &PtrKey::from(dir_entry)).is_some()
    }

    /// The NamespaceNode on which this Mount is mounted.
    pub fn mountpoint(&self) -> Option<NamespaceNode> {
        let scope = RcuReadScope::new();
        let (mount, entry) = self.relations.mountpoint(&scope)?;
        Some(NamespaceNode::new(mount.upgrade()?, entry.upgrade()?))
    }

    /// Add a child mount *without propagating it to the peer group*. For internal use only.
    pub fn add_submount_internal(
        self: &MountHandle,
        guard: &MountsWriteToken,
        dir: &DirEntryHandle,
        mount: MountHandle,
    ) {
        if !dir.is_descendant_of(&self.root) {
            return;
        }

        let submount = mount.kernel().mounts.register_mount(dir, mount.clone());

        let old_mountpoint = {
            let scope = RcuReadScope::new();
            mount.relations.mountpoint(&scope).map(|x| x.clone())
        };
        mount.relations.set_mountpoint(guard, Some((Arc::downgrade(self), Arc::downgrade(dir))));
        assert!(old_mountpoint.is_none(), "add_submount can only take a newly created mount");

        let old_mount = self.relations.insert_submount(
            guard,
            WeakKey::from(dir),
            Arc::downgrade(&mount),
            submount,
        );

        if let Some(old_mount) = old_mount {
            old_mount
                .mount
                .relations
                .set_mountpoint(guard, Some((Arc::downgrade(&mount), Arc::downgrade(&mount.root))));
            let new_old_submount =
                mount.kernel().mounts.register_mount(&mount.root, old_mount.mount.clone());
            mount.relations.insert_submount(
                guard,
                WeakKey::from(&mount.root),
                Arc::downgrade(&old_mount.mount),
                new_old_submount,
            );
        }
    }

    pub fn remove_submount_internal(
        self: &MountHandle,
        guard: &MountsWriteToken,
        mount_hash_key: &PtrKey<DirEntry>,
    ) -> Result<(), Errno> {
        self.relations.remove_submount(guard, mount_hash_key)
    }

    /// Return this mount's current peer group.
    fn peer_group(&self) -> Option<Arc<PeerGroup>> {
        self.relations.peer_group.upgrade()
    }

    /// Handles unregistering from both peer group and upstream simultaneously.
    /// This resolves forwarding the upstream to the next mount in the peer group if necessary.
    fn unregister_from_peer_group_and_upstream(
        guard: &MountsWriteToken,
        peer_group: Option<Arc<PeerGroup>>,
        upstream: Option<(Weak<PeerGroup>, PtrKey<Mount>)>,
        mount_ptr: PtrKey<Mount>,
    ) {
        let upstream_group = match upstream {
            Some((weak_group, mount)) => {
                if let Some(group) = weak_group.upgrade() {
                    group.remove_downstream(guard, mount);
                    Some(group)
                } else {
                    None
                }
            }
            None => None,
        };

        if let Some(group) = peer_group {
            group.remove(guard, mount_ptr);

            if let Some(upstream_group) = upstream_group {
                let next_mount = {
                    let scope = RcuReadScope::new();
                    group.mounts.keys(&scope).next().and_then(|w| w.0.upgrade())
                };
                if let Some(next_mount) = next_mount {
                    next_mount.set_upstream(guard, upstream_group);
                }
            }
        }
    }

    /// Remove this mount from its peer group.
    fn take_from_peer_group(&self, guard: &MountsWriteToken) -> Option<Arc<PeerGroup>> {
        let peer_group = self.relations.take_peer_group(guard);
        if peer_group.is_none() {
            return None;
        }
        let upstream = self.relations.take_upstream(guard);
        let return_group = peer_group.clone();
        Mount::unregister_from_peer_group_and_upstream(
            guard,
            peer_group,
            upstream,
            PtrKey::from(self),
        );
        return_group
    }

    fn upstream(&self) -> Option<Arc<PeerGroup>> {
        let scope = RcuReadScope::new();
        let (group, _) = self.relations.upstream.as_ref(&scope).as_ref()?;
        group.upgrade()
    }

    fn remove_from_upstream(&self, guard: &MountsWriteToken) {
        let upstream = self.relations.take_upstream(guard);
        if let Some((weak_group, mount)) = upstream {
            if let Some(group) = weak_group.upgrade() {
                group.remove_downstream(guard, mount);
            }
        }
    }

    /// Set this mount's peer group.
    fn set_peer_group(self: &Arc<Mount>, guard: &MountsWriteToken, group: Arc<PeerGroup>) {
        self.take_from_peer_group(guard);
        group.add(guard, self);
        self.relations.peer_group.update(Some(group));
    }

    fn set_upstream(self: &Arc<Mount>, guard: &MountsWriteToken, group: Arc<PeerGroup>) {
        self.remove_from_upstream(guard);
        group.add_downstream(guard, self);
        self.relations.upstream.update(Some((Arc::downgrade(&group), Arc::as_ptr(self).into())));
    }

    /// Is the mount in a peer group? Corresponds to MS_SHARED.
    pub fn is_shared(&self) -> bool {
        self.relations.peer_group.is_some()
    }

    /// Put the mount in a peer group. Implements MS_SHARED.
    fn make_shared(self: &Arc<Mount>, guard: &MountsWriteToken) {
        if self.is_shared() {
            return;
        }
        let kernel = self.kernel();
        self.set_peer_group(guard, PeerGroup::new(kernel.get_next_peer_group_id()))
    }

    /// Take the mount out of its peer group, also remove upstream if any. Implements MS_PRIVATE.
    fn make_private(&self, guard: &MountsWriteToken) {
        let peer_group = self.relations.take_peer_group(guard);
        let upstream = self.relations.take_upstream(guard);
        Mount::unregister_from_peer_group_and_upstream(
            guard,
            peer_group,
            upstream,
            PtrKey::from(self),
        );
    }

    /// Take the mount out of its peer group and make it downstream instead. Implements
    /// MountFlags::DOWNSTREAM (MS_SLAVE).
    fn make_downstream(self: &Arc<Mount>, guard: &MountsWriteToken) {
        if let Some(peer_group) = self.take_from_peer_group(guard) {
            self.set_upstream(guard, peer_group);
        }
    }
}

impl PeerGroup {
    fn new(id: u64) -> Arc<Self> {
        Arc::new(Self {
            id,
            mounts: RcuRawHashMap::default(),
            downstream: RcuRawHashMap::default(),
        })
    }

    fn add(&self, _guard: &MountsWriteToken, mount: &Arc<Mount>) {
        // SAFETY: The MountsWriteToken proves we have exclusive write access.
        unsafe { self.mounts.insert(&starnix_rcu::RcuReadScope::new(), WeakKey::from(mount), ()) };
    }

    fn remove(&self, _guard: &MountsWriteToken, mount: PtrKey<Mount>) {
        // SAFETY: The MountsWriteToken proves we have exclusive write access.
        unsafe { self.mounts.remove(&mount) };
    }

    fn add_downstream(&self, _guard: &MountsWriteToken, mount: &Arc<Mount>) {
        // SAFETY: The MountsWriteToken proves we have exclusive write access.
        unsafe {
            self.downstream.insert(&starnix_rcu::RcuReadScope::new(), WeakKey::from(mount), ())
        };
    }

    fn remove_downstream(&self, _guard: &MountsWriteToken, mount: PtrKey<Mount>) {
        // SAFETY: The MountsWriteToken proves we have exclusive write access.
        unsafe { self.downstream.remove(&mount) };
    }

    fn copy_propagation_targets(&self) -> Vec<MountHandle> {
        let mut buf = vec![];
        self.collect_propagation_targets(&mut buf);
        buf
    }

    fn collect_propagation_targets(&self, buf: &mut Vec<MountHandle>) {
        let downstream_mounts: Vec<_> = {
            let scope = RcuReadScope::new();
            buf.extend(self.mounts.keys(&scope).filter_map(|m| m.0.upgrade()));
            self.downstream.keys(&scope).filter_map(|m| m.0.upgrade()).collect()
        };
        for mount in downstream_mounts {
            let peer_group = mount.peer_group();
            match peer_group {
                Some(group) => group.collect_propagation_targets(buf),
                None => buf.push(mount),
            }
        }
    }
}

impl Kernel {
    pub fn get_next_mount_id(&self) -> u64 {
        self.next_mount_id.next()
    }

    pub fn get_next_peer_group_id(&self) -> u64 {
        self.next_peer_group_id.next()
    }

    pub fn get_next_namespace_id(&self) -> u64 {
        self.next_namespace_id.next()
    }
}

impl CurrentTask {
    pub fn create_filesystem(
        &self,
        fs_type: &FsStr,
        options: FileSystemOptions,
    ) -> Result<FileSystemHandle, Errno> {
        // Please register new file systems via //src/starnix/modules/lib.rs, even if the file
        // system is implemented inside starnix_core.
        //
        // Most file systems should be implemented as modules. The VFS provides various traits that
        // let starnix_core integrate file systems without needing to depend on the file systems
        // directly.
        self.kernel()
            .expando
            .get::<FsRegistry>()
            .create(self, fs_type, options)
            .ok_or_else(|| errno!(ENODEV, fs_type))?
    }
}

struct ProcMountsFileSource {
    tid: Pid,
}

impl DynamicFileSource for ProcMountsFileSource {
    fn generate(
        &self,
        _current_task: &CurrentTask,
        sink: &mut DynamicFileBuf,
    ) -> Result<(), Errno> {
        // TODO(tbodt): We should figure out a way to have a real iterator instead of grabbing the
        // entire list in one go. Should we have a BTreeMap<u64, Weak<Mount>> in the Namespace?
        // Also has the benefit of correct (i.e. chronological) ordering. But then we have to do
        // extra work to maintain it.
        let task = self.tid.get_task()?;
        let task_fs = task.running_state()?.fs();
        let root = task_fs.root();
        let ns = task_fs.namespace();
        for_each_mount(&ns.root_mount, &mut |mount| {
            let mountpoint = mount.mountpoint().unwrap_or_else(|| mount.root());
            if !mountpoint.is_descendant_of(&root) {
                return Ok(());
            }
            write!(
                sink,
                "{} {} {} {}{}",
                mount.fs.options.source_for_display(),
                mountpoint.path(&task_fs),
                mount.fs.name(),
                // Report the union of the FileSystem and Mount flags, as well as any FileSystem-
                // or LSM-specific options.
                mount.flags(),
                security::sb_show_options(&task.kernel(), &mount.fs)?,
            )?;
            writeln!(sink, " 0 0")?;
            Ok(())
        })?;
        Ok(())
    }
}

pub struct ProcMountsFile {
    dynamic_file: DynamicFile<ProcMountsFileSource>,
}

impl ProcMountsFile {
    pub fn new_node(tid: Pid) -> impl FsNodeOps {
        SimpleFileNode::new(move |_| {
            Ok(Self { dynamic_file: DynamicFile::new(ProcMountsFileSource { tid: tid.clone() }) })
        })
    }
}

impl FileOps for ProcMountsFile {
    fileops_impl_delegate_read_write_and_seek!(self, self.dynamic_file);
    fileops_impl_noop_sync!();

    fn wait_async(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        waiter: &Waiter,
        _events: FdEvents,
        _handler: EventHandler,
    ) -> Option<WaitCanceler> {
        // Polling this file gives notifications when any change to mounts occurs. This is not
        // implemented yet, but stubbed for Android init.
        Some(waiter.fake_wait())
    }

    fn query_events(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
    ) -> Result<FdEvents, Errno> {
        Ok(FdEvents::empty())
    }
}

#[derive(Clone)]
pub struct ProcMountinfoFile {
    tid: Pid,
}
impl ProcMountinfoFile {
    pub fn new_node(tid: Pid) -> impl FsNodeOps {
        DynamicFile::new_node(Self { tid })
    }
}
impl DynamicFileSource for ProcMountinfoFile {
    fn generate(
        &self,
        _current_task: &CurrentTask,
        sink: &mut DynamicFileBuf,
    ) -> Result<(), Errno> {
        // Returns path to the `dir` from the root of the file system.
        fn path_from_fs_root(dir: &DirEntryHandle) -> FsString {
            let mut path = PathBuilder::new();
            if dir.is_dead() {
                // Return `/foo/dir//deleted` if the dir was deleted.
                path.prepend_element("/deleted".into());
            }
            let scope = RcuReadScope::new();
            let mut current = dir.clone();
            while let Some(parent) = current.parent() {
                path.prepend_element(current.local_name(&scope));
                current = parent;
            }
            path.build_absolute()
        }

        // TODO(tbodt): We should figure out a way to have a real iterator instead of grabbing the
        // entire list in one go. Should we have a BTreeMap<u64, Weak<Mount>> in the Namespace?
        // Also has the benefit of correct (i.e. chronological) ordering. But then we have to do
        // extra work to maintain it.
        let task = self.tid.get_task()?;
        let task_fs = task.running_state()?.fs();
        let root = task_fs.root();
        let ns = task_fs.namespace();
        for_each_mount(&ns.root_mount, &mut |mount| {
            let mountpoint = mount.mountpoint().unwrap_or_else(|| mount.root());
            if !mountpoint.is_descendant_of(&root) {
                return Ok(());
            }
            // Can't fail, mountpoint() and root() can't return a NamespaceNode with no mount
            let parent = mountpoint.mount.as_ref().unwrap();
            write!(
                sink,
                "{} {} {} {} {} {}",
                mount.id,
                parent.id,
                mount.root.node.fs().dev_id,
                path_from_fs_root(&mount.root),
                mountpoint.path(&task_fs),
                mount.mount_flags(),
            )?;
            if let Some(peer_group) = mount.peer_group() {
                write!(sink, " shared:{}", peer_group.id)?;
            }
            if let Some(upstream) = mount.upstream() {
                write!(sink, " master:{}", upstream.id)?;
            }
            writeln!(
                sink,
                " - {} {} {}{}",
                mount.fs.name(),
                mount.fs.options.source_for_display(),
                mount.fs_flags(),
                // LSM options are associated with the FileSystem rather than the Mount.
                security::sb_show_options(&task.kernel(), &mount.fs)?
            )?;
            Ok(())
        })?;
        Ok(())
    }
}

fn for_each_mount<E>(
    mount: &MountHandle,
    callback: &mut impl FnMut(&MountHandle) -> Result<(), E>,
) -> Result<(), E> {
    callback(mount)?;
    for (_, s) in mount.relations.iter_submounts(&RcuReadScope::new()) {
        if let Some(submount) = s.upgrade() {
            for_each_mount(&submount, callback)?;
        }
    }
    Ok(())
}

/// The `SymlinkMode` enum encodes how symlinks are followed during path traversal.
#[derive(Default, PartialEq, Eq, Copy, Clone, Debug)]
pub enum SymlinkMode {
    /// Follow a symlink at the end of a path resolution.
    #[default]
    Follow,

    /// Do not follow a symlink at the end of a path resolution.
    NoFollow,
}

/// The `DirectoryMode` enum encodes whether a lookup requires the target to be a directory.
#[derive(Default, PartialEq, Eq, Copy, Clone, Debug)]
pub enum DirectoryMode {
    /// The target of the path resolution can be any file type.
    #[default]
    AllowAny,

    /// The target of the path resolution must be a directory.
    MustBeDirectory,
}

/// The maximum number of symlink traversals that can be made during path resolution.
pub const MAX_SYMLINK_FOLLOWS: u8 = 40;

/// The context passed during namespace lookups.
///
/// Namespace lookups need to mutate a shared context in order to correctly
/// count the number of remaining symlink traversals.
pub struct LookupContext {
    /// The SymlinkMode for the lookup.
    ///
    /// As the lookup proceeds, the follow count is decremented each time the
    /// lookup traverses a symlink.
    pub symlink_mode: SymlinkMode,

    /// The number of symlinks remaining the follow.
    ///
    /// Each time path resolution calls readlink, this value is decremented.
    pub remaining_follows: u8,

    /// Whether the result of the lookup must be a directory.
    ///
    /// For example, if the path ends with a `/` or if userspace passes
    /// O_DIRECTORY. This flag can be set if the lookup encounters a
    /// symlink that ends with a `/`.
    pub directory_mode: DirectoryMode,

    /// Resolve flags passed to `openat2`. Empty if the lookup originated in any other syscall.
    pub resolve_flags: ResolveFlags,

    /// Base directory for the lookup. Set only when either `RESOLVE_BENEATH` or `RESOLVE_IN_ROOT`
    /// is passed to `openat2`.
    pub resolve_base: ResolveBase,
}

/// Used to specify base directory in `LookupContext` for lookups originating in the `openat2`
/// syscall with either `RESOLVE_BENEATH` or `RESOLVE_IN_ROOT` flag.
#[derive(Clone, Eq, PartialEq)]
pub enum ResolveBase {
    None,

    /// The lookup is not allowed to traverse any node that's not beneath the specified node.
    Beneath(NamespaceNode),

    /// The lookup should be handled as if the root specified node is the file-system root.
    InRoot(NamespaceNode),
}

impl LookupContext {
    pub fn new(symlink_mode: SymlinkMode) -> LookupContext {
        LookupContext {
            symlink_mode,
            remaining_follows: MAX_SYMLINK_FOLLOWS,
            directory_mode: DirectoryMode::default(),
            resolve_flags: ResolveFlags::empty(),
            resolve_base: ResolveBase::None,
        }
    }

    pub fn with(&self, symlink_mode: SymlinkMode, directory_mode: DirectoryMode) -> LookupContext {
        LookupContext {
            symlink_mode,
            directory_mode,
            resolve_base: self.resolve_base.clone(),
            ..*self
        }
    }

    pub fn update_for_path(&mut self, path: &FsStr) {
        if path.last() == Some(&b'/') {
            // The last path element must resolve to a directory. This is because a trailing slash
            // was found in the path.
            self.directory_mode = DirectoryMode::MustBeDirectory;
            // If the last path element is a symlink, we should follow it.
            // See https://pubs.opengroup.org/onlinepubs/9699919799/xrat/V4_xbd_chap03.html#tag_21_03_00_75
            self.symlink_mode = SymlinkMode::Follow;
        }
    }
}

impl Default for LookupContext {
    fn default() -> Self {
        LookupContext::new(SymlinkMode::Follow)
    }
}

/// Whether the path is reachable from the given root.
pub enum PathWithReachability {
    /// The path is reachable from the given root.
    Reachable(FsString),

    /// The path is not reachable from the given root.
    Unreachable(FsString),
}

impl PathWithReachability {
    pub fn into_path(self) -> FsString {
        match self {
            PathWithReachability::Reachable(path) => path,
            PathWithReachability::Unreachable(path) => path,
        }
    }
}

/// A node in a mount namespace.
///
/// This tree is a composite of the mount tree and the FsNode tree.
///
/// These nodes are used when traversing paths in a namespace in order to
/// present the client the directory structure that includes the mounted
/// filesystems.
#[derive(Clone)]
pub struct NamespaceNode {
    /// The mount where this namespace node is mounted.
    ///
    /// A given FsNode can be mounted in multiple places in a namespace. This
    /// field distinguishes between them.
    pub mount: MountInfo,

    /// The FsNode that corresponds to this namespace entry.
    pub entry: DirEntryHandle,
}

impl NamespaceNode {
    pub fn new(mount: MountHandle, entry: DirEntryHandle) -> Self {
        Self { mount: Some(mount).into(), entry }
    }

    /// Create a namespace node that is not mounted in a namespace.
    pub fn new_anonymous(entry: DirEntryHandle) -> Self {
        Self { mount: None.into(), entry }
    }

    /// Create a namespace node that is not mounted in a namespace and that refers to a node that
    /// is not rooted in a hierarchy and has no name.
    pub fn new_anonymous_unrooted(current_task: &CurrentTask, node: FsNodeHandle) -> Self {
        let dir_entry = DirEntry::new_unrooted(node);
        let _ = security::fs_node_init_with_dentry_no_xattr(current_task, &dir_entry);
        Self::new_anonymous(dir_entry)
    }

    /// Create a FileObject corresponding to this namespace node.
    ///
    /// This function is the primary way of instantiating FileObjects. Each
    /// FileObject records the NamespaceNode that created it in order to
    /// remember its path in the Namespace.
    pub fn open(
        &self,
        current_task: &CurrentTask,
        open_check: impl Into<OpenAccessCheck>,
    ) -> Result<FileHandle, Errno> {
        let open_check = open_check.into();
        let ops = self.entry.node.open(current_task, self, open_check)?;
        FileObject::new(current_task, ops, self.clone(), open_check.open_flags())
    }

    /// Create or open a node in the file system.
    ///
    /// Works for any type of node other than a symlink.
    ///
    /// Will return an existing node unless `flags` contains `OpenFlags::EXCL`.
    pub fn open_create_node(
        &self,
        current_task: &CurrentTask,
        name: &FsStr,
        mode: FileMode,
        dev: DeviceId,
        flags: OpenFlags,
    ) -> Result<NamespaceNode, Errno> {
        let owner = current_task.current_fscred();
        let mode = current_task.fs().apply_umask(mode);
        let create_fn = |dir: &FsNodeHandle, mount: &MountInfo, name: &_| {
            dir.create_node(current_task, mount, name, mode, dev, owner)
        };
        let entry = if flags.contains(OpenFlags::EXCL) {
            self.entry.create_entry(current_task, &self.mount, name, create_fn)
        } else {
            self.entry.get_or_create_entry(current_task, &self.mount, name, create_fn)
        }?;
        Ok(self.with_new_entry(entry))
    }

    pub fn into_active(self) -> ActiveNamespaceNode {
        ActiveNamespaceNode::new(self)
    }

    /// Create a node in the file system.
    ///
    /// Works for any type of node other than a symlink.
    ///
    /// Does not return an existing node.
    pub fn create_node(
        &self,
        current_task: &CurrentTask,
        name: &FsStr,
        mode: FileMode,
        dev: DeviceId,
    ) -> Result<NamespaceNode, Errno> {
        let owner = current_task.current_fscred();
        let mode = current_task.fs().apply_umask(mode);
        let entry =
            self.entry.create_entry(current_task, &self.mount, name, |dir, mount, name| {
                dir.create_node(current_task, mount, name, mode, dev, owner)
            })?;
        Ok(self.with_new_entry(entry))
    }

    /// Create a symlink in the file system.
    ///
    /// To create another type of node, use `create_node`.
    pub fn create_symlink(
        &self,
        current_task: &CurrentTask,
        name: &FsStr,
        target: &FsStr,
    ) -> Result<NamespaceNode, Errno> {
        let owner = current_task.current_fscred();
        let entry =
            self.entry.create_entry(current_task, &self.mount, name, |dir, mount, name| {
                dir.create_symlink(current_task, mount, name, target, owner)
            })?;
        Ok(self.with_new_entry(entry))
    }

    /// Creates an anonymous file.
    ///
    /// The FileMode::IFMT of the FileMode is always FileMode::IFREG.
    ///
    /// Used by O_TMPFILE.
    pub fn create_tmpfile(
        &self,
        current_task: &CurrentTask,
        mode: FileMode,
        flags: OpenFlags,
    ) -> Result<NamespaceNode, Errno> {
        let owner = current_task.current_fscred();
        let mode = current_task.fs().apply_umask(mode);
        Ok(self.with_new_entry(self.entry.create_tmpfile(
            current_task,
            &self.mount,
            mode,
            owner,
            flags,
        )?))
    }

    pub fn link(
        &self,
        current_task: &CurrentTask,
        name: &FsStr,
        child: &FsNodeHandle,
    ) -> Result<NamespaceNode, Errno> {
        let dir_entry =
            self.entry.create_entry(current_task, &self.mount, name, |dir, mount, name| {
                dir.link(current_task, mount, name, child)
            })?;
        Ok(self.with_new_entry(dir_entry))
    }

    pub fn bind_socket(
        &self,
        current_task: &CurrentTask,
        name: &FsStr,
        socket: SocketHandle,
        socket_address: SocketAddress,
        mode: FileMode,
    ) -> Result<NamespaceNode, Errno> {
        let dir_entry =
            self.entry.create_entry(current_task, &self.mount, name, |dir, mount, name| {
                let node = dir.create_node(
                    current_task,
                    mount,
                    name,
                    mode,
                    DeviceId::NONE,
                    current_task.current_fscred(),
                )?;
                if let Some(unix_socket) = socket.downcast_socket::<UnixSocket>() {
                    unix_socket.bind_socket_to_node(&socket, socket_address, &node)?;
                } else {
                    return error!(ENOTSUP);
                }
                Ok(node)
            })?;
        Ok(self.with_new_entry(dir_entry))
    }

    pub fn unlink(
        &self,
        current_task: &CurrentTask,
        name: &FsStr,
        kind: UnlinkKind,
        directory_mode: DirectoryMode,
    ) -> Result<(), Errno> {
        if DirEntry::is_reserved_name(name) {
            match kind {
                UnlinkKind::Directory => {
                    if name == ".." {
                        error!(ENOTEMPTY)
                    } else if self.parent().is_none() {
                        // The client is attempting to remove the root.
                        error!(EBUSY)
                    } else {
                        error!(EINVAL)
                    }
                }
                UnlinkKind::NonDirectory => error!(ENOTDIR),
            }
        } else {
            self.entry.unlink(current_task, &self.mount, name, kind, directory_mode)
        }
    }

    // Resolve the current node.
    //
    // Depending on context, this will resolve symlink and mount point.
    fn resolve(
        self,
        current_task: &CurrentTask,
        context: &mut LookupContext,
    ) -> Result<NamespaceNode, Errno> {
        let mut node = self;

        loop {
            if !node.entry.node.is_lnk() || context.symlink_mode == SymlinkMode::NoFollow {
                break;
            }
            if context.remaining_follows == 0
                || context.resolve_flags.contains(ResolveFlags::NO_SYMLINKS)
            {
                return error!(ELOOP);
            }
            context.remaining_follows -= 1;
            node = match node.readlink(current_task)? {
                SymlinkTarget::Path(link_target) => {
                    let link_directory = if link_target[0] == b'/' {
                        // If the path is absolute, we'll resolve the root directory.
                        match &context.resolve_base {
                            ResolveBase::None => current_task.fs().root(),
                            ResolveBase::Beneath(_) => return error!(EXDEV),
                            ResolveBase::InRoot(root) => root.clone(),
                        }
                    } else {
                        // If the path is not absolute, it's a relative directory.
                        // Let's try to get the parent of the current node, or in the case that
                        // the node is the root we can just use that directly.
                        node.parent().unwrap_or(node)
                    };
                    current_task.lookup_path(context, link_directory, link_target.as_ref())?
                }
                SymlinkTarget::Node(node) => {
                    if context.resolve_flags.contains(ResolveFlags::NO_MAGICLINKS) {
                        return error!(ELOOP);
                    }
                    node
                }
            };
        }
        Ok(node.enter_mount())
    }

    /// Traverse down a parent-to-child link in the namespace.
    pub fn lookup_child(
        &self,
        current_task: &CurrentTask,
        context: &mut LookupContext,
        basename: &FsStr,
    ) -> Result<NamespaceNode, Errno> {
        self.lookup_children(current_task, context, &[basename])
    }

    /// Traverse down a parent-to-child link in the namespace.
    pub fn lookup_children(
        &self,
        current_task: &CurrentTask,
        context: &mut LookupContext,
        mut basenames: &[&FsStr],
    ) -> Result<NamespaceNode, Errno> {
        for name in basenames {
            if name.len() > NAME_MAX as usize {
                return error!(ENAMETOOLONG);
            }
        }

        let mut current_namespace_node = self.clone();

        while basenames.len() > 0 {
            if !current_namespace_node.entry.node.is_dir() {
                return error!(ENOTDIR);
            }

            let basename = basenames[0];
            if basename.is_empty() || basename == "." {
                basenames = &basenames[1..];
                continue;
            }
            if basename == ".." {
                let root = match &context.resolve_base {
                    ResolveBase::None => current_task.fs().root(),
                    ResolveBase::Beneath(node) => {
                        // Do not allow traversal out of the 'node'.
                        if current_namespace_node == *node {
                            return error!(EXDEV);
                        }
                        current_task.fs().root()
                    }
                    ResolveBase::InRoot(root) => root.clone(),
                };

                // Make sure this can't escape a chroot.
                if current_namespace_node != root {
                    current_namespace_node =
                        current_namespace_node.parent().unwrap_or(current_namespace_node)
                }
                if context.resolve_flags.contains(ResolveFlags::NO_XDEV)
                    && current_namespace_node.mount != self.mount
                {
                    return error!(EXDEV);
                }

                if context.directory_mode == DirectoryMode::MustBeDirectory
                    && !current_namespace_node.entry.node.is_dir()
                {
                    return error!(ENOTDIR);
                }
                basenames = &basenames[1..];
                continue;
            }
            if basenames.len() == 1
                || !current_namespace_node.entry.node.ops().has_lookup_pipelined()
            {
                current_namespace_node = current_namespace_node.with_new_entry(
                    current_namespace_node.entry.component_lookup(
                        current_task,
                        &current_namespace_node.mount,
                        basename,
                    )?,
                );

                current_namespace_node = current_namespace_node.resolve(current_task, context)?;

                if context.resolve_flags.contains(ResolveFlags::NO_XDEV)
                    && current_namespace_node.mount != self.mount
                {
                    return error!(EXDEV);
                }

                if context.directory_mode == DirectoryMode::MustBeDirectory
                    && !current_namespace_node.entry.node.is_dir()
                {
                    return error!(ENOTDIR);
                }

                basenames = &basenames[1..];
                continue;
            }

            let pipelined_basenames = if let Some(pos) =
                basenames.iter().position(|&name| name.is_empty() || name == "." || name == "..")
            {
                &basenames[..pos]
            } else {
                basenames
            };
            let precomputed_entries = current_namespace_node.entry.get_children_pipelined(
                current_task,
                &current_namespace_node.mount,
                pipelined_basenames,
            );
            for entry in precomputed_entries {
                basenames = &basenames[1..];
                let child = current_namespace_node.with_new_entry(entry?);

                current_namespace_node = child.clone().resolve(current_task, context)?;

                if context.resolve_flags.contains(ResolveFlags::NO_XDEV)
                    && current_namespace_node.mount != self.mount
                {
                    return error!(EXDEV);
                }

                if context.directory_mode == DirectoryMode::MustBeDirectory
                    && !current_namespace_node.entry.node.is_dir()
                {
                    return error!(ENOTDIR);
                }

                if current_namespace_node != child {
                    break;
                }
            }
        }

        Ok(current_namespace_node)
    }

    /// Traverse up a child-to-parent link in the namespace.
    ///
    /// This traversal matches the child-to-parent link in the underlying
    /// FsNode except at mountpoints, where the link switches from one
    /// filesystem to another.
    pub fn parent(&self) -> Option<NamespaceNode> {
        let mountpoint_or_self = self.escape_mount();
        let parent = mountpoint_or_self.entry.parent()?;
        Some(mountpoint_or_self.with_new_entry(parent))
    }

    /// Returns the parent, but does not escape mounts i.e. returns None if this node
    /// is the root of a mount.
    pub fn parent_within_mount(&self) -> Option<DirEntryHandle> {
        if let Ok(_) = self.mount_if_root() {
            return None;
        }
        self.entry.parent()
    }

    /// Whether this namespace node is a descendant of the given node.
    ///
    /// Walks up the namespace node tree looking for ancestor. If ancestor is
    /// found, returns true. Otherwise, returns false.
    pub fn is_descendant_of(&self, ancestor: &NamespaceNode) -> bool {
        let ancestor = ancestor.escape_mount();
        let mut current = self.escape_mount();
        while current != ancestor {
            if let Some(parent) = current.parent() {
                current = parent.escape_mount();
            } else {
                return false;
            }
        }
        true
    }

    /// If this node is a mountpoint, returns the root of the submount.
    ///
    /// This only traverses one level of mount, even if the submount's root
    /// is itself a mountpoint for another mount.
    fn enter_one_mount(&self) -> Option<NamespaceNode> {
        if let Some(mount) = self.mount.deref() {
            if let Some(submount) =
                mount.relations.get_submount(&RcuReadScope::new(), &PtrKey::from(&self.entry))
            {
                return Some(submount.root());
            }
        }
        None
    }

    /// If this is a mount point, return the root of the mount. Otherwise return self.
    ///
    /// This function traverses multiple mounts if there are mounts layered on top of each other.
    /// It handles its own synchronization by acquiring the mounts sequence lock for reading.
    fn enter_mount(&self) -> NamespaceNode {
        // While the child is a mountpoint, replace child with the mount's root.
        let kernel = self.entry.node.fs().kernel.upgrade().expect("kernel");
        kernel.mounts_lock.read_seq(|| {
            let mut inner = self.clone();
            while let Some(inner_root) = inner.enter_one_mount() {
                inner = inner_root;
            }
            inner
        })
    }

    /// If this is a mount point, return the root of the mount. Otherwise return self.
    ///
    /// This function traverses multiple mounts if there are mounts layered on top of each other.
    /// It requires the caller to hold the mounts write lock (proved by `MountsWriteToken`),
    /// unlike `enter_mount` which uses a sequence lock for reading.
    fn enter_mount_locked(&self, _mounts_guard: &MountsWriteToken) -> NamespaceNode {
        // While the child is a mountpoint, replace child with the mount's root.
        let mut inner = self.clone();
        while let Some(inner_root) = inner.enter_one_mount() {
            inner = inner_root;
        }
        inner
    }

    /// If this is the root of a mount, return the mount point. Otherwise return self.
    ///
    /// This is not exactly the same as parent(). If parent() is called on a root, it will escape
    /// the mount, but then return the parent of the mount point instead of the mount point.
    fn escape_mount(&self) -> NamespaceNode {
        let kernel = self.entry.node.fs().kernel.upgrade().expect("kernel");
        kernel.mounts_lock.read_seq(|| {
            let mut mountpoint_or_self = self.clone();
            while let Some(mountpoint) = mountpoint_or_self.mountpoint() {
                mountpoint_or_self = mountpoint;
            }
            mountpoint_or_self
        })
    }

    /// If this node is the root of a mount, return it. Otherwise EINVAL.
    pub fn mount_if_root(&self) -> Result<&MountHandle, Errno> {
        if let Some(mount) = self.mount.deref() {
            if Arc::ptr_eq(&self.entry, &mount.root) {
                return Ok(mount);
            }
        }
        error!(EINVAL)
    }

    /// Returns the mountpoint at this location in the namespace.
    ///
    /// If this node is mounted in another node, this function returns the node
    /// at which this node is mounted. Otherwise, returns None.
    fn mountpoint(&self) -> Option<NamespaceNode> {
        self.mount_if_root().ok()?.mountpoint()
    }

    /// The path from the filesystem root to this node.
    pub fn path(&self, fs: &FsContext) -> FsString {
        self.path_from_root(Some(&fs.root())).into_path()
    }

    /// The path from the root of the namespace to this node.
    pub fn path_escaping_chroot(&self) -> FsString {
        self.path_from_root(None).into_path()
    }

    /// Returns the path to this node, accounting for a custom root.
    /// A task may have a custom root set by `chroot`.
    pub fn path_from_root(&self, root: Option<&NamespaceNode>) -> PathWithReachability {
        if self.mount.is_none() {
            return self.unrooted_path();
        }

        let mut path = PathBuilder::new();
        let mut current = self.escape_mount();
        if let Some(root) = root {
            let scope = RcuReadScope::new();
            // The current node is expected to intersect with the custom root as we travel up the tree.
            let root = root.escape_mount();
            while current != root {
                if let Some(parent) = current.parent() {
                    path.prepend_element(current.entry.local_name(&scope));
                    current = parent.escape_mount();
                } else {
                    // This node hasn't intersected with the custom root and has reached the namespace root.
                    let mut absolute_path = path.build_absolute();
                    if self.entry.is_dead() {
                        absolute_path.extend_from_slice(b" (deleted)");
                    }

                    return PathWithReachability::Unreachable(absolute_path);
                }
            }
        } else {
            // No custom root, so travel up the tree to the namespace root.
            let scope = RcuReadScope::new();
            while let Some(parent) = current.parent() {
                path.prepend_element(current.entry.local_name(&scope));
                current = parent.escape_mount();
            }
        }

        let mut absolute_path = path.build_absolute();
        if self.entry.is_dead() {
            absolute_path.extend_from_slice(b" (deleted)");
        }

        PathWithReachability::Reachable(absolute_path)
    }

    fn unrooted_path(&self) -> PathWithReachability {
        let scope = RcuReadScope::new();
        let mode = self.entry.node.info().mode;
        let local_name = self.entry.local_name(&scope);
        let path = if !local_name.is_empty() {
            format!("anon_inode:{}", local_name)
        } else if mode.is_sock() {
            format!("socket:[{}]", self.entry.node.ino)
        } else if mode.is_fifo() {
            format!("pipe:[{}]", self.entry.node.ino)
        } else {
            format!("file:[{}]", self.entry.node.ino)
        };
        PathWithReachability::Reachable(path.into())
    }

    pub fn mount(&self, what: WhatToMount, flags: MountpointFlags) -> Result<(), Errno> {
        let target = self.enter_mount();

        // A target node may not have a mount point if it's an anonymous node. Modern Linux (6.15+)
        // returns ENOENT in this case, rather than the more intuitive EINVAL.
        if target.mount.is_none() {
            return error!(ENOENT);
        }

        let source_is_dir = match &what {
            WhatToMount::Fs(fs) => fs.root().node.is_dir(),
            WhatToMount::Bind(node) => node.entry.node.is_dir(),
        };

        // The source and target need to both be identical types. In other words, if attempting to
        // mount a source file onto a target directory (or vice versa), we should err.
        if source_is_dir != target.entry.node.is_dir() {
            return error!(ENOTDIR);
        }

        let kernel = self.entry.node.fs().kernel.upgrade().expect("can't mount without a kernel");
        let mounts_guard = kernel.mounts_lock();
        let mountpoint = self.enter_mount_locked(&mounts_guard);

        let mount = mountpoint.mount.as_ref().ok_or_else(|| errno!(ENOENT))?;
        let writeable_mount = mounts_guard.retain(mount);
        let writeable_entry = mounts_guard.retain(&mountpoint.entry);
        writeable_mount.create_submount(
            &mounts_guard,
            &writeable_entry,
            WhatSubmount::New(what, flags),
        )
    }

    /// If this is the root of a filesystem, unmount. Otherwise return EINVAL.
    pub fn unmount(&self, flags: UnmountFlags) -> Result<(), Errno> {
        let kernel = self.entry.node.fs().kernel.upgrade().expect("can't mount without a kernel");
        let mounts_guard = kernel.mounts_lock();

        let mountpoint = self.enter_mount_locked(&mounts_guard);
        mounts_guard.retain(&mountpoint.entry);
        mountpoint.mount.as_ref().map(|mount| mounts_guard.retain(mount));
        let mount = mounts_guard.retain(mountpoint.mount_if_root()?);
        mount.unmount(&mounts_guard, flags)
    }

    pub fn rename(
        current_task: &CurrentTask,
        old_parent: &NamespaceNode,
        old_name: &FsStr,
        new_parent: &NamespaceNode,
        new_name: &FsStr,
        flags: RenameFlags,
    ) -> Result<(), Errno> {
        DirEntry::rename(
            current_task,
            &old_parent.entry,
            &old_parent.mount,
            old_name,
            &new_parent.entry,
            &new_parent.mount,
            new_name,
            flags,
        )
    }

    fn with_new_entry(&self, entry: DirEntryHandle) -> NamespaceNode {
        Self { mount: self.mount.clone(), entry }
    }

    fn mount_hash_key(&self) -> PtrKey<DirEntry> {
        PtrKey::from(&self.entry)
    }

    pub fn apply_suid_and_sgid(&self, creds: &mut Credentials) {
        // From <https://man7.org/linux/man-pages/man2/execve.2.html>:
        //
        //   The aforementioned transformations of the effective IDs are not
        //   performed ... if ... the underlying filesystem is mounted nosuid
        //   (the MS_NOSUID flag for mount(2)).
        if self.mount.flags().contains(MountFlags::NOSUID) {
            return;
        }
        self.entry.node.info().apply_suid_and_sgid(creds)
    }

    pub fn update_atime(&self) {
        // Do not update the atime of this node if it is mounted with the NOATIME flag.
        if !self.mount.flags().contains(MountFlags::NOATIME) {
            self.entry.node.update_info(|info| {
                let now = utc::utc_now();
                info.time_access = now;
                info.pending_time_access_update = true;
            });
        }
    }

    pub fn readlink(&self, current_task: &CurrentTask) -> Result<SymlinkTarget, Errno> {
        self.update_atime();
        self.entry.node.readlink(current_task)
    }

    pub fn notify(&self, event_mask: InotifyMask) {
        if self.mount.is_some() {
            self.entry.notify(event_mask);
        }
    }

    /// Check whether the node can be accessed in the current context with the specified access
    /// flags (read, write, or exec). Accounts for capabilities and whether the current user is the
    /// owner or is in the file's group.
    pub fn check_access(
        &self,
        current_task: &CurrentTask,
        access_check: AccessCheck,
    ) -> Result<(), Errno> {
        self.entry.node.check_access(
            current_task,
            &self.mount,
            access_check.permission_flags(),
            access_check.reason(),
            self,
        )
    }

    /// Checks if O_NOATIME is allowed,
    pub fn check_o_noatime_allowed(&self, current_task: &CurrentTask) -> Result<(), Errno> {
        self.entry.node.check_o_noatime_allowed(current_task)
    }

    pub fn truncate(&self, current_task: &CurrentTask, length: u64) -> Result<(), Errno> {
        self.entry.node.truncate(current_task, &self.mount, length)?;
        self.entry.notify_ignoring_excl_unlink(InotifyMask::MODIFY);
        Ok(())
    }
}

impl fmt::Debug for NamespaceNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NamespaceNode")
            .field("path", &self.path_escaping_chroot())
            .field("mount", &self.mount)
            .field("entry", &self.entry)
            .finish()
    }
}

// Eq/Hash impls intended for the MOUNT_POINTS hash
impl PartialEq for NamespaceNode {
    fn eq(&self, other: &Self) -> bool {
        self.mount.as_ref().map(Arc::as_ptr).eq(&other.mount.as_ref().map(Arc::as_ptr))
            && Arc::ptr_eq(&self.entry, &other.entry)
    }
}
impl Eq for NamespaceNode {}
impl Hash for NamespaceNode {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.mount.as_ref().map(Arc::as_ptr).hash(state);
        Arc::as_ptr(&self.entry).hash(state);
    }
}

/// A namespace node that keeps the underly mount busy.
#[derive(Debug, Clone)]
pub struct ActiveNamespaceNode {
    /// The underlying namespace node.
    name: NamespaceNode,

    /// Adds a reference to the mount client marker to prevent the mount from
    /// being removed while the NamespaceNode is active. Is None iff mount is
    /// None.
    _marker: Option<MountClientMarker>,
}

impl ActiveNamespaceNode {
    pub fn new(name: NamespaceNode) -> Self {
        let marker = name.mount.as_ref().map(|mount| mount.active_client_counter.clone());
        Self { name, _marker: marker }
    }

    pub fn to_passive(&self) -> NamespaceNode {
        self.deref().clone()
    }
}

impl Deref for ActiveNamespaceNode {
    type Target = NamespaceNode;

    fn deref(&self) -> &Self::Target {
        &self.name
    }
}

impl PartialEq for ActiveNamespaceNode {
    fn eq(&self, other: &Self) -> bool {
        self.deref().eq(other.deref())
    }
}
impl Eq for ActiveNamespaceNode {}
impl Hash for ActiveNamespaceNode {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.deref().hash(state)
    }
}

/// Mapping of a file into memory, holding a reference to the [`FileHandle`] and an optional
/// write guard on the underlying node.
#[derive(Debug, Clone)]
#[must_use]
pub struct FileMapping {
    file: FileHandle,
    mode: Option<FileWriteGuardMode>,
}

impl FileMapping {
    /// Creates a new [`FileMapping`], acquiring the specified write guard mode on the node if any.
    pub fn new(file: FileHandle, mode: Option<FileWriteGuardMode>) -> Result<Arc<Self>, Errno> {
        if let Some(mode) = mode {
            file.name.entry.node.write_guard_state.lock().acquire(mode)?;
        }
        Ok(Arc::new(Self { file, mode }))
    }

    /// File backing this mapping.
    pub fn file(&self) -> &FileHandle {
        &self.file
    }

    /// Active namespace node associated with the underlying file.
    pub fn name(&self) -> &ActiveNamespaceNode {
        &self.file.name
    }

    /// Filesystem node associated with the underlying file.
    pub fn node(&self) -> &FsNodeHandle {
        self.file.node()
    }
}

impl PartialEq for FileMapping {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.file, &other.file) && self.mode == other.mode
    }
}

impl Eq for FileMapping {}

impl Drop for FileMapping {
    fn drop(&mut self) {
        if let Some(mode) = self.mode {
            self.file.name.entry.node.write_guard_state.lock().release(mode);
        }
    }
}

/// Tracks all mounts, keyed by mount point.
pub struct Mounts {
    mounts: RcuHashMap<WeakKey<DirEntry>, Vec<WeakKey<Mount>>>,
}

impl Mounts {
    pub fn new() -> Self {
        Mounts { mounts: RcuHashMap::default() }
    }

    /// Registers the mount in the global mounts map.
    fn register_mount(&self, dir_entry: &DirEntryHandle, mount: MountHandle) -> Submount {
        let mut mounts = self.mounts.lock();
        let key = WeakKey::from(dir_entry);
        let mut vec = mounts.get(&key).unwrap_or_else(|| {
            dir_entry.set_has_mounts(true);
            Vec::new()
        });
        vec.push(WeakKey::from(&mount));
        mounts.insert(key, vec);
        Submount { dir: dir_entry.clone(), mount }
    }

    /// Unregisters the mount. This is called by `Submount::drop`.
    fn unregister_mount(&self, dir_entry: &DirEntryHandle, mount: &MountHandle) {
        let mut mounts = self.mounts.lock();
        let key = WeakKey::from(dir_entry);
        if let Some(mut vec) = mounts.get(&key) {
            let index = vec.iter().position(|e| e == &WeakKey::from(mount)).unwrap();
            if vec.len() == 1 {
                mounts.remove(&key);
                dir_entry.set_has_mounts(false);
            } else {
                vec.swap_remove(index);
                mounts.insert(key, vec);
            }
        }
    }

    /// Unmounts all mounts associated with `dir_entry`. This is called when `dir_entry` is
    /// unlinked (which would normally result in EBUSY, but not if it isn't mounted in the local
    /// namespace).
    pub fn unmount(&self, dir_entry: &DirEntry) {
        let mounts = self.mounts.lock().remove(&PtrKey::from(dir_entry));
        if let Some(mounts) = mounts {
            let upgraded: Vec<_> = mounts.iter().filter_map(|m| m.0.upgrade()).collect();
            if let Some(kernel) = upgraded.first().map(|m| m.kernel()) {
                let mounts_guard = kernel.mounts_lock();
                let mounts = scopeguard::guard(upgraded, |mounts| mounts_guard.defer_drop(mounts));
                for mount in &*mounts {
                    // Ignore errors.
                    let _ = mount.unmount(&mounts_guard, UnmountFlags::DETACH);
                }
            }
        }
    }

    /// Drain mounts. For each drained mount, force a FileSystem unmount.
    // TODO(https://fxbug.dev/295073633): Graceful shutdown should try to first unmount the mounts
    // and only force a FileSystem unmount on failure.
    pub fn clear(&self) {
        for (_dir_entry, mounts) in self.mounts.lock().drain() {
            for mount in mounts {
                if let Some(mount) = mount.0.upgrade() {
                    mount.fs.force_unmount_ops();
                }
            }
        }
    }

    /// Returns the set of unique mounted filesystems, including any underlying filesystems
    /// wrapped by stacked filesystems such as OverlayFs.
    fn unique_filesystems(&self) -> HashSet<ArcKey<FileSystem>> {
        let mut filesystems = HashSet::new();
        let mut queue = Vec::new();
        let scope = RcuReadScope::new();
        for (_dir_entry, m_list) in self.mounts.iter(&scope) {
            for m in m_list {
                if let Some(mount) = m.0.upgrade() {
                    if filesystems.insert(ArcKey(mount.fs.clone())) {
                        queue.push(mount.fs.clone());
                    }
                }
            }
        }
        while let Some(fs) = queue.pop() {
            for sub_fs in fs.sub_filesystems() {
                if filesystems.insert(ArcKey(sub_fs.clone())) {
                    queue.push(sub_fs);
                }
            }
        }
        filesystems
    }

    pub fn sync_all(&self, current_task: &CurrentTask) -> Result<(), Errno> {
        for fs in self.unique_filesystems() {
            if let Err(e) = fs.sync(current_task) {
                log_warn!("sync failed for filesystem {:?}: {:?}", fs.name(), e);
            }
        }
        Ok(())
    }

    pub fn drop_caches(&self) {
        let filesystems = self.unique_filesystems();
        // First purge all directory entry caches so that stacked filesystems (like OverlayFs)
        // drop references to underlying DirEntries/FsNodes before backend caches are cleared.
        for fs in &filesystems {
            fs.purge_dcache();
        }
        for fs in &filesystems {
            fs.drop_backend_caches();
        }
    }
}

impl Drop for Mount {
    fn drop(&mut self) {
        // Updating the RCU object without lock is acceptable because this Mount is not available
        // anymore by anything.
        let kernel = self.kernel();
        let peer_group = self.relations.peer_group.upgrade();
        let upstream = self.relations.upstream.cloned();
        self.relations.peer_group.update(None);
        self.relations.upstream.update(None);
        if peer_group.is_some() || upstream.is_some() {
            let mount_ptr = PtrKey::from(self as &Mount);
            register_delayed_call(move |_current_task| {
                let guard = kernel.mounts_lock();
                Mount::unregister_from_peer_group_and_upstream(
                    &guard, peer_group, upstream, mount_ptr,
                );
            });
        }
    }
}

impl fmt::Debug for Mount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let scope = RcuReadScope::new();
        f.debug_struct("Mount")
            .field("id", &(self as *const Mount))
            .field("root", &self.root)
            .field("mountpoint", &self.relations.mountpoint(&scope))
            .field(
                "submounts",
                &self
                    .relations
                    .iter_submounts(&scope)
                    .filter_map(|(_, m)| m.upgrade())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// A RAII object that unregisters a mount when dropped.
#[derive(Debug)]
struct Submount {
    dir: DirEntryHandle,
    mount: MountHandle,
}

impl Drop for Submount {
    fn drop(&mut self) {
        self.mount.kernel().mounts.unregister_mount(&self.dir, &self.mount)
    }
}

#[cfg(test)]
mod test {
    use crate::fs::tmpfs::TmpFs;
    use crate::testing::spawn_kernel_and_run;
    use crate::vfs::namespace::DeviceId;
    use crate::vfs::{
        CallbackSymlinkNode, DirectoryMode, FsNodeInfo, LookupContext, MountInfo, Namespace,
        NamespaceNode, RenameFlags, SymlinkMode, SymlinkTarget, UnlinkKind, WhatToMount,
    };
    use starnix_uapi::mount_flags::MountpointFlags;
    use starnix_uapi::{errno, mode};
    use std::sync::Arc;

    #[::fuchsia::test]
    async fn test_namespace() {
        spawn_kernel_and_run(async |current_task| {
            let kernel = current_task.kernel();
            let root_fs = TmpFs::new_fs(&kernel);
            let root_node = Arc::clone(root_fs.root());
            let _dev_node =
                root_node.create_dir(&current_task, "dev".into()).expect("failed to mkdir dev");
            let dev_fs = TmpFs::new_fs(&kernel);
            let dev_root_node = Arc::clone(dev_fs.root());
            let _dev_pts_node =
                dev_root_node.create_dir(&current_task, "pts".into()).expect("failed to mkdir pts");

            let ns = Namespace::new(root_fs);
            let mut context = LookupContext::default();
            let dev = ns
                .root()
                .lookup_child(&current_task, &mut context, "dev".into())
                .expect("failed to lookup dev");
            dev.mount(WhatToMount::Fs(dev_fs), MountpointFlags::empty())
                .expect("failed to mount dev root node");

            let mut context = LookupContext::default();
            let dev = ns
                .root()
                .lookup_child(&current_task, &mut context, "dev".into())
                .expect("failed to lookup dev");
            let mut context = LookupContext::default();
            let pts = dev
                .lookup_child(&current_task, &mut context, "pts".into())
                .expect("failed to lookup pts");
            let pts_parent =
                pts.parent().ok_or_else(|| errno!(ENOENT)).expect("failed to get parent of pts");
            assert!(Arc::ptr_eq(&pts_parent.entry, &dev.entry));

            let dev_parent =
                dev.parent().ok_or_else(|| errno!(ENOENT)).expect("failed to get parent of dev");
            assert!(Arc::ptr_eq(&dev_parent.entry, &ns.root().entry));
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_mount_does_not_upgrade() {
        spawn_kernel_and_run(async |current_task| {
            let kernel = current_task.kernel();
            let root_fs = TmpFs::new_fs(&kernel);
            let root_node = Arc::clone(root_fs.root());
            let _dev_node =
                root_node.create_dir(&current_task, "dev".into()).expect("failed to mkdir dev");
            let dev_fs = TmpFs::new_fs(&kernel);
            let dev_root_node = Arc::clone(dev_fs.root());
            let _dev_pts_node =
                dev_root_node.create_dir(&current_task, "pts".into()).expect("failed to mkdir pts");

            let ns = Namespace::new(root_fs);
            let mut context = LookupContext::default();
            let dev = ns
                .root()
                .lookup_child(&current_task, &mut context, "dev".into())
                .expect("failed to lookup dev");
            dev.mount(WhatToMount::Fs(dev_fs), MountpointFlags::empty())
                .expect("failed to mount dev root node");
            let mut context = LookupContext::default();
            let new_dev = ns
                .root()
                .lookup_child(&current_task, &mut context, "dev".into())
                .expect("failed to lookup dev again");
            assert!(!Arc::ptr_eq(&dev.entry, &new_dev.entry));
            assert_ne!(&dev, &new_dev);

            let mut context = LookupContext::default();
            let _new_pts = new_dev
                .lookup_child(&current_task, &mut context, "pts".into())
                .expect("failed to lookup pts");
            let mut context = LookupContext::default();
            assert!(dev.lookup_child(&current_task, &mut context, "pts".into()).is_err());
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_path() {
        spawn_kernel_and_run(async |current_task| {
            let kernel = current_task.kernel();
            let root_fs = TmpFs::new_fs(&kernel);
            let root_node = Arc::clone(root_fs.root());
            let _dev_node =
                root_node.create_dir(&current_task, "dev".into()).expect("failed to mkdir dev");
            let dev_fs = TmpFs::new_fs(&kernel);
            let dev_root_node = Arc::clone(dev_fs.root());
            let _dev_pts_node =
                dev_root_node.create_dir(&current_task, "pts".into()).expect("failed to mkdir pts");

            let ns = Namespace::new(root_fs);
            let mut context = LookupContext::default();
            let dev = ns
                .root()
                .lookup_child(&current_task, &mut context, "dev".into())
                .expect("failed to lookup dev");
            dev.mount(WhatToMount::Fs(dev_fs), MountpointFlags::empty())
                .expect("failed to mount dev root node");

            let mut context = LookupContext::default();
            let dev = ns
                .root()
                .lookup_child(&current_task, &mut context, "dev".into())
                .expect("failed to lookup dev");
            let mut context = LookupContext::default();
            let pts = dev
                .lookup_child(&current_task, &mut context, "pts".into())
                .expect("failed to lookup pts");

            assert_eq!("/", ns.root().path_escaping_chroot());
            assert_eq!("/dev", dev.path_escaping_chroot());
            assert_eq!("/dev/pts", pts.path_escaping_chroot());
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_shadowing() {
        spawn_kernel_and_run(async |current_task| {
            let kernel = current_task.kernel();
            let root_fs = TmpFs::new_fs(&kernel);
            let ns = Namespace::new(root_fs.clone());
            let _foo_node = root_fs.root().create_dir(&current_task, "foo".into()).unwrap();
            let mut context = LookupContext::default();
            let foo_dir =
                ns.root().lookup_child(&current_task, &mut context, "foo".into()).unwrap();

            let foofs1 = TmpFs::new_fs(&kernel);
            foo_dir.mount(WhatToMount::Fs(foofs1.clone()), MountpointFlags::empty()).unwrap();
            let mut context = LookupContext::default();
            assert!(Arc::ptr_eq(
                &ns.root().lookup_child(&current_task, &mut context, "foo".into()).unwrap().entry,
                foofs1.root()
            ));
            let foo_dir =
                ns.root().lookup_child(&current_task, &mut context, "foo".into()).unwrap();

            let ns_clone = ns.clone_namespace(&kernel.mounts_lock());

            let foofs2 = TmpFs::new_fs(&kernel);
            foo_dir.mount(WhatToMount::Fs(foofs2.clone()), MountpointFlags::empty()).unwrap();
            let mut context = LookupContext::default();
            assert!(Arc::ptr_eq(
                &ns.root().lookup_child(&current_task, &mut context, "foo".into()).unwrap().entry,
                foofs2.root()
            ));

            assert!(Arc::ptr_eq(
                &ns_clone
                    .root()
                    .lookup_child(&current_task, &mut LookupContext::default(), "foo".into())
                    .unwrap()
                    .entry,
                foofs1.root()
            ));
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_unlink_mounted_directory() {
        spawn_kernel_and_run(async |current_task| {
            let kernel = current_task.kernel();
            let root_fs = TmpFs::new_fs(&kernel);
            let ns1 = Namespace::new(root_fs.clone());
            let ns2 = Namespace::new(root_fs.clone());
            let _foo_node = root_fs.root().create_dir(&current_task, "foo".into()).unwrap();
            let mut context = LookupContext::default();
            let foo_dir =
                ns1.root().lookup_child(&current_task, &mut context, "foo".into()).unwrap();

            let foofs = TmpFs::new_fs(&kernel);
            foo_dir.mount(WhatToMount::Fs(foofs), MountpointFlags::empty()).unwrap();

            // Trying to unlink from ns1 should fail.
            assert_eq!(
                ns1.root()
                    .unlink(
                        &current_task,
                        "foo".into(),
                        UnlinkKind::Directory,
                        DirectoryMode::AllowAny
                    )
                    .unwrap_err(),
                errno!(EBUSY),
            );

            // But unlinking from ns2 should succeed.
            ns2.root()
                .unlink(&current_task, "foo".into(), UnlinkKind::Directory, DirectoryMode::AllowAny)
                .expect("unlink failed");

            // And it should no longer show up in ns1.
            assert_eq!(
                ns1.root()
                    .unlink(
                        &current_task,
                        "foo".into(),
                        UnlinkKind::Directory,
                        DirectoryMode::AllowAny
                    )
                    .unwrap_err(),
                errno!(ENOENT),
            );
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_rename_mounted_directory() {
        spawn_kernel_and_run(async |current_task| {
            let kernel = current_task.kernel();
            let root_fs = TmpFs::new_fs(&kernel);
            let ns1 = Namespace::new(root_fs.clone());
            let ns2 = Namespace::new(root_fs.clone());
            let _foo_node = root_fs.root().create_dir(&current_task, "foo".into()).unwrap();
            let _bar_node = root_fs.root().create_dir(&current_task, "bar".into()).unwrap();
            let _baz_node = root_fs.root().create_dir(&current_task, "baz".into()).unwrap();
            let mut context = LookupContext::default();
            let foo_dir =
                ns1.root().lookup_child(&current_task, &mut context, "foo".into()).unwrap();

            let foofs = TmpFs::new_fs(&kernel);
            foo_dir.mount(WhatToMount::Fs(foofs), MountpointFlags::empty()).unwrap();

            // Trying to rename over foo from ns1 should fail.
            let root = ns1.root();
            assert_eq!(
                NamespaceNode::rename(
                    &current_task,
                    &root,
                    "bar".into(),
                    &root,
                    "foo".into(),
                    RenameFlags::empty()
                )
                .unwrap_err(),
                errno!(EBUSY),
            );
            // Likewise the other way.
            assert_eq!(
                NamespaceNode::rename(
                    &current_task,
                    &root,
                    "foo".into(),
                    &root,
                    "bar".into(),
                    RenameFlags::empty()
                )
                .unwrap_err(),
                errno!(EBUSY),
            );

            // But renaming from ns2 should succeed.
            let root = ns2.root();

            // First rename the directory with the mount.
            NamespaceNode::rename(
                &current_task,
                &root,
                "foo".into(),
                &root,
                "bar".into(),
                RenameFlags::empty(),
            )
            .expect("rename failed");

            // Renaming over a directory with a mount should also work.
            NamespaceNode::rename(
                &current_task,
                &root,
                "baz".into(),
                &root,
                "bar".into(),
                RenameFlags::empty(),
            )
            .expect("rename failed");

            // "foo" and "baz" should no longer show up in ns1.
            assert_eq!(
                ns1.root().lookup_child(&current_task, &mut context, "foo".into()).unwrap_err(),
                errno!(ENOENT)
            );
            assert_eq!(
                ns1.root().lookup_child(&current_task, &mut context, "baz".into()).unwrap_err(),
                errno!(ENOENT)
            );
        })
        .await;
    }

    /// Symlinks which need to be traversed across types (nodes and paths), as well as across
    /// owning directories, can be tricky to get right.
    #[::fuchsia::test]
    async fn test_lookup_with_symlink_chain() {
        spawn_kernel_and_run(async |current_task| {
            // Set up the root filesystem
            let kernel = current_task.kernel();
            let root_fs = TmpFs::new_fs(&kernel);
            let root_node = Arc::clone(root_fs.root());
            let _first_subdir_node = root_node
                .create_dir(&current_task, "first_subdir".into())
                .expect("failed to mkdir dev");
            let _second_subdir_node = root_node
                .create_dir(&current_task, "second_subdir".into())
                .expect("failed to mkdir dev");

            // Set up two subdirectories under the root filesystem
            let first_subdir_fs = TmpFs::new_fs(&kernel);
            let second_subdir_fs = TmpFs::new_fs(&kernel);

            let ns = Namespace::new(root_fs);
            let mut context = LookupContext::default();
            let first_subdir = ns
                .root()
                .lookup_child(&current_task, &mut context, "first_subdir".into())
                .expect("failed to lookup first_subdir");
            first_subdir
                .mount(WhatToMount::Fs(first_subdir_fs), MountpointFlags::empty())
                .expect("failed to mount first_subdir fs node");
            let second_subdir = ns
                .root()
                .lookup_child(&current_task, &mut context, "second_subdir".into())
                .expect("failed to lookup second_subdir");
            second_subdir
                .mount(WhatToMount::Fs(second_subdir_fs), MountpointFlags::empty())
                .expect("failed to mount second_subdir fs node");

            // Create the symlink structure. To trigger potential symlink traversal bugs, we're going
            // for the following directory structure:
            // / (root)
            //     + first_subdir/
            //         - real_file
            //         - path_symlink (-> real_file)
            //     + second_subdir/
            //         - node_symlink (-> path_symlink)
            let real_file_node = first_subdir
                .create_node(&current_task, "real_file".into(), mode!(IFREG, 0o777), DeviceId::NONE)
                .expect("failed to create real_file");
            first_subdir
                .create_symlink(&current_task, "path_symlink".into(), "real_file".into())
                .expect("failed to create path_symlink");

            let mut no_follow_lookup_context = LookupContext::new(SymlinkMode::NoFollow);
            let path_symlink_node = first_subdir
                .lookup_child(&current_task, &mut no_follow_lookup_context, "path_symlink".into())
                .expect("Failed to lookup path_symlink");

            // The second symlink needs to be of type SymlinkTarget::Node in order to trip the sensitive
            // code path. There's no easy method for creating this type of symlink target, so we'll need
            // to construct a node from scratch and insert it into the directory manually.
            let node_symlink_node = second_subdir.entry.node.fs().create_node_and_allocate_node_id(
                CallbackSymlinkNode::new(move || {
                    let node = path_symlink_node.clone();
                    Ok(SymlinkTarget::Node(node))
                }),
                FsNodeInfo::new(mode!(IFLNK, 0o777), current_task.current_fscred()),
            );
            second_subdir
                .entry
                .create_entry(
                    &current_task,
                    &MountInfo::detached(),
                    "node_symlink".into(),
                    move |_dir, _mount, _name| Ok(node_symlink_node),
                )
                .expect("failed to create node_symlink entry");

            // Finally, exercise the lookup under test.
            let mut follow_lookup_context = LookupContext::new(SymlinkMode::Follow);
            let node_symlink_resolution = second_subdir
                .lookup_child(&current_task, &mut follow_lookup_context, "node_symlink".into())
                .expect("lookup with symlink chain failed");

            // The lookup resolution should have correctly followed the symlinks to the real_file node.
            assert!(node_symlink_resolution.entry.node.ino == real_file_node.entry.node.ino);
        })
        .await;
    }
}
