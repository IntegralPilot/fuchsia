// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// TODO(https://github.com/rust-lang/rust/issues/39371): remove
#![allow(non_upper_case_globals)]

use super::selinux_hooks::audit::Auditable;
use super::{
    BinderConnectionState, BpfMapState, BpfProgState, FileObjectState, FileSystemState,
    KernelState, PerfEventState, common_cap, selinux_hooks, yama,
};
use crate::mm::{Mapping, MappingOptions, ProtectionFlags};
use crate::perf::PerfEventFile;
use crate::security::selinux_hooks::current_task_state;
use crate::task::loader::ResolvedProgram;
use crate::task::{CurrentTask, Kernel, Task};
use crate::vfs::fs_args::MountParams;
use crate::vfs::socket::{
    Socket, SocketAddress, SocketDomain, SocketFile, SocketPeer, SocketProtocol,
    SocketShutdownFlags, SocketType,
};
use crate::vfs::{
    DirEntryHandle, DowncastedFile, FileHandle, FileObject, FileSystem, FileSystemHandle,
    FileSystemOps, FsNode, FsStr, FsString, Mount, NamespaceNode, ValueOrSize, XattrOp,
};
use ebpf::MapFlags;
use linux_uapi::{
    perf_event_attr, perf_type_id, perf_type_id_PERF_TYPE_BREAKPOINT,
    perf_type_id_PERF_TYPE_HARDWARE, perf_type_id_PERF_TYPE_HW_CACHE, perf_type_id_PERF_TYPE_RAW,
    perf_type_id_PERF_TYPE_SOFTWARE, perf_type_id_PERF_TYPE_TRACEPOINT,
};
use selinux::{FileSystemMountOptions, InitialSid, SecurityPermission, SecurityServer, TaskAttrs};
use starnix_logging::{CATEGORY_STARNIX_SECURITY, log_debug};
use starnix_uapi::arc_key::WeakKey;
use starnix_uapi::auth::{Credentials, PtraceAccessMode};
use starnix_uapi::device_id::DeviceId;
use starnix_uapi::errors::Errno;
use starnix_uapi::file_mode::{Access, FileMode};
use starnix_uapi::mount_flags::MountFlags;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::signals::Signal;
use starnix_uapi::syslog::SyslogAction;
use starnix_uapi::unmount_flags::UnmountFlags;
use starnix_uapi::user_address::UserAddress;
use starnix_uapi::{bpf_cmd, error, rlimit};
use std::ops::Range;
use std::sync::Arc;
use syncio::zxio_node_attr_has_t;
use zerocopy::FromBytes;

macro_rules! track_hook_duration {
    ($cname:literal) => {
        fuchsia_trace::duration!(CATEGORY_STARNIX_SECURITY, $cname);
    };
}

bitflags::bitflags! {
    /// The flags about which permissions should be checked when opening an FsNode. Used in the
    /// `fs_node_permission()` hook.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct PermissionFlags: u32 {
        const EXEC = 1 as u32;
        const WRITE = 2 as u32;
        const READ = 4 as u32;
        const APPEND = 8 as u32;

        // Internal flag used to indicate that the check is being made on behalf of userspace e.g.
        // via the `access()` syscall.
        const ACCESS = 16 as u32;
    }
}

impl PermissionFlags {
    pub fn as_access(&self) -> Access {
        let mut access = Access::empty();
        if self.contains(PermissionFlags::READ) {
            access |= Access::READ;
        }
        if self.contains(PermissionFlags::WRITE) {
            // `APPEND` only modifies the behaviour of `WRITE` if set, so it is sufficient to only
            // consider whether `WRITE` is set, to calculate the `Access` flags.
            access |= Access::WRITE;
        }
        if self.contains(PermissionFlags::EXEC) {
            access |= Access::EXEC;
        }
        access
    }
}

impl From<Access> for PermissionFlags {
    fn from(access: Access) -> Self {
        // Note that `Access` doesn't have an `append` bit.
        let mut permissions = PermissionFlags::empty();
        if access.contains(Access::READ) {
            permissions |= PermissionFlags::READ;
        }
        if access.contains(Access::WRITE) {
            permissions |= PermissionFlags::WRITE;
        }
        if access.contains(Access::EXEC) {
            permissions |= PermissionFlags::EXEC;
        }
        permissions
    }
}

impl From<ProtectionFlags> for PermissionFlags {
    fn from(protection_flags: ProtectionFlags) -> Self {
        let mut flags = PermissionFlags::empty();
        if protection_flags.contains(ProtectionFlags::READ) {
            flags |= PermissionFlags::READ;
        }
        if protection_flags.contains(ProtectionFlags::WRITE) {
            flags |= PermissionFlags::WRITE;
        }
        if protection_flags.contains(ProtectionFlags::EXEC) {
            flags |= PermissionFlags::EXEC;
        }
        flags
    }
}

impl From<OpenFlags> for PermissionFlags {
    fn from(flags: OpenFlags) -> Self {
        let mut permissions = PermissionFlags::empty();
        if flags.can_read() {
            permissions |= PermissionFlags::READ;
        }
        if flags.can_write() {
            permissions |= PermissionFlags::WRITE;
            if flags.contains(OpenFlags::APPEND) {
                permissions |= PermissionFlags::APPEND;
            }
        }
        permissions
    }
}

impl From<MapFlags> for PermissionFlags {
    fn from(bpf_flags: MapFlags) -> Self {
        if bpf_flags.contains(MapFlags::SyscallReadOnly) {
            PermissionFlags::READ
        } else if bpf_flags.contains(MapFlags::SyscallWriteOnly) {
            PermissionFlags::WRITE
        } else {
            PermissionFlags::READ | PermissionFlags::WRITE
        }
    }
}

/// The flags about the PerfEvent types.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PerfEventType {
    Hardware,
    Software,
    Tracepoint,
    Raw,
    HwCache,
    Breakpoint,
}

// TODO(https://github.com/rust-lang/rust/issues/39371): remove
#[allow(non_upper_case_globals)]
impl TryFrom<perf_type_id> for PerfEventType {
    type Error = Errno;

    fn try_from(type_id: perf_type_id) -> Result<Self, Errno> {
        match type_id {
            perf_type_id_PERF_TYPE_HARDWARE => Ok(Self::Hardware),
            perf_type_id_PERF_TYPE_SOFTWARE => Ok(Self::Software),
            perf_type_id_PERF_TYPE_TRACEPOINT => Ok(Self::Tracepoint),
            perf_type_id_PERF_TYPE_RAW => Ok(Self::Raw),
            perf_type_id_PERF_TYPE_HW_CACHE => Ok(Self::HwCache),
            perf_type_id_PERF_TYPE_BREAKPOINT => Ok(Self::Breakpoint),
            _ => {
                return error!(ENOTSUP);
            }
        }
    }
}

/// The target task type. Used in the `check_perf_event_open_access` LSM hook.
#[derive(PartialEq, Eq)]
pub enum TargetTaskType<'a> {
    /// Monitor all tasks/activities.
    AllTasks,
    /// Only monitor the current task.
    CurrentTask,
    /// Only monitor a specific task.
    Task(&'a Task),
}

/// Executes the `hook` closure if SELinux is enabled, and has a policy loaded.
/// If SELinux is not enabled, or has no policy loaded, then the `default` closure is executed,
/// and its result returned.
fn if_selinux_else<F, R, D>(task: &Task, hook: F, default: D) -> R
where
    F: FnOnce(&Arc<SecurityServer>) -> R,
    D: Fn() -> R,
{
    if let Some(state) = task.kernel().security_state.state.as_ref() {
        if state.has_policy() { hook(&state.server) } else { default() }
    } else {
        default()
    }
}

/// Specialization of `if_selinux_else(...)` for hooks which return a `Result<..., Errno>`, that
/// arranges to return a default `Ok(...)` result value if SELinux is not enabled, or not yet
/// configured with a policy.
fn if_selinux_else_default_ok<R, F>(task: &Task, hook: F) -> Result<R, Errno>
where
    F: FnOnce(&Arc<SecurityServer>) -> Result<R, Errno>,
    R: Default,
{
    if_selinux_else(task, hook, || Ok(R::default()))
}

/// Returns the security state structure for the kernel, based on the supplied "selinux" argument
/// contents.
pub fn kernel_init_security(
    enabled: bool,
    options: String,
    exceptions: Vec<String>,
    inspect_node: &fuchsia_inspect::Node,
) -> KernelState {
    track_hook_duration!("security.hooks.kernel_init_security");
    KernelState {
        state: enabled
            .then(|| selinux_hooks::kernel_init_security(options, exceptions, inspect_node)),
    }
}

/// Checks whether the given `current_task` can become the binder context manager.
/// Corresponds to the `binder_set_context_mgr` hook.
pub fn binder_set_context_mgr(current_task: &CurrentTask) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.binder_set_context_mgr");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::binder::binder_set_context_mgr(security_server, current_task)
    })
}

/// Checks whether the given `current_task` can perform a transaction to `target_task`.
/// Corresponds to the `binder_transaction` hook.
pub fn binder_transaction(
    current_task: &CurrentTask,
    target_task: &Task,
    connection_state: &BinderConnectionState,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.binder_transaction");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::binder::binder_transaction(
            security_server,
            &connection_state.state,
            current_task,
            target_task,
        )
    })
}

/// Checks whether the given `current_task` can transfer Binder objects to `target_task`.
/// Corresponds to the `binder_transfer_binder` hook.
pub fn binder_transfer_binder(current_task: &CurrentTask, target_task: &Task) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.binder_transfer_binder");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::binder::binder_transfer_binder(security_server, current_task, target_task)
    })
}

/// Checks whether the given `receiving_task` can receive `file` in a Binder transaction.
/// Corresponds to the `binder_transfer_file` hook.
pub fn binder_transfer_file(
    current_task: &CurrentTask,
    receiving_task: &Task,
    file: &FileObject,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.binder_transfer_file");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::binder::binder_transfer_file(
            security_server,
            current_task,
            receiving_task,
            file,
        )
    })
}

/// Returns the serialized Security Context associated with the specified state.
/// If the state's SID cannot be resolved then None is returned.
pub fn binder_get_context(
    current_task: &CurrentTask,
    connection_state: &BinderConnectionState,
) -> Option<Vec<u8>> {
    track_hook_duration!("security.hooks.binder_get_context");
    if_selinux_else(
        current_task,
        |security_server| {
            selinux_hooks::binder::binder_get_context(&security_server, &connection_state.state)
        },
        || None,
    )
}

/// Consumes the mount options from the supplied `MountParams` and returns the security mount
/// options for the given `MountParams`.
/// Corresponds to the `sb_eat_lsm_opts` hook.
pub fn sb_eat_lsm_opts(
    kernel: &Kernel,
    mount_params: &mut MountParams,
) -> Result<FileSystemMountOptions, Errno> {
    track_hook_duration!("security.hooks.sb_eat_lsm_opts");
    if kernel.security_state.state.is_some() {
        return selinux_hooks::superblock::sb_eat_lsm_opts(mount_params);
    }
    Ok(FileSystemMountOptions::default())
}

/// Returns security state to associate with a filesystem based on the supplied mount options.
/// This sits somewhere between `fs_context_parse_param()` and `sb_set_mnt_opts()` in function.
pub fn file_system_init_security(
    mount_options: &FileSystemMountOptions,
    ops: &dyn FileSystemOps,
) -> Result<FileSystemState, Errno> {
    track_hook_duration!("security.hooks.file_system_init_security");
    Ok(FileSystemState {
        state: selinux_hooks::superblock::file_system_init_security(mount_options, ops)?,
    })
}

/// Gives the hooks subsystem an opportunity to note that the new `file_system` needs labeling, if
/// SELinux is enabled, but no policy has yet been loaded.
// TODO: https://fxbug.dev/366405587 - Merge this logic into `file_system_resolve_security()` and
// remove this extra hook.
pub fn file_system_post_init_security(kernel: &Kernel, file_system: &FileSystemHandle) {
    track_hook_duration!("security.hooks.file_system_post_init_security");
    if let Some(state) = &kernel.security_state.state {
        if !state.has_policy() {
            // TODO: https://fxbug.dev/367585803 - Revise locking to guard against a policy load
            // sneaking in, in-between `has_policy()` and this `insert()`.
            log_debug!("Queuing {} FileSystem for labeling", file_system.name());
            state.pending_file_systems.lock().insert(WeakKey::from(&file_system));
        }
    }
}

