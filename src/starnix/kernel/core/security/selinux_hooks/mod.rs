// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// TODO(https://github.com/rust-lang/rust/issues/39371): remove
#![allow(non_upper_case_globals)]

pub(super) mod audit;
pub(super) mod binder;
pub(super) mod bpf;
pub(super) mod file;
pub(super) mod fs_node;
pub(super) mod netlink_socket;
pub(super) mod perf_event;
pub(super) mod selinuxfs;
pub(super) mod socket;
pub(super) mod superblock;
pub(super) mod task;
pub(super) mod testing;

use super::PermissionFlags;
use crate::task::{CurrentTask, TaskPersistentInfo};
use crate::vfs::{DirEntry, FileHandle, FileObject, FileSystem, FileSystemOps, FsNode};
use audit::{Auditable, audit_decision};
use fuchsia_rcu::{RcuBox, RcuDroppable, RcuReadGuard};
use indexmap::IndexSet;
use selinux::permission_check::PermissionCheck;
use selinux::policy::{FsUseType, XpermsKind};
use selinux::{
    ClassPermission, CommonFilePermission, CommonFsNodePermission, DirPermission, FdPermission,
    FileClass, FileSystemLabel, FileSystemLabelingScheme, FileSystemMountOptions, ForClass,
    FsNodeClass, InitialSid, KernelPermission, PolicyCap, ProcessPermission, SecurityId,
    SecurityServer, TaskAttrs,
};
use smallvec;
use starnix_logging::{BugRef, CATEGORY_STARNIX_SECURITY, bug_ref, track_stub};
use starnix_sync::{
    LockDepMutex, SeLinuxPeerSidLock, SeLinuxPendingEntriesLock, SeLinuxPendingFileSystemsLock,
    SeLinuxUpdateLock,
};
use starnix_uapi::arc_key::WeakKey;
use starnix_uapi::errors::Errno;
use starnix_uapi::file_mode::FileMode;
use starnix_uapi::{errno, error};
use std::cell::Ref;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

/// Rust cannot infer the permission type from an empty slice, so define an explicitly-typed empty
/// permissions slice to use.
const NO_PERMISSIONS: &[KernelPermission] = &[];

/// Iterable set of permissions returned by `permissions_from_flags()`.
type PermissionFlagsVec = smallvec::SmallVec<[KernelPermission; 3]>;

/// Returns the set of `Permissions` on `class`, corresponding to the specified `flags`.
fn permissions_from_flags(flags: PermissionFlags, class: FsNodeClass) -> PermissionFlagsVec {
    let mut result = PermissionFlagsVec::new();

    if flags.contains(PermissionFlags::READ) {
        result.push(CommonFsNodePermission::Read.for_class(class));
    }
    if flags.contains(PermissionFlags::WRITE) {
        // SELinux uses the `APPEND` bit to distinguish which of the "append" or the more general
        // "write" permission to check for.
        if flags.contains(PermissionFlags::APPEND) {
            result.push(CommonFsNodePermission::Append.for_class(class));
        } else {
            result.push(CommonFsNodePermission::Write.for_class(class));
        }
    }

    if let FsNodeClass::File(class) = class {
        if flags.contains(PermissionFlags::EXEC) {
            if class == FileClass::Dir {
                result.push(DirPermission::Search.into());
            } else {
                result.push(CommonFilePermission::Execute.for_class(class));
            }
        }
    }
    result
}

fn is_internal_operation(current_task: &CurrentTask) -> bool {
    current_task_state(current_task).internal_operation
}

