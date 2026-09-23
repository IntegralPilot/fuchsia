// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// TODO(https://github.com/rust-lang/rust/issues/39371): remove
#![allow(non_upper_case_globals)]

use super::bpf::{check_bpf_map_access, check_bpf_prog_access};
use super::{
    FileObjectOpenState, FileObjectState, FsNodeSidAndClass, NO_PERMISSIONS, PermissionFlags,
    build_permission_check, check_permission, current_task_state, fs_node_effective_sid_and_class,
    has_file_ioctl_permission, has_file_permissions, is_internal_operation, permissions_from_flags,
};
use crate::bpf::fs::BpfHandle;
use crate::mm::{Mapping, MappingNameRef, MappingOptions, ProtectionFlags};
use crate::security::selinux_hooks::{
    ProcessPermission, check_self_permission, has_fs_node_permissions,
};
use crate::task::CurrentTask;
use crate::vfs::{FileHandle, FileObject, FsNode, canonicalize_ioctl_request};
use linux_uapi::{
    F_GETFL, F_GETSIG, F_SETFL, F_SETOWN, F_SETOWN_EX, F_SETSIG, FIBMAP, FIGETBSZ, FIOASYNC,
    FIOCLEX, FIONBIO, FIONCLEX, FIONREAD, FS_IOC_GETFLAGS, FS_IOC_GETVERSION, FS_IOC_SETFLAGS,
    FS_IOC_SETVERSION,
};
use selinux::{
    CommonFilePermission, CommonFsNodePermission, ForClass, FsNodeClass, PolicyCap, SecurityId,
    SecurityServer,
};
use starnix_uapi::errors::Errno;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::user_address::UserAddress;
use std::ops::Range;
use std::sync::OnceLock;

/// Returns the security state for a new file object created by `current_task`.
pub(in crate::security) fn file_alloc_security(current_task: &CurrentTask) -> FileObjectState {
    FileObjectState {
        sid: current_task_state(current_task).current_sid,
        open_state: OnceLock::new(),
    }
}

/// Checks whether the `current_task` has the specified `permission_flags` to the `file`.
pub(in crate::security) fn file_permission(
    security_server: &SecurityServer,
    current_task: &CurrentTask,
    file: &FileObject,
    mut permission_flags: PermissionFlags,
) -> Result<(), Errno> {
    let current_sid = current_task_state(current_task).current_sid;
    let FsNodeSidAndClass { class: file_class, sid: node_sid } =
        fs_node_effective_sid_and_class(&file.name.entry.node);

    // Fast-path: If the caller SID, `FsNode` SID, and policy sequence number all match the values
    // cached by `file_open()` then access checks can be skipped, because `file_open()` has already
    // (re-)verified the caller's access to the `FsNode`.
    if let Some(open_state) = file.security_state.state.open_state.get() {
        if file.security_state.state.sid == current_sid
            && open_state.node_sid == node_sid
            && open_state.policy_seqno == security_server.policy_seqno()
        {
            return Ok(());
        }
    }

    // `WRITE` permission checks must distinguish between append-only and full write permissions.
    if permission_flags.contains(PermissionFlags::WRITE) && file.flags().contains(OpenFlags::APPEND)
    {
        permission_flags |= PermissionFlags::APPEND;
    }

    has_file_permissions(
        &build_permission_check(current_task, security_server),
        current_task,
        current_sid,
        file,
        &permissions_from_flags(permission_flags, file_class),
        current_task.into(),
    )
}