/// Resolves the labeling scheme and arguments for the `file_system`, based on the loaded policy.
/// If no policy has yet been loaded then no work is done, and the `file_system` will instead be
/// labeled when a policy is first loaded.
/// If the `file_system` was already labeled then no further work is done.
pub fn file_system_resolve_security(
    current_task: &CurrentTask,
    file_system: &FileSystemHandle,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.file_system_resolve_security");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::superblock::file_system_resolve_security(
            security_server,
            current_task,
            file_system,
        )
    })
}

/// Used to return an extended attribute name and value to apply to a [`crate::vfs::FsNode`].
pub struct FsNodeSecurityXattr {
    pub name: &'static FsStr,
    pub value: FsString,
}

/// Checks whether the `current_task` is allowed to mmap `file` or memory using the given
/// [`ProtectionFlags`] and [`MappingOptions`].
/// Corresponds to the `mmap_file()` LSM hook.
pub fn mmap_file(
    current_task: &CurrentTask,
    file: Option<&FileHandle>,
    protection_flags: ProtectionFlags,
    options: MappingOptions,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.mmap_file");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::file::mmap_file(
            security_server,
            current_task,
            file,
            protection_flags,
            options,
        )
    })
}

/// Checks whether `current_task` is allowed to request setting the memory protection of
/// `mapping` to `prot`.
/// Corresponds to the `file_mprotect` LSM hook.
pub fn file_mprotect(
    current_task: &CurrentTask,
    range: &Range<UserAddress>,
    mapping: &Mapping,
    prot: ProtectionFlags,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.file_mprotect");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::file::file_mprotect(security_server, current_task, range, mapping, prot)
    })
}

/// Checks whether the `current_task` has the specified `permission_flags` to the `file`.
/// Corresponds to the `file_permission()` LSM hook.
pub fn file_permission(
    current_task: &CurrentTask,
    file: &FileObject,
    permission_flags: PermissionFlags,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.file_permission");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::file::file_permission(security_server, current_task, file, permission_flags)
    })
}

/// Checks whether the `current_task` is allowed to open `file`.
/// Corresponds to the `file_open()` LSM hook.
pub fn file_open(current_task: &CurrentTask, file: &FileObject) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.file_open");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::file::file_open(security_server, current_task, file)
    })
}

/// Called by the VFS to initialize the security state for an `FsNode` that is being linked at
/// `dir_entry`.
/// If the `FsNode` security state had already been initialized, or no policy is yet loaded, then
/// this is a no-op.
/// Corresponds to the `d_instantiate()` LSM hook.
pub fn fs_node_init_with_dentry(
    current_task: &CurrentTask,
    dir_entry: &DirEntryHandle,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.fs_node_init_with_dentry");
    // TODO: https://fxbug.dev/367585803 - Don't use `if_selinux_else()` here, because the `has_policy()`
    // check is racey, so doing non-trivial work in the "else" path is unsafe. Instead, call the SELinux
    // hook implementation, and let it label, or queue, the `FsNode` based on the `FileSystem` label
    // state, thereby ensuring safe ordering.
    if let Some(state) = &current_task.kernel().security_state.state {
        selinux_hooks::fs_node::fs_node_init_with_dentry(
            &state.server,
            current_task,
            dir_entry,
            /* read_xaddr = */
            true,
        )
    } else {
        Ok(())
    }
}

pub fn fs_node_init_with_dentry_no_xattr(
    current_task: &CurrentTask,
    dir_entry: &DirEntryHandle,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.fs_node_init_with_dentry_no_xattr");
    // TODO: https://fxbug.dev/367585803 - Don't use `if_selinux_else()` here, because the `has_policy()`
    // check is racey, so doing non-trivial work in the "else" path is unsafe. Instead, call the SELinux
    // hook implementation, and let it label, or queue, the `FsNode` based on the `FileSystem` label
    // state, thereby ensuring safe ordering.
    if let Some(state) = &current_task.kernel().security_state.state {
        // Sockets are currently implemented using `Anon` nodes, and may be kernel-private, in
        // which case delegate to the anonymous node initializer to apply a placeholder label.
        if dir_entry.node.is_private() {
            return selinux_hooks::fs_node::fs_node_init_anon(
                &state.server,
                current_task,
                &dir_entry.node,
                "",
            );
        }

        selinux_hooks::fs_node::fs_node_init_with_dentry(
            &state.server,
            current_task,
            dir_entry,
            /* read_xaddr = */
            false,
        )
    } else {
        Ok(())
    }
}

// Temporary work-around for lack of a `CurrentTask` during creation of `DirEntry`s for some initial
// file-systems.
// TODO: https://fxbug.dev/455771186 - Clean up with-DirEntry initialization and remove this.
pub fn fs_node_init_with_dentry_deferred(kernel: &Kernel, dir_entry: &DirEntryHandle) {
    track_hook_duration!("security.hooks.fs_node_init_with_dentry_no_xattr");
    if kernel.security_state.state.is_some() {
        selinux_hooks::fs_node::fs_node_init_with_dentry_deferred(dir_entry);
    }
}

/// Applies the given label to the given node without checking any permissions.
/// Used by file-system implementations to set the label for a node, for example when it has
/// prefetched the label in the xattr rather than letting it get fetched by
/// `fs_node_init_with_dentry` later. Calling this doesn't need to exclude the use of
/// `fs_node_init_with_dentry`, it will just turn that call into a fast no-op.
/// Corresponds to the `inode_notifysecctx` LSM hook.
pub fn fs_node_notify_security_context(
    current_task: &CurrentTask,
    fs_node: &FsNode,
    context: &FsStr,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.fs_node_notify_security_context");
    if_selinux_else(
        current_task,
        |security_server| {
            selinux_hooks::fs_node::fs_node_notify_security_context(
                security_server,
                fs_node,
                context,
            )
        },
        || error!(ENOTSUP),
    )
}

/// Called by file-system implementations when creating the `FsNode` for a new file, to determine the
/// correct label based on the `CurrentTask` and `parent` node, and the policy-defined transition
/// rules, and to initialize the `FsNode`'s security state accordingly.
/// If no policy has yet been loaded then this is a no-op; if the `FsNode` corresponds to an xattr-
/// labeled file then it will receive the file-system's "default" label once a policy is loaded.
/// Returns an extended attribute value to set on the newly-created file if the labeling scheme is
/// `fs_use_xattr`. For other labeling schemes (e.g. `fs_use_trans`, mountpoint-labeling) a label
/// is set on the `FsNode` security state, but no extended attribute is set nor returned.
/// The `name` with which the new node is being created allows name-conditional `type_transition`
/// rules to be applied when determining the label for the `new_node`.
/// Corresponds to the `inode_init_security()` LSM hook.
pub fn fs_node_init_on_create(
    current_task: &CurrentTask,
    new_node: &FsNode,
    parent: &FsNode,
    name: &FsStr,
) -> Result<Option<FsNodeSecurityXattr>, Errno> {
    track_hook_duration!("security.hooks.fs_node_init_on_create");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::fs_node::fs_node_init_on_create(
            security_server,
            current_task,
            new_node,
            Some(parent),
            name,
        )
    })
}

/// Called by specialist file-system implementations before creating a new `FsNode`, to obtain the
/// SID with which the code will be labeled, in advance.
///
/// The computed SID will be applied to the `fscreate_sid` field of the supplied `new_creds`, which
/// may then be used with `CurrentTask::override_creds()` to later create the new node.
///
/// Corresponds to the `dentry_create_files_as()` LSM hook.
pub fn dentry_create_files_as(
    current_task: &CurrentTask,
    parent: &FsNode,
    new_node_mode: FileMode,
    new_node_name: &FsStr,
    new_creds: &mut Credentials,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.dentry_create_files_as");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::fs_node::dentry_create_files_as(
            security_server,
            current_task,
            parent,
            new_node_mode,
            new_node_name,
            new_creds,
        )
    })
}

/// Called on creation of anonymous [`crate::vfs::FsNode`]s. APIs that create file-descriptors that
/// are not linked into any filesystem directory structure create anonymous nodes, labeled by this
/// hook rather than `fs_node_init_on_create()` above.
/// Corresponds to the `inode_init_security_anon()` LSM hook.
pub fn fs_node_init_anon(
    current_task: &CurrentTask,
    new_node: &FsNode,
    node_type: &str,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.fs_node_init_anon");
    if let Some(state) = current_task.kernel().security_state.state.as_ref() {
        selinux_hooks::fs_node::fs_node_init_anon(&state.server, current_task, new_node, node_type)
    } else {
        Ok(())
    }
}

/// Validate that `current_task` has permission to create a regular file in the `parent` directory,
/// with the specified file `mode`.
/// Corresponds to the `inode_create()` LSM hook.
pub fn check_fs_node_create_access(
    current_task: &CurrentTask,
    parent: &FsNode,
    mode: FileMode,
    name: &FsStr,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_fs_node_create_access");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::fs_node::check_fs_node_create_access(
            security_server,
            current_task,
            parent,
            mode,
            name,
        )
    })
}

/// Validate that `current_task` has permission to create a symlink to `old_path` in the `parent`
/// directory.
/// Corresponds to the `inode_symlink()` LSM hook.
pub fn check_fs_node_symlink_access(
    current_task: &CurrentTask,
    parent: &FsNode,
    name: &FsStr,
    old_path: &FsStr,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_fs_node_symlink_access");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::fs_node::check_fs_node_symlink_access(
            security_server,
            current_task,
            parent,
            name,
            old_path,
        )
    })
}

/// Validate that `current_task` has permission to create a new directory in the `parent` directory,
/// with the specified file `mode`.
/// Corresponds to the `inode_mkdir()` LSM hook.
pub fn check_fs_node_mkdir_access(
    current_task: &CurrentTask,
    parent: &FsNode,
    mode: FileMode,
    name: &FsStr,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_fs_node_mkdir_access");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::fs_node::check_fs_node_mkdir_access(
            security_server,
            current_task,
            parent,
            mode,
            name,
        )
    })
}

/// Validate that `current_task` has permission to create a new special file, socket or pipe, in the
/// `parent` directory, and with the specified file `mode` and `device_id`.
/// For consistency any calls to `mknod()` with a file `mode` specifying a regular file will be
/// validated by `check_fs_node_create_access()` rather than by this hook.
/// Corresponds to the `inode_mknod()` LSM hook.
pub fn check_fs_node_mknod_access(
    current_task: &CurrentTask,
    parent: &FsNode,
    mode: FileMode,
    name: &FsStr,
    device_id: DeviceId,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_fs_node_mknod_access");
    assert!(!mode.is_reg());

    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::fs_node::check_fs_node_mknod_access(
            security_server,
            current_task,
            parent,
            mode,
            name,
            device_id,
        )
    })
}

/// Validate that `current_task` has  the permission to create a new hard link to a file.
/// Corresponds to the `inode_link()` LSM hook.
pub fn check_fs_node_link_access(
    current_task: &CurrentTask,
    parent: &FsNode,
    child: &FsNode,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_fs_node_link_access");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::fs_node::check_fs_node_link_access(
            security_server,
            current_task,
            parent,
            child,
        )
    })
}

/// Validate that `current_task` has the permission to remove a hard link to a file.
/// Corresponds to the `inode_unlink()` LSM hook.
pub fn check_fs_node_unlink_access(
    current_task: &CurrentTask,
    parent: &FsNode,
    child: &FsNode,
    name: &FsStr,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_fs_node_unlink_access");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::fs_node::check_fs_node_unlink_access(
            security_server,
            current_task,
            parent,
            child,
            name,
        )
    })
}

/// Validate that `current_task` has the permission to remove a directory.
/// Corresponds to the `inode_rmdir()` LSM hook.
pub fn check_fs_node_rmdir_access(
    current_task: &CurrentTask,
    parent: &FsNode,
    child: &FsNode,
    name: &FsStr,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_fs_node_rmdir_access");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::fs_node::check_fs_node_rmdir_access(
            security_server,
            current_task,
            parent,
            child,
            name,
        )
    })
}

/// Checks whether the `current_task` can rename the file or directory `moving_node`.
/// If the rename replaces an existing node, `replaced_node` must contain a reference to the
/// existing node.
/// Corresponds to the `inode_rename()` LSM hook.
pub fn check_fs_node_rename_access(
    current_task: &CurrentTask,
    old_parent: &FsNode,
    moving_node: &FsNode,
    new_parent: &FsNode,
    replaced_node: Option<&FsNode>,
    old_basename: &FsStr,
    new_basename: &FsStr,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_fs_node_rename_access");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::fs_node::check_fs_node_rename_access(
            security_server,
            current_task,
            old_parent,
            moving_node,
            new_parent,
            replaced_node,
            old_basename,
            new_basename,
        )
    })
}