/// Checks that `current_task` has permission to "use" the specified `file`, and the specified
/// `permissions` to the underlying [`crate::vfs::FsNode`].
fn has_file_permissions(
    permission_check: &PermissionCheck<'_>,
    current_task: &CurrentTask,
    subject_sid: SecurityId,
    file: &FileObject,
    permissions: &[impl ForClass<FsNodeClass>],
    audit_context: Auditable<'_>,
) -> Result<(), Errno> {
    if is_internal_operation(current_task) {
        return Ok(());
    };
    // Validate that the `subject` has the "fd { use }" permission to the `file`.
    // If the file and task security domains are identical then `fd { use }` is implicitly granted.
    let file_sid = file.security_state.state.sid;
    if subject_sid != file_sid {
        let node = file.node().as_ref().as_ref();
        let audit_context = [audit_context, file.into(), node.into()];
        check_permission(
            permission_check,
            current_task,
            subject_sid,
            file_sid,
            FdPermission::Use,
            (&audit_context).into(),
        )?;
    }

    // Validate that the `subject` has the desired `permissions`, if any, to the underlying node.
    if !permissions.is_empty() {
        let audit_context = [audit_context, file.into()];
        has_fs_node_permissions(
            permission_check,
            current_task,
            subject_sid,
            file.node(),
            permissions,
            (&audit_context).into(),
        )?;
    }

    Ok(())
}

fn has_file_ioctl_permission(
    permission_check: &PermissionCheck<'_>,
    current_task: &CurrentTask,
    subject_sid: SecurityId,
    file: &FileObject,
    ioctl: u16,
    audit_context: Auditable<'_>,
) -> Result<(), Errno> {
    // Validate that the `subject` has the "fd { use }" permission to the `file`.
    has_file_permissions(
        permission_check,
        current_task,
        subject_sid,
        file,
        NO_PERMISSIONS,
        audit_context,
    )?;

    // Validate that the `subject` has the `ioctl` permission on the underlying node,
    // as well as the specified ioctl extended permission.
    let fs_node = file.node().as_ref().as_ref();
    if fs_node.is_private() {
        return Ok(());
    }
    let FsNodeSidAndClass { sid: target_sid, class: target_class } =
        fs_node_effective_sid_and_class(fs_node);

    let audit_context =
        &[audit_context, file.into(), fs_node.into(), Auditable::IoctlCommand(ioctl)];

    // Check the `ioctl` permission and extended permission on the underlying node.
    check_permission_and_xperms(
        permission_check,
        current_task,
        subject_sid,
        target_sid,
        CommonFsNodePermission::Ioctl.for_class(target_class),
        XpermsKind::Ioctl,
        ioctl,
        audit_context.into(),
    )
}

fn check_permission_and_xperms(
    permission_check: &PermissionCheck<'_>,
    current_task: &CurrentTask,
    subject_sid: SecurityId,
    target_sid: SecurityId,
    permission: KernelPermission,
    xperms_kind: XpermsKind,
    xperm: u16,
    audit_context: Auditable<'_>,
) -> Result<(), Errno> {
    if is_internal_operation(current_task) {
        return Ok(());
    }
    let result = permission_check.has_extended_permission(
        xperms_kind,
        subject_sid,
        target_sid,
        permission.clone(),
        xperm,
    );

    if result.audit {
        if !result.permit() {
            current_task
                .kernel()
                .security_state
                .state
                .as_ref()
                .unwrap()
                .access_denial_count
                .fetch_add(1, Ordering::Release);
        }

        audit_decision(
            current_task,
            permission_check,
            result.clone(),
            subject_sid,
            target_sid,
            permission.into(),
            audit_context.into(),
        );
    }

    result.permit().then_some(()).ok_or_else(|| errno!(EACCES))
}

/// Checks that `current_task` has the specified `permissions` to the `node`, without auditing.
fn has_fs_node_permissions_dontaudit(
    permission_check: &PermissionCheck<'_>,
    _current_task: &CurrentTask,
    subject_sid: SecurityId,
    fs_node: &FsNode,
    permissions: &[impl ForClass<FsNodeClass>],
) -> Result<(), Errno> {
    fuchsia_trace::duration!(
        CATEGORY_STARNIX_SECURITY,
        "security.selinux.has_fs_node_permissions_dontaudit"
    );

    if fs_node.is_private() {
        return Ok(());
    }

    let target = fs_node_effective_sid_and_class(fs_node);
    for permission in permissions {
        if !permission_check
            .has_permission(subject_sid, target.sid, permission.for_class(target.class))
            .permit()
        {
            return error!(EACCES);
        }
    }

    Ok(())
}