/// Checks whether `current_task` is allowed to open `file`.
pub(in crate::security) fn file_open(
    security_server: &SecurityServer,
    current_task: &CurrentTask,
    file: &FileObject,
) -> Result<(), Errno> {
    if is_internal_operation(current_task) {
        return Ok(());
    }

    // Fast-path: To allow the fast-path optimization in `file_permission()` the caller's
    // access to the `file`'s underlying `FsNode` must be verified and the `FsNode` SID and policy
    // sequence number used for that verification stored in the `FileObjectState`.
    //
    // The `FsNode` SID and policy sequence number are captured before validating access, to
    // safeguard against concurrent re-labeling or policy re-loading:
    // - `FsNode` access is checked manually, rather than delegating to `has_fs_node_permission()`,
    //   to ensure that the cached SID matches that for which access was validated.
    // - If the policy is re-loaded then subsequent `file_permission()` calls will observe the later
    //   sequence number and fail-safe by re-doing the check.
    let policy_version = security_server.policy_seqno();
    let FsNodeSidAndClass { sid: node_sid, class } = fs_node_effective_sid_and_class(file.node());

    let current_sid = current_task_state(current_task).current_sid;
    // `file_alloc_security()` ws called by this task immediately before `file_open()`, so the
    // `FileObject` must be labeled with this task's SID.
    assert_eq!(current_sid, file.security_state.state.sid);

    let permission_check = build_permission_check(current_task, security_server);
    let audit_context = [current_task.into(), file.into()];

    // Fast-path: Verify that the currently has the required `FsNode` access, to allow the fast-path
    // in `file_permission()` to safely skip subsequent re-validation.
    let mut open_permissions = permissions_from_flags(file.flags().into(), class);
    if security_server.is_policycap_enabled(PolicyCap::OpenPerms) {
        if let FsNodeClass::File(file_class) = class {
            open_permissions.push(CommonFilePermission::Open.for_class(file_class));
        }
    }
    for permission in open_permissions {
        check_permission(
            &permission_check,
            current_task,
            current_sid,
            node_sid,
            permission,
            (&audit_context).into(),
        )?;
    }

    // Fast-path: Cache the `FsNode` SID and policy sequence number for which access was validated.
    let open_state = FileObjectOpenState { node_sid, policy_seqno: policy_version };
    file.security_state.state.open_state.set(open_state).expect("file_open() called at most once");

    Ok(())
}

/// Returns whether the `current_task` can receive `file` via a socket IPC.
pub(in crate::security) fn file_receive(
    security_server: &SecurityServer,
    current_task: &CurrentTask,
    receiving_sid: SecurityId,
    file: &FileObject,
) -> Result<(), Errno> {
    let permission_check = build_permission_check(current_task, security_server);
    let fs_node_class = fs_node_effective_sid_and_class(file.node()).class;
    let permission_flags = file.flags().into();

    // BPF resources are wrapped into file descriptors for interaction with userspace,
    // but have a distinct set of permissions associated with the underlying objects rather
    // than on the `FsNode`.
    if let Some(bpf_handle) = file.downcast_file::<BpfHandle>() {
        has_file_permissions(
            &permission_check,
            current_task,
            receiving_sid,
            file,
            NO_PERMISSIONS,
            current_task.into(),
        )?;
        match *bpf_handle {
            BpfHandle::Map(map) => check_bpf_map_access(
                security_server,
                current_task,
                receiving_sid,
                &map.security_state,
                permission_flags,
            )?,
            BpfHandle::Program(prog) => check_bpf_prog_access(
                security_server,
                current_task,
                receiving_sid,
                &prog.security_state,
            )?,
            _ => {}
        }
        return Ok(());
    }

    has_file_permissions(
        &permission_check,
        current_task,
        receiving_sid,
        file,
        &permissions_from_flags(permission_flags, fs_node_class),
        current_task.into(),
    )
}