/// Checks whether the `current_task` can read the symbolic link in `fs_node`.
/// Corresponds to the `inode_readlink()` LSM hook.
pub fn check_fs_node_read_link_access(
    current_task: &CurrentTask,
    fs_node: &FsNode,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_fs_node_read_link_access");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::fs_node::check_fs_node_read_link_access(
            security_server,
            current_task,
            fs_node,
        )
    })
}

/// Checks whether the `current_task` can access an inode.
/// Corresponds to the `inode_permission()` LSM hook.
pub fn fs_node_permission(
    current_task: &CurrentTask,
    fs_node: &FsNode,
    permission_flags: PermissionFlags,
    audit_context: Auditable<'_>,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.fs_node_permission");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::fs_node::fs_node_permission(
            security_server,
            current_task,
            fs_node,
            permission_flags,
            audit_context,
        )
    })
}

/// Returns whether the `current_task` can receive `file` via a socket IPC.
/// Corresponds to the `file_receive()` LSM hook.
pub fn file_receive(current_task: &CurrentTask, file: &FileObject) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.file_receive");
    if_selinux_else_default_ok(current_task, |security_server| {
        let receiving_sid = current_task_state(current_task).current_sid;
        selinux_hooks::file::file_receive(security_server, current_task, receiving_sid, file)
    })
}

/// Returns the security state for a new file object created by `current_task`.
/// Corresponds to the `file_alloc_security()` LSM hook.
pub fn file_alloc_security(current_task: &CurrentTask) -> FileObjectState {
    track_hook_duration!("security.hooks.file_alloc_security");
    FileObjectState { state: selinux_hooks::file::file_alloc_security(current_task) }
}

/// Returns the security context to be assigned to a BinderConnection, based on the task that
/// creates it.
pub fn binder_connection_alloc(current_task: &CurrentTask) -> BinderConnectionState {
    track_hook_duration!("security.hooks.binder_connection_alloc");
    BinderConnectionState { state: selinux_hooks::binder::binder_connection_alloc(current_task) }
}

/// Returns the security context to be assigned to a BPM map object, based on the task that
/// creates it.
/// Corresponds to the `bpf_map_alloc_security()` LSM hook.
pub fn bpf_map_alloc(current_task: &CurrentTask) -> BpfMapState {
    track_hook_duration!("security.hooks.bpf_map_alloc");
    BpfMapState { state: selinux_hooks::bpf::bpf_map_alloc(current_task) }
}

/// Returns the security context to be assigned to a BPM program object, based on the task
/// that creates it.
/// Corresponds to the `bpf_prog_alloc_security()` LSM hook.
pub fn bpf_prog_alloc(current_task: &CurrentTask) -> BpfProgState {
    track_hook_duration!("security.hooks.bpf_prog_alloc");
    BpfProgState { state: selinux_hooks::bpf::bpf_prog_alloc(current_task) }
}

/// Returns whether `current_task` can issue an ioctl to `file`.
/// Corresponds to the `file_ioctl()` LSM hook.
pub fn check_file_ioctl_access(
    current_task: &CurrentTask,
    file: &FileObject,
    request: u32,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_file_ioctl_access");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::file::check_file_ioctl_access(security_server, current_task, file, request)
    })
}

/// Updates the supplied `new_creds` with the necessary FS and LSM credentials to correctly label
/// a new `FsNode` on copy-up, to match the existing `fs_node`.
///
/// - fs_node: The "lower" filesystem node that is to be copied-up.
/// - fs: The OverlayFS instance performing the copy-up operation.
// TODO: https://fxbug.dev/398696739 - Revise this API to accept the overlay FsNode for which
// copy-up is being performed, rather than separate "lower" `fs_node` and overlay `fs`.
///
/// Corresponds to the `security_inode_copy_up()` LSM hook.
pub fn fs_node_copy_up(
    current_task: &CurrentTask,
    fs_node: &FsNode,
    fs: &FileSystem,
    new_creds: &mut Credentials,
) {
    if_selinux_else(
        current_task,
        |_security_server| {
            selinux_hooks::fs_node::fs_node_copy_up(current_task, fs_node, fs, new_creds)
        },
        || {},
    )
}

/// This hook is called by the `flock` syscall. Returns whether `current_task` can perform
/// a lock operation on the given file.
///
/// See also `check_file_fcntl_access()` for `lock` permission checks performed after an
/// fcntl lock request.
///
/// Corresponds to the `file_lock()` LSM hook.
pub fn check_file_lock_access(current_task: &CurrentTask, file: &FileObject) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_file_lock_access");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::file::check_file_lock_access(security_server, current_task, file)
    })
}

/// Returns whether `current_task` has the permissions to execute this fcntl syscall.
/// Corresponds to the `file_fcntl()` LSM hook.
pub fn check_file_fcntl_access(
    current_task: &CurrentTask,
    file: &FileObject,
    fcntl_cmd: u32,
    fcntl_arg: u64,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_file_fcntl_access");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::file::check_file_fcntl_access(
            security_server,
            current_task,
            file,
            fcntl_cmd,
            fcntl_arg,
        )
    })
}

/// Checks whether `current_task` can set attributes on `node`.
/// Corresponds to the `inode_setattr()` LSM hook.
pub fn check_fs_node_setattr_access(
    current_task: &CurrentTask,
    node: &FsNode,
    attributes: &zxio_node_attr_has_t,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_fs_node_setattr_access");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::fs_node::check_fs_node_setattr_access(
            security_server,
            current_task,
            node,
            attributes,
        )
    })
}

/// Return the default initial `TaskAttrs` for kernel tasks.
/// Corresponds to the `task_alloc()` LSM hook, in the special case when current_task is null.
pub fn task_alloc_for_kernel() -> TaskAttrs {
    track_hook_duration!("security.hooks.task_alloc_for_kernel");
    TaskAttrs::for_kernel()
}

/// Labels an [`crate::vfs::FsNode`], by attaching a pseudo-label to the `fs_node`, which allows
/// indirect resolution of the effective label. Makes the security attributes of `fs_node` track the
/// `task`'s security attributes, even if the task's security attributes change. Called for the
/// /proc/<pid> `FsNode`s when they are created.
/// Corresponds to the `task_to_inode` LSM hook.
pub fn task_to_fs_node(current_task: &CurrentTask, task: &Task, fs_node: &FsNode) {
    track_hook_duration!("security.hooks.task_to_fs_node");
    // The fs_node_init_with_task hook doesn't require any policy-specific information. Only check
    // if SElinux is enabled before running it.
    if current_task.kernel().security_state.state.is_some() {
        selinux_hooks::task::fs_node_init_with_task(task, &fs_node);
    }
}

/// Returns `TaskAttrs` for a new `Task`, based on that of the provided `context`.
/// The effect is similar to combining the `task_alloc()` and `setprocattr()` LSM hooks, with the
/// difference that no access-checks are performed, and the "#<name>" syntax may be used to
/// have the `Task` assigned one of the "initial" Security Contexts, to allow components to be run
/// prior to a policy being loaded.
pub fn task_for_context(task: &Task, context: &FsStr) -> Result<TaskAttrs, Errno> {
    track_hook_duration!("security.hooks.task_for_context");
    Ok(if let Some(kernel_state) = task.kernel().security_state.state.as_ref() {
        selinux_hooks::task::task_alloc_from_context(&kernel_state.server, context)
    } else {
        Ok(TaskAttrs::for_selinux_disabled())
    }?)
}

/// Returns true if there exits a `dontaudit` rule for `current_task` access to `fs_node`, which
/// includes the `audit_access` pseudo-permission.
/// This appears to be handled via additional options & flags in other hooks, by LSM.
pub fn has_dontaudit_access(current_task: &CurrentTask, fs_node: &FsNode) -> bool {
    track_hook_duration!("security.hooks.has_dontaudit_access");
    if_selinux_else(
        current_task,
        |security_server| {
            selinux_hooks::fs_node::has_dontaudit_access(security_server, current_task, fs_node)
        },
        || false,
    )
}

/// Returns true if a task has the specified `capability`.
/// Corresponds to the `capable()` LSM hook invoked with a no-audit flag set.
pub fn is_task_capable_noaudit(
    current_task: &CurrentTask,
    capability: starnix_uapi::auth::Capabilities,
) -> bool {
    track_hook_duration!("security.hooks.is_task_capable_noaudit");
    return common_cap::capable(current_task, capability).is_ok()
        && if_selinux_else(
            current_task,
            |security_server| {
                selinux_hooks::task::is_task_capable_noaudit(
                    &selinux_hooks::build_permission_check(current_task, security_server),
                    &current_task,
                    capability,
                )
            },
            || true,
        );
}

/// Checks if a task has the specified `capability`.
/// Corresponds to the `capable()` LSM hook.
pub fn check_creds_capable(
    current_task: &CurrentTask,
    creds: &Credentials,
    capability: starnix_uapi::auth::Capabilities,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_creds_capable");
    common_cap::creds_capable(creds, capability)?;
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::task::check_creds_capable(
            &selinux_hooks::build_permission_check(current_task, security_server),
            &current_task,
            creds,
            capability,
        )
    })
}

/// Checks if a task has the specified `capability`.
/// Corresponds to the `capable()` LSM hook.
pub fn check_task_capable(
    current_task: &CurrentTask,
    capability: starnix_uapi::auth::Capabilities,
) -> Result<(), Errno> {
    check_creds_capable(current_task, &**current_task.current_creds(), capability)
}

/// Checks if creating a task is allowed.
/// Corresponds to the `task_alloc()` LSM hook, except this hook doesn't modify the task's label.
pub fn check_task_create_access(current_task: &CurrentTask) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_task_create_access");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::task::check_task_create_access(
            &selinux_hooks::build_permission_check(current_task, security_server),
            current_task,
        )
    })
}

/// Checks if creating a socket is allowed.
/// Corresponds to the `socket_create()` LSM hook.
pub fn check_socket_create_access(
    current_task: &CurrentTask,
    domain: SocketDomain,
    socket_type: SocketType,
    protocol: SocketProtocol,
    kernel_private: bool,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.socket_create");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::socket::check_socket_create_access(
            &security_server,
            current_task,
            domain,
            socket_type,
            protocol,
            kernel_private,
        )
    })
}

/// Sets the peer security context for each socket in the pair.
/// Corresponds to the `socket_socketpair()` LSM hook.
pub fn socket_socketpair(
    current_task: &CurrentTask,
    left: DowncastedFile<'_, SocketFile>,
    right: DowncastedFile<'_, SocketFile>,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.socket_socketpair");
    if_selinux_else_default_ok(current_task, |_| {
        selinux_hooks::socket::socket_socketpair(left, right)
    })
}

/// Computes and updates the socket security class associated with a new socket.
/// Corresponds to the `socket_post_create()` LSM hook.
pub fn socket_post_create(current_task: &CurrentTask, socket: &Socket) {
    track_hook_duration!("security.hooks.socket_post_create");
    if let Some(state) = &current_task.kernel().security_state.state {
        selinux_hooks::socket::socket_post_create(&state.server, socket);
    }
}

/// Checks if the `current_task` is allowed to perform a bind operation for this `socket`.
/// Corresponds to the `socket_bind()` LSM hook.
pub fn check_socket_bind_access(
    current_task: &CurrentTask,
    socket: &Socket,
    socket_address: &SocketAddress,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_socket_bind_access");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::socket::check_socket_bind_access(
            &security_server,
            current_task,
            socket,
            socket_address,
        )
    })
}

/// Checks if the `current_task` is allowed to initiate a connection with `socket`.
/// Corresponds to the `socket_connect()` LSM hook.
pub fn check_socket_connect_access(
    current_task: &CurrentTask,
    socket: DowncastedFile<'_, SocketFile>,
    socket_peer: &SocketPeer,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_socket_connect_access");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::socket::check_socket_connect_access(
            &security_server,
            current_task,
            socket,
            socket_peer,
        )
    })
}

/// Checks if the `current_task` is allowed to listen on `socket_node`.
/// Corresponds to the `socket_listen()` LSM hook.
pub fn check_socket_listen_access(
    current_task: &CurrentTask,
    socket: &Socket,
    backlog: i32,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_socket_listen_access");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::socket::check_socket_listen_access(
            &security_server,
            current_task,
            socket,
            backlog,
        )
    })
}

