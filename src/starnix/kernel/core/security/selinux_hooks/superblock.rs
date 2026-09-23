// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::fs_node::fs_node_init_with_dentry;
use super::{
    FileSystemLabel, FileSystemState, FsNodeSidAndClass, check_permission, current_task_state,
    fs_node_effective_sid_and_class,
};

use crate::task::CurrentTask;
use crate::vfs::fs_args::MountParams;
use crate::vfs::{FileSystem, FileSystemHandle, FileSystemOps, FsStr, Mount, NamespaceNode};
use linux_uapi::AUDIT_SELINUX_ERR;
use selinux::permission_check::PermissionCheck;
use selinux::{
    CommonFilePermission, FileSystemMountOptions, FileSystemPermission, ForClass, FsNodeClass,
    SecurityId, SecurityServer,
};
use starnix_logging::{log_debug, track_stub};
use starnix_uapi::error;
use starnix_uapi::errors::Errno;
use starnix_uapi::mount_flags::MountFlags;
use starnix_uapi::unmount_flags::UnmountFlags;
use std::fmt::Formatter;

/// Returns the [`SecurityId`] of `fs`.
/// If the filesystem is not labeled, returns EPERM.
fn fs_sid(fs: &FileSystem) -> Result<SecurityId, Errno> {
    let Some(fs_label) = fs.security_state.state.label() else {
        return error!(EPERM);
    };
    Ok(fs_label.sid)
}

/// Returns security state to associate with a filesystem based on the supplied mount options.
pub(in crate::security) fn file_system_init_security(
    mount_options: &FileSystemMountOptions,
    ops: &dyn FileSystemOps,
) -> Result<FileSystemState, Errno> {
    Ok(FileSystemState::new(mount_options.clone(), ops))
}

/// Resolves the labeling scheme and arguments for the `file_system`, based on the loaded policy.
pub(in crate::security) fn file_system_resolve_security(
    security_server: &SecurityServer,
    current_task: &CurrentTask,
    file_system: &FileSystemHandle,
) -> Result<(), Errno> {
    // TODO: https://fxbug.dev/334094811 - Determine how failures, e.g. mount options containing
    // Security Context values that are not valid in the loaded policy.
    let fs_state = &file_system.security_state.state;

    let mut root_node_to_init = Option::default();

    if fs_state.label.get().is_none() {
        let requested_label = label_from_mount_options_and_name(
            security_server,
            current_task,
            &fs_state.mount_options,
            file_system.name(),
        )?;

        let default_label = label_from_mount_options_and_name(
            security_server,
            current_task,
            &FileSystemMountOptions::default(),
            file_system.name(),
        )?;

        if requested_label.sid != default_label.sid {
            let permission_check = super::build_permission_check(current_task, security_server);
            let source_sid = current_task_state(current_task).current_sid;
            let audit_context = [current_task.into(), file_system.as_ref().into()];

            check_permission(
                &permission_check,
                current_task,
                source_sid,
                default_label.sid,
                FileSystemPermission::RelabelFrom,
                (&audit_context).into(),
            )?;

            check_permission(
                &permission_check,
                current_task,
                source_sid,
                requested_label.sid,
                FileSystemPermission::RelabelTo,
                (&audit_context).into(),
            )?;
        }

        fs_state.label.get_or_init(|| {
            // This caller is initializing the file system, so note the root node to be initialized.
            root_node_to_init = file_system.maybe_root();
            requested_label
        });
    }

    let pending_entries = {
        let pending = &mut *file_system.security_state.state.pending_entries.lock();
        std::mem::take(pending)
    };

    // This step will be performed only when the file system label is first resolved.
    if let Some(root_dir_entry) = root_node_to_init {
        fs_node_init_with_dentry(
            security_server,
            current_task,
            root_dir_entry,
            /* read_xaddr = */
            true,
        )?;
    }

    // Label the `FsNode`s for any `pending_entries`.
    let labeled_entries = pending_entries.len();
    for dir_entry in pending_entries {
        if let Some(dir_entry) = dir_entry.0.upgrade() {
            fs_node_init_with_dentry(
                security_server,
                current_task,
                &dir_entry,
                /* read_xaddr = */
                true,
            )
            .unwrap_or_else(|_| panic!("Failed to resolve FsNode label"));
        }
    }
    log_debug!("Labeled {} entries in {} FileSystem", labeled_entries, file_system.name());

    Ok(())
}