/// Returns whether `current_task` can issue an ioctl to `file`.
pub(in crate::security) fn check_file_ioctl_access(
    security_server: &SecurityServer,
    current_task: &CurrentTask,
    file: &FileObject,
    request: u32,
) -> Result<(), Errno> {
    let permission_check = build_permission_check(current_task, security_server);
    let subject_sid = current_task_state(current_task).current_sid;
    match canonicalize_ioctl_request(current_task, request) {
        FIBMAP | FIONREAD | FIGETBSZ | FS_IOC_GETFLAGS | FS_IOC_GETVERSION => has_file_permissions(
            &permission_check,
            current_task,
            subject_sid,
            file,
            &[CommonFsNodePermission::GetAttr],
            current_task.into(),
        ),
        FS_IOC_SETFLAGS | FS_IOC_SETVERSION => has_file_permissions(
            &permission_check,
            current_task,
            subject_sid,
            file,
            &[CommonFsNodePermission::SetAttr],
            current_task.into(),
        ),
        FIONBIO | FIOASYNC => has_file_permissions(
            &permission_check,
            current_task,
            subject_sid,
            file,
            NO_PERMISSIONS,
            current_task.into(),
        ),
        FIOCLEX | FIONCLEX if security_server.is_policycap_enabled(PolicyCap::IoctlSkipCloexec) => {
            return Ok(());
        }
        _ => {
            // The ioctl command is the 2 least-significant bytes of `request`.
            let ioctl = request as u16;
            has_file_ioctl_permission(
                &permission_check,
                current_task,
                subject_sid,
                file,
                ioctl,
                current_task.into(),
            )
        }
    }
}

/// Returns whether `current_task` can perform a lock operation on the given `file`.
pub(in crate::security) fn check_file_lock_access(
    security_server: &SecurityServer,
    current_task: &CurrentTask,
    file: &FileObject,
) -> Result<(), Errno> {
    let permission_check = build_permission_check(current_task, security_server);
    let subject_sid = current_task_state(current_task).current_sid;
    has_file_permissions(
        &permission_check,
        current_task,
        subject_sid,
        file,
        &[CommonFsNodePermission::Lock],
        current_task.into(),
    )
}

/// This hook is called by the `fcntl` syscall. Returns whether `current_task` can perform
/// `fcntl_cmd` on the given file.
pub(in crate::security) fn check_file_fcntl_access(
    security_server: &SecurityServer,
    current_task: &CurrentTask,
    file: &FileObject,
    fcntl_cmd: u32,
    fcntl_arg: u64,
) -> Result<(), Errno> {
    let permission_check = build_permission_check(current_task, security_server);
    let subject_sid = current_task_state(current_task).current_sid;

    match fcntl_cmd {
        F_SETFL
            if file.flags().contains(OpenFlags::APPEND)
                && !OpenFlags::from_bits_truncate(fcntl_arg as u32).contains(OpenFlags::APPEND) =>
        {
            // If `O_APPEND` is being cleared then check the "write" permission.
            // Although the flag only affects files opened with the writable bit
            // set, the SELinux Test Suite validates that it is not possible to
            // clear the `O_APPEND` bit from an `O_RDONLY` file.
            has_fs_node_permissions(
                &build_permission_check(current_task, security_server),
                current_task,
                subject_sid,
                file.node(),
                &[CommonFsNodePermission::Write],
                current_task.into(),
            )
        }
        F_SETFL | F_GETFL | F_SETSIG | F_GETSIG | F_SETOWN | F_SETOWN_EX => has_file_permissions(
            &permission_check,
            current_task,
            subject_sid,
            file,
            NO_PERMISSIONS,
            current_task.into(),
        ),

        _ => Ok(()),
    }
}

/// Checks if the requested protection changes `prot` can be applied to `mapping`.
pub(in crate::security) fn file_mprotect(
    security_server: &SecurityServer,
    current_task: &CurrentTask,
    mapping_range: &Range<UserAddress>,
    mapping: &Mapping,
    prot: ProtectionFlags,
) -> Result<(), Errno> {
    if !mapping.can_exec() && prot.contains(ProtectionFlags::EXEC) {
        let permission = match mapping.name() {
            MappingNameRef::Heap => Some(ProcessPermission::ExecHeap),
            MappingNameRef::Stack => {
                // `execstack` is checked when making executable the stack of the initial thread.
                Some(ProcessPermission::ExecStack)
            }
            MappingNameRef::None
            | MappingNameRef::Vdso
            | MappingNameRef::Vvar
            | MappingNameRef::Vma(_)
            | MappingNameRef::File(_)
            | MappingNameRef::AioContext(_)
            | MappingNameRef::Ashmem(_) => {
                // TODO(b/409256444): Check `execmod`

                // `execstack` is checked when making executable a mapping that contains
                // the stackpointer.
                let stack_pointer_register =
                    current_task.thread_state.registers.stack_pointer_register();
                if mapping_range.contains(&UserAddress::const_from(stack_pointer_register)) {
                    Some(ProcessPermission::ExecStack)
                } else {
                    None
                }
            }
        };
        if let Some(permission) = permission {
            let subject_sid = current_task_state(current_task).current_sid;
            check_self_permission(
                &build_permission_check(current_task, security_server),
                current_task,
                subject_sid,
                permission,
                current_task.into(),
            )?;
        }
    }
    let fs_node = match mapping.name() {
        MappingNameRef::File(file) => {
            let node: &FsNode = file.node();
            Some(node)
        }
        _ => None,
    };
    let mapping_options = mapping.flags().options();
    file_map_prot_check(security_server, current_task, fs_node, prot, mapping_options)?;
    Ok(())
}