/// Checks that `current_task` has the specified `permissions` to the `node`.
fn has_fs_node_permissions(
    permission_check: &PermissionCheck<'_>,
    current_task: &CurrentTask,
    subject_sid: SecurityId,
    fs_node: &FsNode,
    permissions: &[impl ForClass<FsNodeClass>],
    audit_context: Auditable<'_>,
) -> Result<(), Errno> {
    fuchsia_trace::duration!(CATEGORY_STARNIX_SECURITY, "security.selinux.has_fs_node_permissions");

    if fs_node.is_private() {
        return Ok(());
    }

    let target = fs_node_effective_sid_and_class(fs_node);

    let fs = fs_node.fs();
    let audit_context = [audit_context, fs_node.into(), fs.as_ref().into()];
    for permission in permissions {
        check_permission(
            permission_check,
            current_task,
            subject_sid,
            target.sid,
            permission.for_class(target.class),
            (&audit_context).into(),
        )?;
    }

    Ok(())
}

fn file_class_from_file_mode(mode: FileMode) -> Result<FileClass, Errno> {
    let file_type = mode.bits() & starnix_uapi::S_IFMT;
    match file_type {
        starnix_uapi::S_IFLNK => Ok(FileClass::LnkFile),
        starnix_uapi::S_IFDIR => Ok(FileClass::Dir),
        starnix_uapi::S_IFREG => Ok(FileClass::File),
        starnix_uapi::S_IFCHR => Ok(FileClass::ChrFile),
        starnix_uapi::S_IFBLK => Ok(FileClass::BlkFile),
        starnix_uapi::S_IFIFO => Ok(FileClass::FifoFile),
        starnix_uapi::S_IFSOCK => Ok(FileClass::SockFile),
        0 => {
            track_stub!(TODO("https://fxbug.dev/378864191"), "File with zero IFMT?");
            Ok(FileClass::File)
        }
        _ => error!(EINVAL, format!("mode: {:?}", mode)),
    }
}

#[macro_export]
macro_rules! TODO_DENY {
    ($bug_url:literal, $message:literal) => {{
        use starnix_logging::bug_ref;
        bug_ref!($bug_url)
    }};
}

/// Returns the `SecurityId` and `FsNodeClass` that should be used for SELinux access control checks
/// against `fs_node`.
fn fs_node_effective_sid_and_class(fs_node: &FsNode) -> FsNodeSidAndClass {
    let label_class = fs_node.security_state.0.read();
    if matches!(label_class.label, FsNodeLabel::Uninitialized) {
        // We should never reach here, but for now enforce it in debug builds.
        if cfg!(any(test, debug_assertions)) {
            panic!(
                "Unlabeled FsNode@{} of class {:?} in {} (label {:?})",
                fs_node.ino,
                file_class_from_file_mode(fs_node.info().mode),
                fs_node.fs().name(),
                fs_node.fs().security_state.state.label(),
            );
        } else {
            track_stub!(TODO("https://fxbug.dev/381210513"), "SID requested for unlabeled FsNode");
        }
    }
    FsNodeSidAndClass { sid: label_class.sid(), class: label_class.class() }
}

/// Checks whether `source_sid` is allowed the specified `permission` on `target_sid`.
fn check_permission<P: ClassPermission + Into<KernelPermission> + Clone + 'static>(
    permission_check: &PermissionCheck<'_>,
    current_task: &CurrentTask,
    source_sid: SecurityId,
    target_sid: SecurityId,
    permission: P,
    audit_context: Auditable<'_>,
) -> Result<(), Errno> {
    fuchsia_trace::duration!(CATEGORY_STARNIX_SECURITY, "security.selinux.check_permission");

    if is_internal_operation(current_task) {
        return Ok(());
    }
    let result = permission_check.has_permission(source_sid, target_sid, permission.clone());

    if result.audit {
        if !result.permit() {
            current_task
                .kernel()
                .security_state
                .state
                .as_ref()
                .unwrap()
                .access_denial_count
                .fetch_add(1, Ordering::Release);
        }

        audit_decision(
            current_task,
            permission_check,
            result.clone(),
            source_sid,
            target_sid,
            permission.into(),
            audit_context,
        );
    };

    result.permit().then_some(()).ok_or_else(|| errno!(EACCES))
}