/// Returns the security label to be applied to a file system with the name `fs_name`
/// that is to be mounted with `mount_options`.
pub(super) fn label_from_mount_options_and_name(
    security_server: &SecurityServer,
    current_task: &CurrentTask,
    mount_options: &FileSystemMountOptions,
    fs_name: &'static FsStr,
) -> Result<FileSystemLabel, Errno> {
    // TODO: https://fxbug.dev/361297862 - Replace this workaround with more
    // general handling of these special Fuchsia filesystems.
    let effective_name: &FsStr =
        if *fs_name == "remotefs" || *fs_name == "remote_bundle" || *fs_name == "remotevol" {
            track_stub!(
                TODO("https://fxbug.dev/361297862"),
                "Applying ext4 labeling configuration to remote filesystems"
            );
            "ext4".into()
        } else {
            fs_name
        };
    match security_server.resolve_fs_label(effective_name.into(), mount_options) {
        Err(e) => {
            let audit_logger = current_task.kernel().audit_logger();
            audit_logger.audit_log(AUDIT_SELINUX_ERR as u16, || {
                format!("Failed to initialize {fs_name} (as {effective_name}): {e:?}")
            });
            error!(EINVAL)
        }
        Ok(value) => Ok(value),
    }
}

/// Consumes the SELinux mount options from the supplied `MountParams` and returns the security
/// mount options for the given `MountParams`.
pub(in crate::security) fn sb_eat_lsm_opts(
    mount_params: &mut MountParams,
) -> Result<FileSystemMountOptions, Errno> {
    let context = mount_params.remove(FsStr::new(b"context"));
    let def_context = mount_params.remove(FsStr::new(b"defcontext"));
    let fs_context = mount_params.remove(FsStr::new(b"fscontext"));
    let root_context = mount_params.remove(FsStr::new(b"rootcontext"));

    // If a "context" is specified then it is used for all nodes in the filesystem, so the other
    // security context options would not be meaningful to combine with it, except "fscontext".
    if context.is_some() && (def_context.is_some() || root_context.is_some()) {
        return error!(EINVAL);
    }
    Ok(FileSystemMountOptions {
        context: context.map(Into::into),
        def_context: def_context.map(Into::into),
        fs_context: fs_context.map(Into::into),
        root_context: root_context.map(Into::into),
    })
}

/// Checks if `current_task` has the permission to mount `fs`.
pub(in crate::security) fn sb_kern_mount(
    permission_check: &PermissionCheck<'_>,
    current_task: &CurrentTask,
    fs: &FileSystem,
) -> Result<(), Errno> {
    let audit_context = [current_task.into(), fs.into()];
    let source_sid = current_task_state(current_task).current_sid;
    let target_sid = fs_sid(fs)?;
    check_permission(
        permission_check,
        current_task,
        source_sid,
        target_sid,
        FileSystemPermission::Mount,
        (&audit_context).into(),
    )
}

/// Checks if `current_task` has the permission to mount at `path` with the mounting flags `flags`.
pub(in crate::security) fn sb_mount(
    permission_check: &PermissionCheck<'_>,
    current_task: &CurrentTask,
    path: &NamespaceNode,
    flags: MountFlags,
) -> Result<(), Errno> {
    let source_sid = current_task_state(current_task).current_sid;
    if flags.contains(MountFlags::REMOUNT) {
        let mount = path.mount_if_root()?;
        let fs = mount.root().entry.node.fs();
        let target_sid = fs_sid(&fs)?;
        let audit_context = [current_task.into(), fs.as_ref().into()];
        check_permission(
            permission_check,
            current_task,
            source_sid,
            target_sid,
            FileSystemPermission::Remount,
            (&audit_context).into(),
        )
    } else {
        let node = path.entry.node.as_ref().as_ref();
        let FsNodeSidAndClass { sid: target_sid, class: target_class } =
            fs_node_effective_sid_and_class(node);
        let FsNodeClass::File(target_class) = target_class else {
            panic!("sb_mount on non-file-like class")
        };
        let audit_context = [current_task.into(), node.into()];
        check_permission(
            permission_check,
            current_task,
            source_sid,
            target_sid,
            CommonFilePermission::MountOn.for_class(target_class),
            (&audit_context).into(),
        )
    }
}