/// Checks if the `current_task` is allowed to accept connections on `listening_socket`. Sets
/// the security label and SID for the accepted socket to match those of the listening socket.
/// Corresponds to the `socket_accept()` LSM hook.
pub fn socket_accept(
    current_task: &CurrentTask,
    listening_socket: DowncastedFile<'_, SocketFile>,
    accepted_socket: DowncastedFile<'_, SocketFile>,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_socket_getname_access");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::socket::socket_accept(
            &security_server,
            current_task,
            listening_socket,
            accepted_socket,
        )
    })
}

/// Checks if the `current_task` is allowed to get socket options on `socket`.
/// Corresponds to the `socket_getsockopt()` LSM hook.
pub fn check_socket_getsockopt_access(
    current_task: &CurrentTask,
    socket: &Socket,
    level: u32,
    optname: u32,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_socket_getsockopt_access");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::socket::check_socket_getsockopt_access(
            &security_server,
            current_task,
            socket,
            level,
            optname,
        )
    })
}

/// Checks if the `current_task` is allowed to set socket options on `socket`.
/// Corresponds to the `socket_getsockopt()` LSM hook.
pub fn check_socket_setsockopt_access(
    current_task: &CurrentTask,
    socket: &Socket,
    level: u32,
    optname: u32,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_socket_setsockopt_access");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::socket::check_socket_setsockopt_access(
            &security_server,
            current_task,
            socket,
            level,
            optname,
        )
    })
}

/// Checks if the `current_task` is allowed to send a message on `socket`.
/// Corresponds to the `socket_sendmsg()` LSM hook.
pub fn check_socket_sendmsg_access(
    current_task: &CurrentTask,
    socket: &Socket,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_socket_sendmsg_access");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::socket::check_socket_sendmsg_access(&security_server, current_task, socket)
    })
}

/// Checks if the `current_task` is allowed to receive a message on `socket`.
/// Corresponds to the `socket_recvmsg()` LSM hook.
pub fn check_socket_recvmsg_access(
    current_task: &CurrentTask,
    socket: &Socket,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_socket_recvmsg_access");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::socket::check_socket_recvmsg_access(&security_server, current_task, socket)
    })
}

/// Checks if the `current_task` is allowed to get the local name of `socket`.
/// Corresponds to the `socket_getsockname()` LSM hook.
pub fn check_socket_getsockname_access(
    current_task: &CurrentTask,
    socket: &Socket,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_socket_getname_access");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::socket::check_socket_getname_access(&security_server, current_task, socket)
    })
}

/// Checks if the `current_task` is allowed to get the remote name of `socket`.
/// Corresponds to the `socket_getpeername()` LSM hook.
pub fn check_socket_getpeername_access(
    current_task: &CurrentTask,
    socket: &Socket,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_socket_getname_access");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::socket::check_socket_getname_access(&security_server, current_task, socket)
    })
}

/// Checks if the `current_task` is allowed to shutdown `socket`.
/// Corresponds to the `socket_shutdown()` LSM hook.
pub fn check_socket_shutdown_access(
    current_task: &CurrentTask,
    socket: &Socket,
    how: SocketShutdownFlags,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_socket_shutdown_access");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::socket::check_socket_shutdown_access(
            &security_server,
            current_task,
            socket,
            how,
        )
    })
}

/// Returns the Security Context with which the [`crate::vfs::Socket`]'s peer is labeled.
/// Corresponds to the `socket_getpeersec_stream()` LSM hook.
pub fn socket_getpeersec_stream(
    current_task: &CurrentTask,
    socket: &Socket,
) -> Result<Vec<u8>, Errno> {
    track_hook_duration!("security.hooks.socket_getpeersec_stream");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::socket::socket_getpeersec_stream(&security_server, current_task, socket)
    })
}

/// Returns the Security Context with which the [`crate::vfs::Socket`]'s is labeled, to return to
/// the recipient via `SCM_SECURITY` auxiliary data, if `SO_PASSSEC` is set.
/// Corresponds to the `socket_getpeersec_dgram()` LSM hook.
pub fn socket_getpeersec_dgram(current_task: &CurrentTask, socket: &Socket) -> Vec<u8> {
    track_hook_duration!("security.hooks.socket_getpeersec_dgram");
    if_selinux_else(
        current_task,
        |security_server| {
            selinux_hooks::socket::socket_getpeersec_dgram(&security_server, current_task, socket)
        },
        Vec::default,
    )
}

/// Checks if the Unix domain `sending_socket` is allowed to send a message to the
/// `receiving_socket`.
/// Corresponds to the `unix_may_send()` LSM hook.
pub fn unix_may_send(
    current_task: &CurrentTask,
    sending_socket: &Socket,
    receiving_socket: &Socket,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.unix_may_send");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::socket::unix_may_send(
            &security_server,
            current_task,
            sending_socket,
            receiving_socket,
        )
    })
}

/// Checks if the Unix domain `client_socket` is allowed to connect to `listening_socket`, and
/// initialises the peer information in the client and server sockets.
/// Corresponds to the `unix_stream_connect()` LSM hook.
pub fn unix_stream_connect(
    current_task: &CurrentTask,
    client_socket: &Socket,
    listening_socket: &Socket,
    server_socket: &Socket,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.unix_stream_connect");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::socket::unix_stream_connect(
            &security_server,
            current_task,
            client_socket,
            listening_socket,
            server_socket,
        )
    })
}

/// Checks if the `current_task` is allowed to send a message of `message_type` on the Netlink
/// `socket`.
/// Corresponds to the `netlink_send()` LSM hook.
pub fn check_netlink_send_access(
    current_task: &CurrentTask,
    socket: &Socket,
    message_type: u16,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_netlink_send_access");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::netlink_socket::check_netlink_send_access(
            &security_server,
            current_task,
            socket,
            message_type,
        )
    })
}

/// Checks if the `current_task` has permission to create a new TUN device.
/// Corresponds to the `tun_dev_create()` LSM hook.
pub fn check_tun_dev_create_access(current_task: &CurrentTask) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_tun_dev_create_access");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::socket::check_tun_dev_create_access(&security_server, current_task)
    })
}

/// Updates credentials based on the executable file (e.g., SUID/SGID, capabilities).
///
/// Corresponds to the `bprm_creds_from_file` LSM hook.
pub fn bprm_creds_from_file(
    current_task: &CurrentTask,
    resolved_program: &mut ResolvedProgram,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.bprm_creds_from_file");

    let (no_new_privs, is_ptraced) = {
        let state = current_task.read();
        (state.no_new_privs(), state.is_ptraced())
    };

    let enable_suid = current_task.kernel().features.enable_suid && !no_new_privs && !is_ptraced;
    if enable_suid {
        resolved_program.file.name().apply_suid_and_sgid(&mut resolved_program.creds);
    }

    // On exec, the filesystem UIDs are always reset to the effective UIDs.
    resolved_program.creds.fsuid = resolved_program.creds.euid;
    resolved_program.creds.fsgid = resolved_program.creds.egid;

    // The effective user ID of the process is copied to the saved set-
    // user-ID; similarly, the effective group ID is copied to the saved
    // set-group-ID. This copying takes place after any effective ID
    // changes that occur because of the set-user-ID and set-group-ID
    // mode bits.
    resolved_program.creds.saved_uid = resolved_program.creds.euid;
    resolved_program.creds.saved_gid = resolved_program.creds.egid;

    common_cap::bprm_creds_from_file(current_task, resolved_program)?;

    Ok(())
}

/// Checks permissions for `execve`, updating [`ResolvedProgram`] with the post-exec
/// security state if allowed.
///
/// Corresponds to the `bprm_creds_for_exec()` LSM hook.
pub fn bprm_creds_for_exec(
    current_task: &CurrentTask,
    resolved_program: &mut ResolvedProgram,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.bprm_creds_for_exec");
    if let Some(state) = &current_task.kernel().security_state.state {
        if state.has_policy() {
            return selinux_hooks::task::bprm_creds_for_exec(
                &state.server,
                current_task,
                resolved_program,
            );
        } else {
            // SELinux is enabled but not yet configured, so apply the "init" SID.
            let previous_sid = current_task.current_creds().security_state.current_sid;
            resolved_program.creds.security_state =
                TaskAttrs::for_transition(InitialSid::Init.into(), previous_sid);
        }
    }
    Ok(())
}

/// Applies pre-commit security restrictions (such as revoking inaccessible file descriptors)
/// immediately before [`ResolvedProgram::creds`] are committed to the calling task.
///
/// Corresponds to the `bprm_committing_creds()` LSM hook.
pub fn bprm_committing_creds(
    current_task: &CurrentTask,
    resolved_program: &ResolvedProgram,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.bprm_committing_creds");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::task::bprm_committing_creds(security_server, current_task, resolved_program);
        Ok(())
    })
}

/// Called immediately after new credentials have been applied to the process during `exec()`.
///
/// Corresponds to the `bprm_committed_creds()` LSM hook.
pub fn bprm_committed_creds(current_task: &CurrentTask) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.bprm_committed_creds");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::task::bprm_committed_creds(security_server, current_task);
        Ok(())
    })
}

/// Checks if `source` may exercise the "getsched" permission on `target`.
/// Corresponds to the `task_getscheduler()` LSM hook.
pub fn check_task_getscheduler_access(source: &CurrentTask, target: &Task) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.task_getscheduler");
    if_selinux_else_default_ok(source, |security_server| {
        selinux_hooks::task::check_getsched_access(
            &selinux_hooks::build_permission_check(source, security_server),
            &source,
            &target,
        )
    })
}

/// Checks if setsched is allowed.
/// Corresponds to the `task_setscheduler()` LSM hook.
pub fn check_task_setscheduler_access(source: &CurrentTask, target: &Task) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.task_setscheduler");
    if_selinux_else_default_ok(source, |security_server| {
        selinux_hooks::task::check_setsched_access(
            &selinux_hooks::build_permission_check(source, security_server),
            &source,
            &target,
        )
    })
}

/// Checks if setting nice value is allowed.
/// Corresponds to the `task_setnice()` LSM hook.
pub fn check_task_setnice_access(source: &CurrentTask, target: &Task) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_task_setnice_access");
    if_selinux_else_default_ok(source, |security_server| {
        selinux_hooks::task::check_setsched_access(
            &selinux_hooks::build_permission_check(source, security_server),
            &source,
            &target,
        )
    })
}

/// Checks if getpgid is allowed.
/// Corresponds to the `task_getpgid()` LSM hook.
pub fn check_getpgid_access(source: &CurrentTask, target: &Task) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_getpgid_access");
    if_selinux_else_default_ok(source, |security_server| {
        selinux_hooks::task::check_getpgid_access(
            &selinux_hooks::build_permission_check(source, security_server),
            &source,
            &target,
        )
    })
}

/// Checks if setpgid is allowed.
/// Corresponds to the `task_setpgid()` LSM hook.
pub fn check_setpgid_access(source: &CurrentTask, target: &Task) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_setpgid_access");
    if_selinux_else_default_ok(source, |security_server| {
        selinux_hooks::task::check_setpgid_access(
            &selinux_hooks::build_permission_check(source, security_server),
            &source,
            &target,
        )
    })
}

/// Called when the current task queries the session Id of the `target` task.
/// Corresponds to the `task_getsid()` LSM hook.
pub fn check_task_getsid(source: &CurrentTask, target: &Task) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_task_getsid");
    if_selinux_else_default_ok(source, |security_server| {
        selinux_hooks::task::check_task_getsid(
            &selinux_hooks::build_permission_check(source, security_server),
            &source,
            &target,
        )
    })
}

/// Called when the current task queries the Linux capabilities of the `target` task.
/// Corresponds to the `capget()` LSM hook.
pub fn check_getcap_access(source: &CurrentTask, target: &Task) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_getcap_access");
    if_selinux_else_default_ok(source, |security_server| {
        selinux_hooks::task::check_getcap_access(
            &selinux_hooks::build_permission_check(source, security_server),
            &source,
            &target,
        )
    })
}

/// Called when the current task attempts to set the Linux capabilities of the `target`
/// task.
/// Corresponds to the `capset()` LSM hook.
pub fn check_setcap_access(source: &CurrentTask, target: &Task) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_setcap_access");
    if_selinux_else_default_ok(source, |security_server| {
        selinux_hooks::task::check_setcap_access(
            &selinux_hooks::build_permission_check(source, security_server),
            &source,
            &target,
        )
    })
}