/// Checks that `subject_sid` has the specified process `permission` on `self`.
fn check_self_permission<P: ClassPermission + Into<KernelPermission> + Clone + 'static>(
    permission_check: &PermissionCheck<'_>,
    current_task: &CurrentTask,
    subject_sid: SecurityId,
    permission: P,
    audit_context: Auditable<'_>,
) -> Result<(), Errno> {
    check_permission(
        permission_check,
        current_task,
        subject_sid,
        subject_sid,
        permission,
        audit_context,
    )
}

async fn create_inspect_values(
    security_server: Arc<SecurityServer>,
) -> Result<fuchsia_inspect::Inspector, anyhow::Error> {
    let inspector = fuchsia_inspect::Inspector::default();

    let policy_bytes = if let Some(policy_data) = security_server.get_binary_policy() {
        policy_data.len().try_into()?
    } else {
        0
    };
    inspector.root().record_uint("policy_bytes", policy_bytes);

    Ok(inspector)
}

/// Returns the security state structure for the kernel.
pub(super) fn kernel_init_security(
    options: String,
    exceptions: Vec<String>,
    inspect_node: &fuchsia_inspect::Node,
) -> KernelState {
    let server = SecurityServer::new(options, exceptions);
    let inspect_node = inspect_node.create_child("selinux");

    let server_for_inspect = server.clone();
    inspect_node.record_lazy_values("server", move || {
        Box::pin(create_inspect_values(server_for_inspect.clone()))
    });

    KernelState {
        server,
        pending_file_systems: LockDepMutex::default(),
        selinuxfs_null: OnceLock::default(),
        access_denial_count: AtomicU64::new(0u64),
        has_policy: false.into(),
        _inspect_node: inspect_node,
    }
}

/// The global SELinux security structures, held by the `Kernel`.
pub(super) struct KernelState {
    // Owning reference to the SELinux `SecurityServer`.
    pub(super) server: Arc<SecurityServer>,

    /// Set of [`create::vfs::FileSystem`]s that have been constructed, and must be labeled as soon
    /// as a policy is loaded into the `server`. Insertion order is retained, via use of `IndexSet`,
    /// to ensure that filesystems have labels initialized in creation order, which is important
    /// e.g. when initializing "overlayfs" node labels, based on the labels of the underlying nodes.
    pub(super) pending_file_systems:
        LockDepMutex<IndexSet<WeakKey<FileSystem>>, SeLinuxPendingFileSystemsLock>,

    /// True when the `server` has a policy loaded.
    pub(super) has_policy: AtomicBool,

    /// Stashed reference to "/sys/fs/selinux/null" used for replacing inaccessible file descriptors
    /// with a null file.
    pub(super) selinuxfs_null: OnceLock<FileHandle>,

    /// Counts the number of times that an AVC denial is audit-logged.
    pub(super) access_denial_count: AtomicU64,

    /// Inspect node through which SELinux status is exposed.
    pub(super) _inspect_node: fuchsia_inspect::Node,
}

impl KernelState {
    pub(super) fn access_denial_count(&self) -> u64 {
        self.access_denial_count.load(Ordering::Acquire)
    }

    pub(super) fn has_policy(&self) -> bool {
        self.has_policy.load(Ordering::Acquire)
    }
}

// Security state for a PerfEventFileState instance.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct PerfEventState {
    sid: SecurityId,
}