/// Checks if `current_task` can mmap `file` or anonymous memory with the given `protection_flags`
/// and `mapping_options`.
pub(in crate::security) fn mmap_file(
    security_server: &SecurityServer,
    current_task: &CurrentTask,
    file: Option<&FileHandle>,
    protection_flags: ProtectionFlags,
    mapping_options: MappingOptions,
) -> Result<(), Errno> {
    if let Some(file) = file {
        let current_sid = current_task_state(current_task).current_sid;
        has_file_permissions(
            &build_permission_check(current_task, security_server),
            &current_task,
            current_sid,
            file,
            &[CommonFsNodePermission::Map],
            current_task.into(),
        )?;
    }
    let fs_node = file.map(|file| -> &FsNode { file.node() });
    file_map_prot_check(security_server, current_task, fs_node, protection_flags, mapping_options)
}

/// Checks if `current_task` has the permission to set `prot` on a mapping
/// described by `mapping_options` potentially associated with `fs_node`.
fn file_map_prot_check(
    security_server: &SecurityServer,
    current_task: &CurrentTask,
    fs_node: Option<&FsNode>,
    prot: ProtectionFlags,
    mapping_options: MappingOptions,
) -> Result<(), Errno> {
    // This function checks:
    // * `execmem` when mapping with `PROT_EXEC` an anonymous mapping.
    // * `execmem` when mapping with `PROT_EXEC` a writable private mapping.
    // * `read` when mapping a file.
    // * `write` when mapping a shared file with `PROT_WRITE`.
    // * `execute` when mapping a file with `PROT_EXEC`.
    if prot.contains(ProtectionFlags::EXEC) {
        let anonymous_mapping = mapping_options.contains(MappingOptions::ANONYMOUS);
        let private_writable_mapping = !mapping_options.contains(MappingOptions::SHARED)
            && prot.contains(ProtectionFlags::WRITE);
        if anonymous_mapping || private_writable_mapping {
            let current_sid = current_task_state(current_task).current_sid;
            check_permission(
                &build_permission_check(current_task, security_server),
                current_task,
                current_sid,
                current_sid,
                ProcessPermission::ExecMem,
                current_task.into(),
            )?;
        }
    }

    if let Some(fs_node) = fs_node {
        let node_class = fs_node_effective_sid_and_class(fs_node).class;
        let flags = {
            let mut flags: PermissionFlags = prot.into();
            // After mapping a file into memory you can read its content, so
            // the read permission needs to be checked.
            flags |= PermissionFlags::READ;
            if !mapping_options.contains(MappingOptions::SHARED) {
                // When mapping a file privately, the writes to the mapping
                // aren't propagated to the file, so there's no need to
                // check for the write permission.
                flags.remove(PermissionFlags::WRITE);
            }
            flags
        };
        let permissions = permissions_from_flags(flags, node_class);
        let current_sid = current_task_state(current_task).current_sid;
        has_fs_node_permissions(
            &build_permission_check(current_task, security_server),
            current_task,
            current_sid,
            fs_node,
            &permissions,
            current_task.into(),
        )?;
    }
    Ok(())
}