/// Checks that `mount` is getting remounted with the same security state as before.
pub(in crate::security) fn sb_remount(
    _security_server: &SecurityServer,
    mount: &Mount,
    new_mount_options: FileSystemMountOptions,
) -> Result<(), Errno> {
    if mount.security_state().state.mount_options != new_mount_options {
        return error!(EACCES);
    }
    Ok(())
}

/// Internal type that allows the `FileSystemMountOptions` to be `Display`ed as comma-separated
/// options suitable for inclusion in "/proc/mounts" or "/proc/self/mountinfo".
///
/// Because this is a `Display` implementation and only ever used to append to a non-empty list of
/// stringified mount options, the implementation produces UTF-8 output and includes a leading
/// comma, e.g: ",context=foo,root_context=bar,seclabel"
struct DisplayFileSystemMountOptions<'a>(&'a FileSystemState);

impl<'a> std::fmt::Display for DisplayFileSystemMountOptions<'a> {
    fn fmt(&self, buf: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        let mut write_option = |name, option: Option<&Vec<u8>>| -> Result<(), std::fmt::Error> {
            let Some(value) = option else {
                return Ok(());
            };
            write!(buf, ",{name}={}", FsStr::new(value))
        };

        // Mounter-supplied options are serializable without SELinux being enabled or configured.
        let options = &self.0.mount_options;
        write_option("context", options.context.as_ref())?;
        write_option("fscontext", options.fs_context.as_ref())?;
        write_option("defcontext", options.def_context.as_ref())?;
        write_option("rootcontext", options.root_context.as_ref())?;

        // `supports_relabel()` defaults to false if SELinux is not enabled.
        if self.0.supports_relabel() {
            write!(buf, ",seclabel")?;
        }

        Ok(())
    }
}

/// Writes the LSM mount options of `mount` to `buf`.
pub(in crate::security) fn sb_show_options<'a>(
    fs: &'a FileSystem,
) -> Result<impl std::fmt::Display + 'a, Errno> {
    Ok(DisplayFileSystemMountOptions(&fs.security_state.state))
}

/// Checks if `current_task` has the permission to get information on `fs`.
pub(in crate::security) fn sb_statfs(
    permission_check: &PermissionCheck<'_>,
    current_task: &CurrentTask,
    fs: &FileSystem,
) -> Result<(), Errno> {
    let audit_context = [current_task.into(), fs.into()];
    let source_sid = current_task_state(current_task).current_sid;
    let target_sid = fs_sid(fs)?;
    check_permission(
        permission_check,
        current_task,
        source_sid,
        target_sid,
        FileSystemPermission::GetAttr,
        (&audit_context).into(),
    )
}

/// Checks if `current_task` has the permission to unmount the filesystem mounted on
/// `node` using the unmount flags `_flags`.
pub(in crate::security) fn sb_umount(
    permission_check: &PermissionCheck<'_>,
    current_task: &CurrentTask,
    node: &NamespaceNode,
    _flags: UnmountFlags,
) -> Result<(), Errno> {
    let source_sid = current_task_state(current_task).current_sid;
    let mount = node.mount_if_root()?;
    let fs = mount.root().entry.node.fs();
    let target_sid = fs_sid(&fs)?;
    let audit_context = [current_task.into(), fs.as_ref().into()];
    check_permission(
        permission_check,
        current_task,
        source_sid,
        target_sid,
        FileSystemPermission::Unmount,
        (&audit_context).into(),
    )
}