pub(in crate::security) fn current_task_state(current_task: &CurrentTask) -> Ref<'_, TaskAttrs> {
    Ref::map(current_task.current_creds(), |creds| &creds.security_state)
}

/// Returns the SID of a task. Panics if the task is using overridden credentials.
pub(in crate::security) fn task_consistent_attrs(current_task: &CurrentTask) -> Ref<'_, TaskAttrs> {
    assert!(!current_task.has_overridden_creds());
    current_task_state(current_task)
}

/// Builds a `PermissionCheck` from the local cache of `current_task` and the given `security_server`.
pub(in crate::security) fn build_permission_check<'a>(
    current_task: &'a CurrentTask,
    security_server: &'a SecurityServer,
) -> PermissionCheck<'a> {
    security_server.as_permission_check(&current_task.security_state.state.local_cache)
}

/// Security state for a [`crate::vfs::FileObject`] instance.
#[derive(Debug)]
pub(super) struct FileObjectState {
    sid: SecurityId,
    open_state: OnceLock<FileObjectOpenState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FileObjectOpenState {
    /// The SID of the FsNode when it was opened.
    node_sid: SecurityId,
    /// The policy version (change count) when it was opened.
    policy_seqno: selinux::PolicySeqNo,
}

/// Security state for a [`crate::vfs::FileSystem`] instance. This holds the security fields
/// parsed from the mount options and the selected labeling scheme.
#[derive(Debug)]
pub(super) struct FileSystemState {
    // Fields used prior to policy-load, to hold mount options, etc.
    mount_options: FileSystemMountOptions,
    pending_entries: LockDepMutex<IndexSet<WeakKey<DirEntry>>, SeLinuxPendingEntriesLock>,

    // Set once the initial policy has been loaded, taking into account `mount_options`.
    label: OnceLock<FileSystemLabel>,
}

impl FileSystemState {
    fn new(mount_options: FileSystemMountOptions, _ops: &dyn FileSystemOps) -> Self {
        let pending_entries = IndexSet::new().into();
        let label = OnceLock::new();

        Self { mount_options, pending_entries, label }
    }

    /// Returns the resolved `FileSystemLabel`, or `None` if no policy has yet been loaded.
    pub fn label(&self) -> Option<&FileSystemLabel> {
        self.label.get()
    }

    /// Returns true if this file system supports dynamic re-labeling of file nodes.
    pub fn supports_relabel(&self) -> bool {
        let Some(label) = self.label() else {
            return false;
        };
        match label.scheme {
            FileSystemLabelingScheme::Mountpoint { .. } => false,
            FileSystemLabelingScheme::FsUse { .. } => true,
            FileSystemLabelingScheme::GenFsCon { supports_seclabel } => supports_seclabel,
        }
    }

    /// Returns true if this file system persists labels in extended attributes.
    pub fn supports_xattr(&self) -> bool {
        let Some(label) = self.label() else {
            return false;
        };
        match label.scheme {
            FileSystemLabelingScheme::Mountpoint { .. }
            | FileSystemLabelingScheme::GenFsCon { .. } => false,
            FileSystemLabelingScheme::FsUse { fs_use_type, .. } => fs_use_type == FsUseType::Xattr,
        }
    }
}

/// Holds security state associated with a [`crate::vfs::FsNode`].
#[derive(Debug)]
pub(super) struct FsNodeState {
    label: RcuBox<FsNodeLabelAndClass>,
    update_lock: LockDepMutex<(), SeLinuxUpdateLock>,
}

impl Default for FsNodeState {
    fn default() -> Self {
        Self {
            label: RcuBox::new(FsNodeLabelAndClass {
                label: FsNodeLabel::Uninitialized,
                class: None,
            }),
            update_lock: Default::default(),
        }
    }
}

impl FsNodeState {
    pub(super) fn read(&self) -> RcuReadGuard<FsNodeLabelAndClass> {
        self.label.read()
    }