/// Checks if sending a signal is allowed.
/// Corresponds to the `task_kill()` LSM hook.
pub fn check_signal_access(
    source: &CurrentTask,
    target: &Task,
    signal: Signal,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_signal_access");
    if_selinux_else_default_ok(source, |security_server| {
        selinux_hooks::task::check_signal_access(
            &selinux_hooks::build_permission_check(source, security_server),
            &source,
            &target,
            signal,
        )
    })
}

/// Checks if a particular syslog action is allowed.
/// Corresponds to the `task_syslog()` LSM hook.
pub fn check_syslog_access(source: &CurrentTask, action: SyslogAction) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_syslog_access");
    if_selinux_else_default_ok(source, |security_server| {
        selinux_hooks::task::check_syslog_access(
            &selinux_hooks::build_permission_check(source, security_server),
            &source,
            action,
        )
    })
}

/// Checks whether the `parent_tracer_task` is allowed to trace the `current_task`.
/// Corresponds to the `ptrace_traceme()` LSM hook.
pub fn ptrace_traceme(current_task: &CurrentTask, parent_tracer_task: &Task) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.ptrace_traceme");
    yama::ptrace_traceme(current_task, parent_tracer_task)?;
    common_cap::ptrace_traceme(current_task, parent_tracer_task)?;
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::task::ptrace_traceme(
            &selinux_hooks::build_permission_check(current_task, security_server),
            current_task,
            parent_tracer_task,
        )
    })
}

/// Checks whether the current `current_task` is allowed to trace `tracee_task`.
/// Corresponds to the `ptrace_access_check()` LSM hook.
pub fn ptrace_access_check(
    current_task: &CurrentTask,
    tracee_task: &Task,
    mode: PtraceAccessMode,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.ptrace_access_check");
    yama::ptrace_access_check(current_task, tracee_task, mode)?;
    common_cap::ptrace_access_check(current_task, tracee_task, mode)?;
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::task::ptrace_access_check(
            &selinux_hooks::build_permission_check(current_task, security_server),
            current_task,
            tracee_task,
            mode,
        )
    })
}

/// Called when the current task calls prlimit on a different task.
/// Corresponds to the `task_prlimit()` LSM hook.
pub fn task_prlimit(
    source: &CurrentTask,
    target: &Task,
    check_get_rlimit: bool,
    check_set_rlimit: bool,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.task_prlimit");
    if_selinux_else_default_ok(source, |security_server| {
        selinux_hooks::task::task_prlimit(
            &selinux_hooks::build_permission_check(source, security_server),
            &source,
            &target,
            check_get_rlimit,
            check_set_rlimit,
        )
    })
}

/// Called before `source` sets the resource limits of `target` from `old_limit` to `new_limit`.
/// Corresponds to the `security_task_setrlimit` hook.
pub fn task_setrlimit(
    source: &CurrentTask,
    target: &Task,
    old_limit: rlimit,
    new_limit: rlimit,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.task_setrlimit");
    if_selinux_else_default_ok(source, |security_server| {
        selinux_hooks::task::task_setrlimit(
            &selinux_hooks::build_permission_check(source, security_server),
            &source,
            &target,
            old_limit,
            new_limit,
        )
    })
}

/// Check permission before mounting `fs`.
/// Corresponds to the `sb_kern_mount()` LSM hook.
pub fn sb_kern_mount(current_task: &CurrentTask, fs: &FileSystem) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.sb_kern_mount");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::superblock::sb_kern_mount(
            &selinux_hooks::build_permission_check(current_task, security_server),
            current_task,
            fs,
        )
    })
}

/// Check permission before mounting to `path`. `flags` contains the mount flags that determine the
/// kind of mount operation done, and therefore the permissions that the caller requires.
/// Corresponds to the `sb_mount()` LSM hook.
pub fn sb_mount(
    current_task: &CurrentTask,
    path: &NamespaceNode,
    flags: MountFlags,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.sb_mount");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::superblock::sb_mount(
            &selinux_hooks::build_permission_check(current_task, security_server),
            current_task,
            path,
            flags,
        )
    })
}

/// Checks permission before remounting `mount` with `new_mount_params`.
/// Corresponds to the `sb_remount()` LSM hook.
pub fn sb_remount(
    current_task: &CurrentTask,
    mount: &Mount,
    new_mount_options: FileSystemMountOptions,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.sb_remount");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::superblock::sb_remount(security_server, mount, new_mount_options)
    })
}

/// Returns a `Display` implementation that Writes the LSM mount options of `fs` into `buf`.
/// Corresponds to the `sb_show_options` LSM hook.
pub fn sb_show_options<'a>(
    _kernel: &Kernel,
    fs: &'a FileSystem,
) -> Result<impl std::fmt::Display + 'a, Errno> {
    track_hook_duration!("security.hooks.sb_show_options");
    selinux_hooks::superblock::sb_show_options(fs)
}

/// Checks if `current_task` has the permission to get the filesystem statistics of `fs`.
/// Corresponds to the `sb_statfs()` LSM hook.
pub fn sb_statfs(current_task: &CurrentTask, fs: &FileSystem) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.sb_statfs");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::superblock::sb_statfs(
            &selinux_hooks::build_permission_check(current_task, security_server),
            current_task,
            fs,
        )
    })
}

/// Checks if `current_task` has the permission to unmount the filesystem mounted on
/// `node` using the unmount flags `flags`.
/// Corresponds to the `sb_umount()` LSM hook.
pub fn sb_umount(
    current_task: &CurrentTask,
    node: &NamespaceNode,
    flags: UnmountFlags,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.sb_umount");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::superblock::sb_umount(
            &selinux_hooks::build_permission_check(current_task, security_server),
            current_task,
            node,
            flags,
        )
    })
}

/// Checks if `current_task` has the permission to read file attributes for  `fs_node`.
/// Corresponds to the `inode_getattr()` hook.
pub fn check_fs_node_getattr_access(
    current_task: &CurrentTask,
    fs_node: &FsNode,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_fs_node_getattr_access");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::fs_node::check_fs_node_getattr_access(security_server, current_task, fs_node)
    })
}

/// Returns true if the security subsystem should skip capability checks on access to the named
/// attribute, false otherwise.
pub fn fs_node_xattr_skipcap(name: &FsStr) -> bool {
    selinux_hooks::fs_node::fs_node_xattr_skipcap(name)
}

/// This is called by Starnix even for filesystems which support extended attributes, unlike Linux
/// LSM.
/// Partially corresponds to the `inode_setxattr()` LSM hook: It is equivalent to
/// `inode_setxattr()` for non-security xattrs, while `fs_node_setsecurity()` is always called for
/// security xattrs. See also [`fs_node_setsecurity()`].
pub fn check_fs_node_setxattr_access(
    current_task: &CurrentTask,
    fs_node: &FsNode,
    name: &FsStr,
    value: &FsStr,
    op: XattrOp,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_fs_node_setxattr_access");
    common_cap::fs_node_setxattr(current_task, fs_node, name, value, op)?;
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::fs_node::check_fs_node_setxattr_access(
            security_server,
            current_task,
            fs_node,
            name,
            value,
            op,
        )
    })
}

/// Corresponds to the `inode_getxattr()` LSM hook.
pub fn check_fs_node_getxattr_access(
    current_task: &CurrentTask,
    fs_node: &FsNode,
    name: &FsStr,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_fs_node_getxattr_access");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::fs_node::check_fs_node_getxattr_access(
            security_server,
            current_task,
            fs_node,
            name,
        )
    })
}

/// Corresponds to the `inode_listxattr()` LSM hook.
pub fn check_fs_node_listxattr_access(
    current_task: &CurrentTask,
    fs_node: &FsNode,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_fs_node_listxattr_access");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::fs_node::check_fs_node_listxattr_access(
            security_server,
            current_task,
            fs_node,
        )
    })
}

/// Corresponds to the `inode_removexattr()` LSM hook.
pub fn check_fs_node_removexattr_access(
    current_task: &CurrentTask,
    fs_node: &FsNode,
    name: &FsStr,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_fs_node_removexattr_access");
    common_cap::fs_node_removexattr(current_task, fs_node, name)?;
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::fs_node::check_fs_node_removexattr_access(
            security_server,
            current_task,
            fs_node,
            name,
        )
    })
}

/// If SELinux is enabled and `fs_node` is in a filesystem without xattr support, returns the xattr
/// name for the security label associated with inode. Otherwise returns None.
///
/// This hook is called from the `listxattr` syscall.
///
/// Corresponds to the `inode_listsecurity()` LSM hook.
pub fn fs_node_listsecurity(current_task: &CurrentTask, fs_node: &FsNode) -> Option<FsString> {
    track_hook_duration!("security.hooks.fs_node_listsecurity");
    if_selinux_else(
        current_task,
        |_| selinux_hooks::fs_node::fs_node_listsecurity(fs_node),
        || None,
    )
}

/// Returns the value of the specified "security.*" attribute for `fs_node`.
/// If SELinux is enabled then requests for the "security.selinux" attribute will return the
/// Security Context corresponding to the SID with which `fs_node` has been labeled, even if the
/// node's file system does not generally support extended attributes.
/// If SELinux is not enabled, or the node is not labeled with a SID, then the call is delegated to
/// the [`crate::vfs::FsNodeOps`], so the returned value may not be a valid Security Context.
/// Corresponds to the `inode_getsecurity()` LSM hook.
pub fn fs_node_getsecurity(
    current_task: &CurrentTask,
    fs_node: &FsNode,
    name: &FsStr,
    max_size: usize,
) -> Result<ValueOrSize<FsString>, Errno> {
    track_hook_duration!("security.hooks.fs_node_getsecurity");
    if_selinux_else(
        current_task,
        |security_server| {
            selinux_hooks::fs_node::fs_node_getsecurity(
                security_server,
                current_task,
                fs_node,
                name,
                max_size,
            )
        },
        || fs_node.ops().get_xattr(fs_node, current_task, name, max_size),
    )
}

/// Called when an extended attribute with "security."-prefixed `name` is being set, after having
/// passed the discretionary and `check_fs_node_setxattr_access()` permission-checks.
/// This allows the LSM (e.g. SELinux) to update internal state as necessary for xattr changes.
///
/// Partially corresponds to the `inode_setsecurity()` and `inode_post_setxattr()` LSM hooks.
pub fn fs_node_setsecurity(
    current_task: &CurrentTask,
    fs_node: &FsNode,
    name: &FsStr,
    value: &FsStr,
    op: XattrOp,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.fs_node_setsecurity");
    if_selinux_else(
        current_task,
        |security_server| {
            selinux_hooks::fs_node::fs_node_setsecurity(
                security_server,
                current_task,
                fs_node,
                name,
                value,
                op,
            )
        },
        || fs_node.ops().set_xattr(fs_node, current_task, name, value, op),
    )
}

/// Checks whether `current_task` can perform the given bpf `cmd`. This hook is called from the
/// `sys_bpf()` syscall after the attribute is copied into the kernel.
/// Corresponds to the `bpf()` LSM hook.
pub fn check_bpf_access<Attr: FromBytes>(
    current_task: &CurrentTask,
    cmd: bpf_cmd,
    attr: &Attr,
    attr_size: u32,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_bpf_access");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::bpf::check_bpf_access(security_server, current_task, cmd, attr, attr_size)
    })
}

/// Checks whether `current_task` can create a bpf_map. This hook is called from the
/// `sys_bpf()` syscall when the kernel tries to generate and return a file descriptor for maps.
/// Corresponds to the `bpf_map()` LSM hook.
pub fn check_bpf_map_access(
    current_task: &CurrentTask,
    bpf_map_state: &BpfMapState,
    flags: PermissionFlags,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_bpf_map_access");
    if_selinux_else_default_ok(current_task, |security_server| {
        let subject_sid = current_task_state(current_task).current_sid;
        selinux_hooks::bpf::check_bpf_map_access(
            security_server,
            current_task,
            subject_sid,
            bpf_map_state,
            flags,
        )
    })
}

/// Checks whether `current_task` can create a bpf_program. This hook is called from the
/// `sys_bpf()` syscall when the kernel tries to generate and return a file descriptor for
/// programs.
/// Corresponds to the `bpf_prog()` LSM hook.
pub fn check_bpf_prog_access(
    current_task: &CurrentTask,
    bpf_program_state: &BpfProgState,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_bpf_prog_access");
    if_selinux_else_default_ok(current_task, |security_server| {
        let subject_sid = current_task_state(current_task).current_sid;
        selinux_hooks::bpf::check_bpf_prog_access(
            security_server,
            current_task,
            subject_sid,
            bpf_program_state,
        )
    })
}

