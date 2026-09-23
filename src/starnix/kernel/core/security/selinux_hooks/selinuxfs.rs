// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::{
    PolicyCapSupport, build_permission_check, check_permission, current_task_state,
    policycap_support, set_cached_sid, superblock,
};

use crate::task::CurrentTask;
use crate::vfs::FileHandle;
use selinux::{InitialSid, PolicyCap, SecurityPermission, SecurityServer};
use starnix_logging::{log_info, log_warn};
use starnix_uapi::errors::Errno;
use std::sync::atomic::Ordering;
use strum::VariantArray as _;

pub(in crate::security) fn selinuxfs_init_null(
    current_task: &CurrentTask,
    null_file_handle: &FileHandle,
) {
    // Apply the "devnull" initial SID to the node.
    set_cached_sid(null_file_handle.node(), InitialSid::Devnull.into());

    let kernel_state = current_task
        .kernel()
        .security_state
        .state
        .as_ref()
        .expect("selinux kernel state exists when selinux is enabled");

    kernel_state
        .selinuxfs_null
        .set(null_file_handle.clone())
        .expect("selinuxfs null file initialized at most once");
}

/// Called by the "selinuxfs" when a policy has been successfully loaded, to allow policy-dependent
/// initialization to be completed.
pub(in crate::security) fn selinuxfs_policy_loaded(current_task: &CurrentTask) {
    let kernel_state = current_task.kernel().security_state.state.as_ref().unwrap();
    let security_server = &kernel_state.server;
    assert!(security_server.has_policy(), "selinuxfs_policy_loaded() without policy");

    // Compare the policy capabilities against this kernel's support level, and emit warnings for
    // each mismatch.
    for capability in PolicyCap::VARIANTS {
        let is_enabled = security_server.is_policycap_enabled(*capability);
        log_info!("SELinux:  policy capability {}={}", capability.name(), is_enabled as u8);
        match policycap_support(*capability) {
            PolicyCapSupport::AlwaysOn(bug) => {
                if !is_enabled {
                    log_warn!(
                        "SELinux:  policy capability {} cannot be disabled bug={bug}",
                        capability.name()
                    );
                }
            }
            PolicyCapSupport::AlwaysOff(bug) => {
                if is_enabled {
                    log_warn!(
                        "SELinux:  policy capability {} is not supported bug={bug}",
                        capability.name()
                    );
                }
            }
            PolicyCapSupport::Configurable | PolicyCapSupport::NotImplemented => (),
        }
    }

    // Invoke `file_system_resolve_security()` on all pre-existing `FileSystem`s.
    // No new `FileSystem`s should be added to `pending_file_systems` after policy load.
    let pending_file_systems = std::mem::take(&mut *kernel_state.pending_file_systems.lock());
    for file_system in pending_file_systems {
        if let Some(file_system) = file_system.0.upgrade() {
            superblock::file_system_resolve_security(security_server, current_task, &file_system)
                .unwrap_or_else(|e| {
                    panic!("Failed to resolve {} FileSystem label: {:?}", file_system.name(), e)
                });
        }
    }

    kernel_state.has_policy.store(true, Ordering::Release);
}

/// Used by the "selinuxfs" module to perform checks on SELinux API file accesses.
pub(in crate::security) fn selinuxfs_check_access(
    security_server: &SecurityServer,
    current_task: &CurrentTask,
    permission: SecurityPermission,
) -> Result<(), Errno> {
    let source_sid = current_task_state(current_task).current_sid;
    let target_sid = InitialSid::Security.into();
    let permission_check = build_permission_check(current_task, security_server);
    check_permission(
        &permission_check,
        current_task,
        source_sid,
        target_sid,
        permission,
        current_task.into(),
    )
}