    pub(super) fn update(&self, new_label: FsNodeLabel, new_class: FsNodeClass) {
        let _lock = self.update_lock.lock();
        let mut new_label_class = (*self.label.read()).clone();
        new_label_class.label = new_label;
        new_label_class.class = Some(new_class);
        self.label.update(new_label_class);
    }

    pub(super) fn update_label(&self, new_label: FsNodeLabel) {
        let _lock = self.update_lock.lock();
        let mut new_label_class = (*self.label.read()).clone();
        new_label_class.label = new_label;
        self.label.update(new_label_class);
    }

    pub(super) fn update_class(&self, new_class: FsNodeClass) {
        let _lock = self.update_lock.lock();
        let mut new_label_class = (*self.label.read()).clone();
        new_label_class.class = Some(new_class);
        self.label.update(new_label_class);
    }

    #[cfg(test)]
    pub(super) fn clear_label_for_test(&self) {
        let _lock = self.update_lock.lock();
        let class = self.label.read().class;
        self.label.update(FsNodeLabelAndClass { label: FsNodeLabel::Uninitialized, class });
    }
}

/// Describes the security label for a [`crate::vfs::FsNode`].
#[derive(Debug, Clone, RcuDroppable)]
pub(super) enum FsNodeLabel {
    Uninitialized,
    SecurityId { sid: SecurityId },
    // TODO(https://fxbug.dev/451613626): Consider replacing by a reference to a task-or-zombie.
    FromTask { task_state: TaskPersistentInfo },
}

impl FsNodeLabel {
    pub fn sid(&self) -> SecurityId {
        match self {
            FsNodeLabel::Uninitialized => InitialSid::Unlabeled.into(),
            FsNodeLabel::SecurityId { sid } => *sid,
            FsNodeLabel::FromTask { task_state } => {
                task_state.real_creds().security_state.current_sid
            }
        }
    }
}

#[derive(Debug, Clone, RcuDroppable)]
pub(super) struct FsNodeLabelAndClass {
    pub label: FsNodeLabel,
    pub class: Option<FsNodeClass>,
}

impl FsNodeLabelAndClass {
    pub fn class(&self) -> FsNodeClass {
        self.class.unwrap_or(FsNodeClass::File(FileClass::File))
    }

    pub fn sid(&self) -> SecurityId {
        self.label.sid()
    }
}

/// Holds the SID and class with which an `FsNode` is labeled, for use in permissions checks.
#[derive(Debug, PartialEq)]
pub(super) struct FsNodeSidAndClass {
    pub sid: SecurityId,
    pub class: FsNodeClass,
}

/// Security state for a [`crate::binderfs::BinderConnection`] instance. This holds the
/// [`starnix_uapi::selinux::SecurityId`] of the task as it was when it created the connection.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct BinderConnectionState {
    sid: SecurityId,
}

/// Security state for a [`crate::vfs::Socket`] instance. This holds the [`starnix_uapi::selinux::SecurityId`] of
/// the peer socket.
#[derive(Debug, Default)]
pub(super) struct SocketState {
    peer_sid: LockDepMutex<Option<SecurityId>, SeLinuxPeerSidLock>,
}

/// Security state for a bpf [`ebpf_api::maps::Map`] instance. This currently just holds the
/// SID that the [`crate::task::Task`] that created the file object had.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct BpfMapState {
    sid: SecurityId,
}

/// Security state for a bpf [`starnix_core::bpf::program::Program`]. instance. This currently just
/// holds the SID that the [`crate::task::Task`] that created the file object had.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct BpfProgState {
    sid: SecurityId,
}

/// Sets the cached security id associated with `fs_node` to `sid`. Storing the security id will
/// cause the security id to *not* be recomputed by the SELinux LSM when determining the effective
/// security id of this [`FsNode`].
pub(super) fn set_cached_sid(fs_node: &FsNode, sid: SecurityId) {
    fs_node.security_state.0.update_label(FsNodeLabel::SecurityId { sid });
}