/// Checks whether `current_task` has the correct permissions to monitor the given target task or
/// tasks.
/// Corresponds to the `perf_event_open` LSM hook.
pub fn check_perf_event_open_access(
    current_task: &CurrentTask,
    target_task_type: TargetTaskType<'_>,
    attr: &perf_event_attr,
    event_type: PerfEventType,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_perf_event_open_access");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::perf_event::check_perf_event_open_access(
            security_server,
            current_task,
            target_task_type,
            attr,
            event_type,
        )
    })
}

/// Returns the security context to be assigned to a PerfEventFileState, based on the task that
/// creates it.
/// Corresponds to the `perf_event_alloc` LSM hook.
pub fn perf_event_alloc(current_task: &CurrentTask) -> PerfEventState {
    track_hook_duration!("security.hooks.perf_event_alloc");
    PerfEventState { state: selinux_hooks::perf_event::perf_event_alloc(current_task) }
}

/// Checks whether `current_task` has the correct permissions to read the given `perf_event_file`
/// Corresponds to the `perf_event_read` LSM hook.
pub fn check_perf_event_read_access(
    current_task: &CurrentTask,
    perf_event_file: &PerfEventFile,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_perf_event_read_access");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::perf_event::check_perf_event_read_access(
            security_server,
            current_task,
            perf_event_file,
        )
    })
}

/// Checks whether `current_task` has the correct permissions to write to the given `perf_event_file`.
/// Corresponds to the `perf_event_write` LSM hook.
pub fn check_perf_event_write_access(
    current_task: &CurrentTask,
    perf_event_file: &PerfEventFile,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.check_perf_event_write_access");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::perf_event::check_perf_event_write_access(
            security_server,
            current_task,
            perf_event_file,
        )
    })
}

/// Identifies one of the Security Context attributes associated with a task.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ProcAttr {
    Current,
    Exec,
    FsCreate,
    KeyCreate,
    Previous,
    SockCreate,
}

/// Returns the Security Context associated with the `name`ed entry for the specified `target` task.
/// Corresponds to the `getprocattr()` LSM hook.
pub fn get_procattr(
    current_task: &CurrentTask,
    target: &Task,
    attr: ProcAttr,
) -> Result<Vec<u8>, Errno> {
    track_hook_duration!("security.hooks.get_procattr");
    if_selinux_else(
        current_task,
        |security_server| {
            selinux_hooks::task::get_procattr(security_server, current_task, target, attr)
        },
        // If SELinux is disabled then there are no values to return.
        || error!(EINVAL),
    )
}

/// Sets the Security Context associated with the `name`ed entry for the current task.
/// Corresponds to the `setprocattr()` LSM hook.
pub fn set_procattr(
    current_task: &CurrentTask,
    attr: ProcAttr,
    context: &[u8],
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.set_procattr");
    if_selinux_else(
        current_task,
        |security_server| {
            selinux_hooks::task::set_procattr(security_server, current_task, attr, context)
        },
        // If SELinux is disabled then no writes are accepted.
        || error!(EINVAL),
    )
}

/// Returns true if SELinux is enabled on the kernel for this task.
pub fn fs_is_xattr_labeled(fs: FileSystemHandle) -> bool {
    fs.security_state.state.supports_xattr()
}

/// Stashes a reference to the selinuxfs null file for later use by hooks that remap
/// inaccessible file descriptors to null.
pub fn selinuxfs_init_null(current_task: &CurrentTask, null_fs_node: &FileHandle) {
    // Note: No `if_selinux_...` guard because hook is invoked inside selinuxfs initialization code;
    // i.e., hook is only invoked when selinux is enabled.
    selinux_hooks::selinuxfs::selinuxfs_init_null(current_task, null_fs_node)
}

/// Called by the "selinuxfs" when a policy has been successfully loaded, to allow policy-dependent
/// initialization to be completed. This includes resolving labeling schemes and state for
/// file-systems mounted prior to policy load (e.g. the "selinuxfs" itself), and initializing
/// security state for any file nodes they may already contain.
// TODO: https://fxbug.dev/362917997 - Remove this when SELinux LSM is modularized.
pub fn selinuxfs_policy_loaded(current_task: &CurrentTask) {
    track_hook_duration!("security.hooks.selinuxfs_policy_loaded");
    selinux_hooks::selinuxfs::selinuxfs_policy_loaded(current_task)
}

/// Used by the "selinuxfs" module to access the SELinux administration API, if enabled.
// TODO: https://fxbug.dev/335397745 - Return a more restricted API, or ...
// TODO: https://fxbug.dev/362917997 - Remove this when SELinux LSM is modularized.
pub fn selinuxfs_get_admin_api(current_task: &CurrentTask) -> Option<Arc<SecurityServer>> {
    current_task.kernel().security_state.state.as_ref().map(|state| state.server.clone())
}

/// Used by the "selinuxfs" module to perform checks on SELinux API file accesses.
// TODO: https://fxbug.dev/362917997 - Remove this when SELinux LSM is modularized.
pub fn selinuxfs_check_access(
    current_task: &CurrentTask,
    permission: SecurityPermission,
) -> Result<(), Errno> {
    track_hook_duration!("security.hooks.selinuxfs_check_access");
    if_selinux_else_default_ok(current_task, |security_server| {
        selinux_hooks::selinuxfs::selinuxfs_check_access(security_server, current_task, permission)
    })
}

/// Marks the credentials as being used for an internal operation. All SELinux permission checks
/// will be skipped on this task.
pub fn creds_start_internal_operation(current_task: &CurrentTask) -> Arc<Credentials> {
    track_hook_duration!("security.hooks.creds_start_internal_operation");
    let mut creds = Credentials::clone(&current_task.current_creds());
    creds.security_state.internal_operation = true;
    creds.into()
}

pub mod testing {
    use super::{Arc, KernelState, SecurityServer, selinux_hooks};
    use starnix_sync::LockDepMutex;
    use std::sync::OnceLock;
    use std::sync::atomic::AtomicU64;

    /// Used by Starnix' `testing.rs` to create `KernelState` wrapping a test-
    /// supplied `SecurityServer`.
    pub fn kernel_state(security_server: Option<Arc<SecurityServer>>) -> KernelState {
        let state = security_server.map(|server| selinux_hooks::KernelState {
            server,
            pending_file_systems: LockDepMutex::default(),
            selinuxfs_null: OnceLock::default(),
            access_denial_count: AtomicU64::new(0u64),
            has_policy: false.into(),
            _inspect_node: fuchsia_inspect::Node::default(),
        });
        KernelState { state }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security;
    use crate::security::selinux_hooks::get_cached_sid;
    use crate::security::selinux_hooks::testing::{
        self, spawn_kernel_with_selinux_hooks_test_policy_and_run,
    };
    use crate::testing::{create_task, spawn_kernel_and_run, spawn_kernel_with_selinux_and_run};
    use linux_uapi::XATTR_NAME_SELINUX;
    use selinux::InitialSid;
    use starnix_uapi::auth::PTRACE_MODE_ATTACH;
    use starnix_uapi::signals::SIGTERM;

    const VALID_SECURITY_CONTEXT: &[u8] = b"u:object_r:test_valid_t:s0";
    const VALID_SECURITY_CONTEXT_WITH_NUL: &[u8] = b"u:object_r:test_valid_t:s0\0";

    const DIFFERENT_VALID_SECURITY_CONTEXT: &[u8] = b"u:object_r:test_different_valid_t:s0";
    const DIFFERENT_VALID_SECURITY_CONTEXT_WITH_NUL: &[u8] =
        b"u:object_r:test_different_valid_t:s0\0";

    const INVALID_SECURITY_CONTEXT_INTERNAL_NUL: &[u8] = b"u:object_r:test_valid_\0t:s0";

    const INVALID_SECURITY_CONTEXT: &[u8] = b"not_a_u:object_r:test_valid_t:s0";

    #[derive(Default, Debug, PartialEq)]
    enum TestHookResult {
        WasRun,
        WasNotRun,
        #[default]
        WasNotRunDefault,
    }

    #[fuchsia::test]
    async fn if_selinux_else_disabled() {
        spawn_kernel_and_run(async |current_task| {
            assert!(current_task.kernel().security_state.state.is_none());

            let check_result =
                if_selinux_else_default_ok(current_task, |_| Ok(TestHookResult::WasRun));
            assert_eq!(check_result, Ok(TestHookResult::WasNotRunDefault));

            let run_else_result = if_selinux_else(
                current_task,
                |_| TestHookResult::WasRun,
                || TestHookResult::WasNotRun,
            );
            assert_eq!(run_else_result, TestHookResult::WasNotRun);
        })
        .await;
    }

    #[fuchsia::test]
    async fn if_selinux_else_without_policy() {
        spawn_kernel_with_selinux_and_run(async |current_task, _security_server| {
            let check_result =
                if_selinux_else_default_ok(current_task, |_| Ok(TestHookResult::WasRun));
            assert_eq!(check_result, Ok(TestHookResult::WasNotRunDefault));

            let run_else_result = if_selinux_else(
                current_task,
                |_| TestHookResult::WasRun,
                || TestHookResult::WasNotRun,
            );
            assert_eq!(run_else_result, TestHookResult::WasNotRun);
        })
        .await;
    }

    #[fuchsia::test]
    async fn if_selinux_else_with_policy() {
        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, _security_server| {
            let check_result =
                if_selinux_else_default_ok(current_task, |_| Ok(TestHookResult::WasRun));
            assert_eq!(check_result, Ok(TestHookResult::WasRun));

            let run_else_result = if_selinux_else(
                current_task,
                |_| TestHookResult::WasRun,
                || TestHookResult::WasNotRun,
            );
            assert_eq!(run_else_result, TestHookResult::WasRun);
        })
        .await;
    }

    #[fuchsia::test]
    async fn task_create_access_allowed_for_selinux_disabled() {
        spawn_kernel_and_run(async |current_task| {
            assert!(current_task.kernel().security_state.state.is_none());
            assert_eq!(check_task_create_access(current_task), Ok(()));
        })
        .await;
    }