/// Sets the Task associated with `fs_node` to `task`.
/// The effective security id of the [`FsNode`] will be that of the task, even if the security id
/// of the task changes.
fn fs_node_set_label_with_task(fs_node: &FsNode, task_persistent_info: &TaskPersistentInfo) {
    fs_node
        .security_state
        .0
        .update_label(FsNodeLabel::FromTask { task_state: task_persistent_info.clone() });
}

/// Ensures that the `fs_node`'s security state has an appropriate security class set.
/// As per the NSA report description, the security class is chosen based on the `FileMode`, unless
/// a security class more specific than "file" has already been set on the node.
fn fs_node_ensure_class(fs_node: &FsNode) -> Result<FsNodeClass, Errno> {
    let label_class = fs_node.security_state.0.read();
    if let Some(class) = label_class.class {
        return Ok(class);
    }

    let file_mode = fs_node.info().mode;
    let _lock = fs_node.security_state.0.update_lock.lock();
    let label_class = fs_node.security_state.0.read();
    if let Some(class) = label_class.class {
        return Ok(class);
    }
    let mut new_label_class = (*label_class).clone();
    let class = file_class_from_file_mode(file_mode)?.into();
    new_label_class.class = Some(class);
    fs_node.security_state.0.label.update(new_label_class);
    Ok(class)
}

#[cfg(test)]
/// Returns the SID with which the node is labeled, if any, for use by `FsNode` labeling tests.
pub(super) fn get_cached_sid(fs_node: &FsNode) -> Option<SecurityId> {
    let label_class = fs_node.security_state.0.read();
    if !matches!(label_class.label, FsNodeLabel::Uninitialized) {
        Some(label_class.sid())
    } else {
        None
    }
}

/// Returned by `policycap_support()` to indicate whether a policy is always-on, always-off,
/// the affected functionality is not-implemented, or it is fully supported/configurable.
#[derive(Debug)]
pub enum PolicyCapSupport {
    AlwaysOn(BugRef),
    AlwaysOff(BugRef),
    Configurable,
    NotImplemented,
}

/// Returns a `PolicyCapSupport` indicating the state of support, and the `BugRef` to report if
/// emitting a partial support warning.
fn policycap_support(policy_cap: PolicyCap) -> PolicyCapSupport {
    match policy_cap {
        PolicyCap::AlwaysCheckNetwork => {
            PolicyCapSupport::AlwaysOff(bug_ref!("https://fxbug.dev/452453565"))
        }
        PolicyCap::CgroupSeclabel => PolicyCapSupport::Configurable,
        PolicyCap::ExtendedSocketClass => PolicyCapSupport::Configurable,
        PolicyCap::FunctionfsSeclabel => PolicyCapSupport::Configurable,
        PolicyCap::GenfsSeclabelSymlinks => PolicyCapSupport::Configurable,
        PolicyCap::GenfsSeclabelWildcard => {
            PolicyCapSupport::AlwaysOff(bug_ref!("https://fxbug.dev/452453565"))
        }
        PolicyCap::IoctlSkipCloexec => PolicyCapSupport::Configurable,
        PolicyCap::MemfdClass => PolicyCapSupport::Configurable,
        PolicyCap::NetifWildcard => {
            PolicyCapSupport::AlwaysOff(bug_ref!("https://fxbug.dev/452453565"))
        }
        PolicyCap::NetlinkXperm => PolicyCapSupport::Configurable,
        PolicyCap::NetworkPeerControls => PolicyCapSupport::NotImplemented,
        PolicyCap::NnpNosuidTransition => PolicyCapSupport::Configurable,
        PolicyCap::OpenPerms => PolicyCapSupport::AlwaysOn(bug_ref!("https://fxbug.dev/452453565")),
        PolicyCap::UserspaceInitialContext => PolicyCapSupport::Configurable,
    }
}

/// Security state for a current task. This holds the task-local cache for permission check results.
#[derive(Default, Debug)]
pub struct CurrentTaskState {
    pub local_cache: selinux::permission_check::PerThreadCache,
}