    #[fuchsia::test]
    async fn task_create_access_allowed_for_permissive_mode() {
        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, security_server| {
            security_server.set_enforcing(false);
            assert_eq!(check_task_create_access(current_task), Ok(()));
        })
        .await;
    }

    #[fuchsia::test]
    async fn exec_access_allowed_for_selinux_disabled() {
        spawn_kernel_and_run(async |current_task| {
            assert!(current_task.kernel().security_state.state.is_none());
            let executable = testing::create_test_file(current_task);
            let file = testing::open_test_file(current_task, &executable);
            let mut resolved_program = testing::make_resolved_program(current_task, file);
            assert_eq!(bprm_creds_for_exec(current_task, &mut resolved_program), Ok(()));
        })
        .await;
    }

    #[fuchsia::test]
    async fn exec_access_allowed_for_permissive_mode() {
        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, security_server| {
            security_server.set_enforcing(false);
            let executable = testing::create_test_file(current_task);
            let file = testing::open_test_file(current_task, &executable);
            let mut resolved_program = testing::make_resolved_program(current_task, file);
            // Expect that access is granted.
            let result = bprm_creds_for_exec(current_task, &mut resolved_program);
            assert!(result.is_ok());
        })
        .await;
    }

    #[fuchsia::test]
    async fn exec_no_state_update_for_selinux_disabled() {
        spawn_kernel_and_run(async |current_task| {
            let target_sid = InitialSid::Unlabeled.into();

            assert!(selinux_hooks::current_task_state(current_task).current_sid != target_sid);

            // Set exec_sid to cause the hook to apply a transition, to verify if it is updated or not.
            testing::mutate_attrs_for_test(current_task, |attrs| {
                attrs.exec_sid = Some(target_sid);
            });

            let executable = testing::create_test_file(current_task);
            let file = testing::open_test_file(current_task, &executable);
            let mut resolved_program = testing::make_resolved_program(current_task, file);

            let before_hook_sid = selinux_hooks::current_task_state(current_task).current_sid;

            bprm_creds_for_exec(current_task, &mut resolved_program).unwrap();
            assert_eq!(resolved_program.creds.security_state.current_sid, before_hook_sid);
        })
        .await;
    }

    #[fuchsia::test]
    async fn exec_initial_context_for_selinux_without_policy() {
        spawn_kernel_with_selinux_and_run(async |current_task, _security_server| {
            let elf_sid = InitialSid::Unlabeled.into();
            assert_ne!(selinux_hooks::current_task_state(current_task).current_sid, elf_sid);
            assert_ne!(
                selinux_hooks::current_task_state(current_task).current_sid,
                InitialSid::Init.into()
            );

            // Set exec_sid to cause the hook to apply a transition, to verify if it is updated or not.
            testing::mutate_attrs_for_test(current_task, |attrs| {
                attrs.exec_sid = Some(elf_sid);
            });

            let executable = testing::create_test_file(current_task);
            let file = testing::open_test_file(current_task, &executable);
            let mut resolved_program = testing::make_resolved_program(current_task, file);

            bprm_creds_for_exec(current_task, &mut resolved_program).unwrap();

            assert_eq!(resolved_program.creds.security_state.current_sid, InitialSid::Init.into());
        })
        .await;
    }

    #[fuchsia::test]
    async fn exec_state_update_for_permissive_mode() {
        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, security_server| {
            security_server.set_enforcing(false);
            let elf_sid = security_server
                .security_context_to_sid(b"u:object_r:fork_no_t:s0".into())
                .expect("invalid security context");

            assert_ne!(elf_sid, selinux_hooks::current_task_state(current_task).current_sid);

            // Set exec_sid to cause the hook to apply a transition, to verify if it is updated or not.
            testing::mutate_attrs_for_test(current_task, |attrs| {
                attrs.exec_sid = Some(elf_sid);
            });

            let executable = testing::create_test_file(current_task);
            let file = testing::open_test_file(current_task, &executable);
            let mut resolved_program = testing::make_resolved_program(current_task, file);

            bprm_creds_for_exec(current_task, &mut resolved_program).unwrap();
            assert_eq!(resolved_program.creds.security_state.current_sid, elf_sid);
        })
        .await;
    }

    #[fuchsia::test]
    async fn getsched_access_allowed_for_selinux_disabled() {
        spawn_kernel_and_run(async |current_task| {
            let another_task = create_task(&current_task.kernel(), "another-task");
            assert_eq!(check_task_getscheduler_access(current_task, &another_task), Ok(()));
        })
        .await;
    }

    #[fuchsia::test]
    async fn getsched_access_allowed_for_permissive_mode() {
        spawn_kernel_with_selinux_and_run(async |current_task, _security_server| {
            let another_task = create_task(&current_task.kernel(), "another-task");
            assert_eq!(check_task_getscheduler_access(current_task, &another_task), Ok(()));
        })
        .await;
    }

    #[fuchsia::test]
    async fn setsched_access_allowed_for_selinux_disabled() {
        spawn_kernel_and_run(async |current_task| {
            let another_task = create_task(&current_task.kernel(), "another-task");
            assert_eq!(check_task_setscheduler_access(current_task, &another_task), Ok(()));
        })
        .await;
    }

    #[fuchsia::test]
    async fn setsched_access_allowed_for_permissive_mode() {
        spawn_kernel_with_selinux_and_run(async |current_task, _security_server| {
            let another_task = create_task(&current_task.kernel(), "another-task");
            assert_eq!(check_task_setscheduler_access(current_task, &another_task), Ok(()));
        })
        .await;
    }

    #[fuchsia::test]
    async fn getpgid_access_allowed_for_selinux_disabled() {
        spawn_kernel_and_run(async |current_task| {
            let another_task = create_task(&current_task.kernel(), "another-task");
            assert_eq!(check_getpgid_access(current_task, &another_task), Ok(()));
        })
        .await;
    }

    #[fuchsia::test]
    async fn getpgid_access_allowed_for_permissive_mode() {
        spawn_kernel_with_selinux_and_run(async |current_task, _security_server| {
            let another_task = create_task(&current_task.kernel(), "another-task");
            assert_eq!(check_getpgid_access(current_task, &another_task), Ok(()));
        })
        .await;
    }

    #[fuchsia::test]
    async fn setpgid_access_allowed_for_selinux_disabled() {
        spawn_kernel_and_run(async |current_task| {
            let another_task = create_task(&current_task.kernel(), "another-task");
            assert_eq!(check_setpgid_access(current_task, &another_task), Ok(()));
        })
        .await;
    }

    #[fuchsia::test]
    async fn setpgid_access_allowed_for_permissive_mode() {
        spawn_kernel_with_selinux_and_run(async |current_task, _security_server| {
            let another_task = create_task(&current_task.kernel(), "another-task");
            assert_eq!(check_setpgid_access(current_task, &another_task), Ok(()));
        })
        .await;
    }

    #[fuchsia::test]
    async fn task_getsid_allowed_for_selinux_disabled() {
        spawn_kernel_and_run(async |current_task| {
            let another_task = create_task(&current_task.kernel(), "another-task");
            assert_eq!(check_task_getsid(current_task, &another_task), Ok(()));
        })
        .await;
    }

    #[fuchsia::test]
    async fn task_getsid_allowed_for_permissive_mode() {
        spawn_kernel_with_selinux_and_run(async |current_task, _security_server| {
            let another_task = create_task(&current_task.kernel(), "another-task");
            assert_eq!(check_task_getsid(current_task, &another_task), Ok(()));
        })
        .await;
    }

    #[fuchsia::test]
    async fn signal_access_allowed_for_selinux_disabled() {
        spawn_kernel_and_run(async |current_task| {
            let another_task = create_task(&current_task.kernel(), "another-task");
            assert_eq!(check_signal_access(current_task, &another_task, SIGTERM), Ok(()));
        })
        .await;
    }

    #[fuchsia::test]
    async fn signal_access_allowed_for_permissive_mode() {
        spawn_kernel_with_selinux_and_run(async |current_task, _security_server| {
            let another_task = create_task(&current_task.kernel(), "another-task");
            assert_eq!(check_signal_access(current_task, &another_task, SIGTERM), Ok(()));
        })
        .await;
    }

    #[fuchsia::test]
    async fn ptrace_traceme_access_allowed_for_selinux_disabled() {
        spawn_kernel_and_run(async |current_task| {
            let another_task = create_task(&current_task.kernel(), "another-task");
            assert_eq!(ptrace_traceme(current_task, &another_task), Ok(()));
        })
        .await;
    }

    #[fuchsia::test]
    async fn ptrace_traceme_access_allowed_for_permissive_mode() {
        spawn_kernel_with_selinux_and_run(async |current_task, _security_server| {
            let another_task = create_task(&current_task.kernel(), "another-task");
            assert_eq!(ptrace_traceme(current_task, &another_task), Ok(()));
        })
        .await;
    }

    #[fuchsia::test]
    async fn ptrace_attach_access_allowed_for_selinux_disabled() {
        spawn_kernel_and_run(async |current_task| {
            let another_task = create_task(&current_task.kernel(), "another-task");
            assert_eq!(
                ptrace_access_check(current_task, &another_task, PTRACE_MODE_ATTACH),
                Ok(())
            );
        })
        .await;
    }

    #[fuchsia::test]
    async fn ptrace_attach_access_allowed_for_permissive_mode() {
        spawn_kernel_with_selinux_and_run(async |current_task, _security_server| {
            let another_task = create_task(&current_task.kernel(), "another-task");
            assert_eq!(
                ptrace_access_check(current_task, &another_task, PTRACE_MODE_ATTACH),
                Ok(())
            );
        })
        .await;
    }

    #[fuchsia::test]
    async fn task_prlimit_access_allowed_for_selinux_disabled() {
        spawn_kernel_and_run(async |current_task| {
            let another_task = create_task(&current_task.kernel(), "another-task");
            assert_eq!(task_prlimit(current_task, &another_task, true, true), Ok(()));
        })
        .await;
    }

    #[fuchsia::test]
    async fn task_prlimit_access_allowed_for_permissive_mode() {
        spawn_kernel_with_selinux_and_run(async |current_task, _security_server| {
            let another_task = create_task(&current_task.kernel(), "another-task");
            assert_eq!(task_prlimit(current_task, &another_task, true, true), Ok(()));
        })
        .await;
    }

    #[fuchsia::test]
    async fn fs_node_task_to_fs_node_noop_selinux_disabled() {
        spawn_kernel_and_run(async |current_task| {
            let node = &testing::create_test_file(current_task).entry.node;
            task_to_fs_node(current_task, &current_task.task, &node);
            assert_eq!(None, selinux_hooks::get_cached_sid(node));
        })
        .await;
    }

    #[fuchsia::test]
    async fn fs_node_setsecurity_selinux_disabled_only_sets_xattr() {
        spawn_kernel_and_run(async |current_task| {
            let node = &testing::create_test_file(current_task).entry.node;

            fs_node_setsecurity(
                &current_task,
                &node,
                XATTR_NAME_SELINUX.to_bytes().into(),
                VALID_SECURITY_CONTEXT.into(),
                XattrOp::Set,
            )
            .expect("set_xattr(security.selinux) failed");

            assert_eq!(None, selinux_hooks::get_cached_sid(node));
        })
        .await;
    }

    #[fuchsia::test]
    async fn fs_node_setsecurity_selinux_without_policy_only_sets_xattr() {
        spawn_kernel_with_selinux_and_run(async |current_task, _security_server| {
            let node = &testing::create_test_file(current_task).entry.node;
            fs_node_setsecurity(
                &current_task,
                &node,
                XATTR_NAME_SELINUX.to_bytes().into(),
                VALID_SECURITY_CONTEXT.into(),
                XattrOp::Set,
            )
            .expect("set_xattr(security.selinux) failed");

            assert_eq!(None, selinux_hooks::get_cached_sid(node));
        })
        .await;
    }

    #[fuchsia::test]
    async fn fs_node_setsecurity_selinux_permissive_sets_xattr_and_label() {
        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, security_server| {
            security_server.set_enforcing(false);
            let expected_sid = security_server
                .security_context_to_sid(VALID_SECURITY_CONTEXT.into())
                .expect("no SID for VALID_SECURITY_CONTEXT");
            let node = &testing::create_test_file(&current_task).entry.node;

            // Safeguard against a false positive by ensuring `expected_sid` is not already the file's label.
            assert_ne!(Some(expected_sid), selinux_hooks::get_cached_sid(node));

            fs_node_setsecurity(
                &current_task,
                &node,
                XATTR_NAME_SELINUX.to_bytes().into(),
                VALID_SECURITY_CONTEXT.into(),
                XattrOp::Set,
            )
            .expect("set_xattr(security.selinux) failed");

            // Verify that the SID now cached on the node is that SID
            // corresponding to VALID_SECURITY_CONTEXT.
            assert_eq!(Some(expected_sid), selinux_hooks::get_cached_sid(node));
        })
        .await;
    }

    #[fuchsia::test]
    async fn fs_node_setsecurity_not_selinux_only_sets_xattr() {
        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, security_server| {
            let valid_security_context_sid = security_server
                .security_context_to_sid(VALID_SECURITY_CONTEXT.into())
                .expect("no SID for VALID_SECURITY_CONTEXT");
            let node = &testing::create_test_file(current_task).entry.node;
            // The label assigned to the test file on creation must differ from
            // VALID_SECURITY_CONTEXT, otherwise this test may return a false
            // positive.
            let whatever_sid = selinux_hooks::get_cached_sid(node);
            assert_ne!(Some(valid_security_context_sid), whatever_sid);

            fs_node_setsecurity(
                &current_task,
                &node,
                "security.selinu!".into(), // Note: name != "security.selinux".
                VALID_SECURITY_CONTEXT.into(),
                XattrOp::Set,
            )
            .expect("set_xattr(security.selinux) failed");

            // Verify that the node's SID (whatever it was) has not changed.
            assert_eq!(whatever_sid, selinux_hooks::get_cached_sid(node));
        })
        .await;
    }

    #[fuchsia::test]
    async fn fs_node_setsecurity_selinux_enforcing_invalid_context_fails() {
        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, _security_server| {
            let node = &testing::create_test_file(current_task).entry.node;

            let before_sid = selinux_hooks::get_cached_sid(node);
            assert_ne!(Some(InitialSid::Unlabeled.into()), before_sid);

            assert!(
                check_fs_node_setxattr_access(
                    &current_task,
                    &node,
                    XATTR_NAME_SELINUX.to_bytes().into(),
                    "!".into(), // Note: Not a valid security context.
                    XattrOp::Set,
                )
                .is_err()
            );

            assert_eq!(before_sid, selinux_hooks::get_cached_sid(node));
        })
        .await;
    }

    #[fuchsia::test]
    async fn fs_node_setsecurity_selinux_permissive_invalid_context_sets_xattr_and_label() {
        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, security_server| {
            security_server.set_enforcing(false);
            let node = &testing::create_test_file(current_task).entry.node;

            assert_ne!(Some(InitialSid::Unlabeled.into()), selinux_hooks::get_cached_sid(node));

            fs_node_setsecurity(
                &current_task,
                &node,
                XATTR_NAME_SELINUX.to_bytes().into(),
                "!".into(), // Note: Not a valid security context.
                XattrOp::Set,
            )
            .expect("set_xattr(security.selinux) failed");

            assert_eq!(Some(InitialSid::Unlabeled.into()), selinux_hooks::get_cached_sid(node));
        })
        .await;
    }

    #[fuchsia::test]
    async fn fs_node_setsecurity_different_sid_for_different_context() {
        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, _security_server| {
            let node = &testing::create_test_file(current_task).entry.node;

            fs_node_setsecurity(
                &current_task,
                &node,
                XATTR_NAME_SELINUX.to_bytes().into(),
                VALID_SECURITY_CONTEXT.into(),
                XattrOp::Set,
            )
            .expect("set_xattr(security.selinux) failed");

            assert!(selinux_hooks::get_cached_sid(node).is_some());

            let first_sid = selinux_hooks::get_cached_sid(node).unwrap();
            fs_node_setsecurity(
                &current_task,
                &node,
                XATTR_NAME_SELINUX.to_bytes().into(),
                DIFFERENT_VALID_SECURITY_CONTEXT.into(),
                XattrOp::Set,
            )
            .expect("set_xattr(security.selinux) failed");

            assert!(selinux_hooks::get_cached_sid(node).is_some());

            let second_sid = selinux_hooks::get_cached_sid(node).unwrap();

            assert_ne!(first_sid, second_sid);
        })
        .await;
    }

    #[fuchsia::test]
    async fn fs_node_getsecurity_returns_cached_context() {
        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, security_server| {
            let node = &testing::create_test_file(current_task).entry.node;

            // Set a mismatched value in `node`'s "security.seliux" attribute.
            const TEST_VALUE: &str = "Something Random";
            node.ops()
                .set_xattr(
                    node,
                    current_task,
                    XATTR_NAME_SELINUX.to_bytes().into(),
                    TEST_VALUE.into(),
                    XattrOp::Set,
                )
                .expect("set_xattr(security.selinux) failed");

            // Attach a valid SID to the `node`.
            let sid = security_server
                .security_context_to_sid(VALID_SECURITY_CONTEXT.into())
                .expect("security context to SID");
            selinux_hooks::set_cached_sid(&node, sid);

            // Reading the security attribute should return the Security Context for the SID, rather than delegating.
            let result =
                fs_node_getsecurity(current_task, node, XATTR_NAME_SELINUX.to_bytes().into(), 4096);
            assert_eq!(
                result,
                Ok(ValueOrSize::Value(FsString::new(VALID_SECURITY_CONTEXT_WITH_NUL.into())))
            );
        })
        .await;
    }

    #[fuchsia::test]
    async fn fs_node_getsecurity_delegates_to_get_xattr() {
        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, security_server| {
            let node = &testing::create_test_file(current_task).entry.node;

            // Set an invalid value in `node`'s "security.selinux" attribute.
            // This requires SELinux to be in permissive mode, otherwise the "relabelto" permission check will fail.
            security_server.set_enforcing(false);
            const TEST_VALUE: &str = "Something Random";
            fs_node_setsecurity(
                &current_task,
                node,
                XATTR_NAME_SELINUX.to_bytes().into(),
                TEST_VALUE.into(),
                XattrOp::Set,
            )
            .expect("set_xattr(security.selinux) failed");
            security_server.set_enforcing(true);

            // Reading the security attribute should pass-through to read the value from the file system.
            let result =
                fs_node_getsecurity(current_task, node, XATTR_NAME_SELINUX.to_bytes().into(), 4096);
            assert_eq!(result, Ok(ValueOrSize::Value(FsString::new(TEST_VALUE.into()))));
        })
        .await;
    }

    #[fuchsia::test]
    async fn set_get_procattr() {
        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, _security_server| {
            assert_eq!(get_procattr(current_task, current_task, ProcAttr::Exec), Ok(Vec::new()));

            assert_eq!(
                // Test policy allows "kernel_t" tasks to set the "exec" context.
                set_procattr(current_task, ProcAttr::Exec, VALID_SECURITY_CONTEXT.into()),
                Ok(())
            );

            assert_eq!(
                // Test policy does not allow "kernel_t" tasks to set the "sockcreate" context.
                set_procattr(
                    current_task,
                    ProcAttr::SockCreate,
                    DIFFERENT_VALID_SECURITY_CONTEXT.into()
                ),
                error!(EACCES)
            );

            assert_eq!(
                // It is never permitted to set the "previous" context.
                set_procattr(
                    current_task,
                    ProcAttr::Previous,
                    DIFFERENT_VALID_SECURITY_CONTEXT.into()
                ),
                error!(EINVAL)
            );

            assert_eq!(
                // Cannot set an invalid context.
                set_procattr(current_task, ProcAttr::Exec, INVALID_SECURITY_CONTEXT.into()),
                error!(EINVAL)
            );

            assert_eq!(
                get_procattr(current_task, current_task, ProcAttr::Exec),
                Ok(VALID_SECURITY_CONTEXT_WITH_NUL.into())
            );

            assert!(get_procattr(current_task, current_task, ProcAttr::Current).is_ok());
        })
        .await;
    }

    #[fuchsia::test]
    async fn set_get_procattr_with_nulls() {
        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, _security_server| {
            assert_eq!(get_procattr(current_task, current_task, ProcAttr::Exec), Ok(Vec::new()));

            assert_eq!(
                // Setting a Context with a string with trailing null(s) should work, if the Context is valid.
                set_procattr(current_task, ProcAttr::Exec, VALID_SECURITY_CONTEXT_WITH_NUL.into()),
                Ok(())
            );

            assert_eq!(
                // Nulls in the middle of an otherwise valid Context truncate it, rendering it invalid.
                set_procattr(
                    current_task,
                    ProcAttr::FsCreate,
                    INVALID_SECURITY_CONTEXT_INTERNAL_NUL.into()
                ),
                error!(EINVAL)
            );

            assert_eq!(
                get_procattr(current_task, current_task, ProcAttr::Exec),
                Ok(VALID_SECURITY_CONTEXT_WITH_NUL.into())
            );

            assert_eq!(
                get_procattr(current_task, current_task, ProcAttr::FsCreate),
                Ok(Vec::new())
            );
        })
        .await;
    }

    #[fuchsia::test]
    async fn set_get_procattr_clear_context() {
        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, _security_server| {
            // Set up the "exec" and "fscreate" Contexts with valid values.
            assert_eq!(
                set_procattr(current_task, ProcAttr::Exec, VALID_SECURITY_CONTEXT.into()),
                Ok(())
            );
            assert_eq!(
                set_procattr(
                    current_task,
                    ProcAttr::FsCreate,
                    DIFFERENT_VALID_SECURITY_CONTEXT.into()
                ),
                Ok(())
            );

            // Clear the "exec" context with a write containing a single null octet.
            assert_eq!(set_procattr(current_task, ProcAttr::Exec, b"\0"), Ok(()));
            assert_eq!(current_task.current_creds().security_state.exec_sid, None);

            // Clear the "fscreate" context with a write containing a single newline.
            assert_eq!(set_procattr(current_task, ProcAttr::FsCreate, b"\x0a"), Ok(()));
            assert_eq!(current_task.current_creds().security_state.fscreate_sid, None);
        })
        .await;
    }

    #[fuchsia::test]
    async fn set_get_procattr_setcurrent() {
        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, _security_server| {
            // Stash the initial "previous" context.
            let initial_previous =
                get_procattr(current_task, current_task, ProcAttr::Previous).unwrap();

            assert_eq!(
                // Dynamically transition to a valid new context.
                set_procattr(current_task, ProcAttr::Current, VALID_SECURITY_CONTEXT.into()),
                Ok(())
            );

            assert_eq!(
                // "current" should report the new context.
                get_procattr(current_task, current_task, ProcAttr::Current),
                Ok(VALID_SECURITY_CONTEXT_WITH_NUL.into())
            );

            assert_eq!(
                // "prev" should continue to report the original context.
                get_procattr(current_task, current_task, ProcAttr::Previous),
                Ok(initial_previous.clone())
            );

            assert_eq!(
                // Dynamically transition to a different valid context.
                set_procattr(
                    current_task,
                    ProcAttr::Current,
                    DIFFERENT_VALID_SECURITY_CONTEXT.into()
                ),
                Ok(())
            );

            assert_eq!(
                // "current" should report the different new context.
                get_procattr(current_task, current_task, ProcAttr::Current),
                Ok(DIFFERENT_VALID_SECURITY_CONTEXT_WITH_NUL.into())
            );

            assert_eq!(
                // "prev" should continue to report the original context.
                get_procattr(current_task, current_task, ProcAttr::Previous),
                Ok(initial_previous.clone())
            );
        })
        .await;
    }

    #[fuchsia::test]
    async fn set_get_procattr_selinux_permissive() {
        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, security_server| {
            security_server.set_enforcing(false);
            assert_eq!(
                get_procattr(current_task, &current_task.task, ProcAttr::Exec),
                Ok(Vec::new())
            );

            assert_eq!(
                // Test policy allows "kernel_t" tasks to set the "exec" context.
                set_procattr(current_task, ProcAttr::Exec, VALID_SECURITY_CONTEXT.into()),
                Ok(())
            );

            assert_eq!(
                // Test policy does not allow "kernel_t" tasks to set the "fscreate" context, but
                // in permissive mode the setting will be allowed.
                set_procattr(
                    current_task,
                    ProcAttr::FsCreate,
                    DIFFERENT_VALID_SECURITY_CONTEXT.into()
                ),
                Ok(())
            );

            assert_eq!(
                // Setting an invalid context should fail, even in permissive mode.
                set_procattr(current_task, ProcAttr::Exec, INVALID_SECURITY_CONTEXT.into()),
                error!(EINVAL)
            );

            assert_eq!(
                get_procattr(current_task, &current_task.task, ProcAttr::Exec),
                Ok(VALID_SECURITY_CONTEXT_WITH_NUL.into())
            );

            assert!(get_procattr(current_task, &current_task.task, ProcAttr::Current).is_ok());
        })
        .await;
    }

    #[fuchsia::test]
    async fn set_get_procattr_selinux_disabled() {
        spawn_kernel_and_run(async |current_task| {
            assert_eq!(
                set_procattr(&current_task, ProcAttr::Exec, VALID_SECURITY_CONTEXT.into()),
                error!(EINVAL)
            );

            assert_eq!(
                // Test policy allows "kernel_t" tasks to set the "exec" context.
                set_procattr(&current_task, ProcAttr::Exec, VALID_SECURITY_CONTEXT.into()),
                error!(EINVAL)
            );

            assert_eq!(
                // Test policy does not allow "kernel_t" tasks to set the "fscreate" context.
                set_procattr(&current_task, ProcAttr::FsCreate, VALID_SECURITY_CONTEXT.into()),
                error!(EINVAL)
            );

            assert_eq!(
                // Cannot set an invalid context.
                set_procattr(&current_task, ProcAttr::Exec, INVALID_SECURITY_CONTEXT.into()),
                error!(EINVAL)
            );

            assert_eq!(
                get_procattr(&current_task, &current_task.task, ProcAttr::Current),
                error!(EINVAL)
            );
        })
        .await;
    }

    #[fuchsia::test]
    async fn create_file_with_fscreate_sid() {
        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, security_server| {
            let sid =
                security_server.security_context_to_sid(VALID_SECURITY_CONTEXT.into()).unwrap();
            let source_node = &testing::create_test_file(current_task).entry.node;

            fs_node_setsecurity(
                &current_task,
                &source_node,
                XATTR_NAME_SELINUX.to_bytes().into(),
                VALID_SECURITY_CONTEXT.into(),
                XattrOp::Set,
            )
            .expect("set_xattr(security.selinux) failed");

            let mut creds = Credentials::clone(&current_task.current_creds());
            security::fs_node_copy_up(current_task, source_node, &source_node.fs(), &mut creds);
            let dir_entry = current_task
                .override_creds(creds.into(), || {
                    current_task
                        .fs()
                        .root()
                        .create_node(
                            &current_task,
                            "test_file2".into(),
                            FileMode::IFREG,
                            DeviceId::NONE,
                        )
                        .unwrap()
                })
                .entry;

            assert_eq!(get_cached_sid(&dir_entry.node), Some(sid));
        })
        .await;
    }
}
