// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::power::{
    PowerStateFile, PowerSyncOnSuspendFile, PowerWakeLockFile, PowerWakeUnlockFile,
    PowerWakeupCountFile,
};
use crate::task::Kernel;
use crate::vfs::pseudo::simple_directory::SimpleDirectoryMutator;
use crate::vfs::pseudo::simple_file::create_bytes_file_with_handler;
use starnix_uapi::file_mode::mode;

pub fn build_power_directory(kernel: &Kernel, dir: &SimpleDirectoryMutator) {
    dir.entry(
        "wakeup_count",
        PowerWakeupCountFile::new_node(kernel.suspend_resume_manager.clone()),
        mode!(IFREG, 0o644),
    );
    dir.entry("wake_lock", PowerWakeLockFile::new_node(), mode!(IFREG, 0o660));
    dir.entry("wake_unlock", PowerWakeUnlockFile::new_node(), mode!(IFREG, 0o660));
    dir.entry("state", PowerStateFile::new_node(), mode!(IFREG, 0o644));
    dir.entry("sync_on_suspend", PowerSyncOnSuspendFile::new_node(), mode!(IFREG, 0o644));
    dir.subdir("suspend_stats", 0o755, |dir| {
        let read_only_file_mode = mode!(IFREG, 0o444);
        dir.entry(
            "success",
            create_bytes_file_with_handler(kernel.weak_self.clone(), |kernel| {
                kernel.suspend_resume_manager.suspend_stats().success_count.to_string()
            }),
            read_only_file_mode,
        );
        dir.entry(
            "fail",
            create_bytes_file_with_handler(kernel.weak_self.clone(), |kernel| {
                kernel.suspend_resume_manager.suspend_stats().fail_count.to_string()
            }),
            read_only_file_mode,
        );
        dir.entry(
            "last_failed_dev",
            create_bytes_file_with_handler(kernel.weak_self.clone(), |kernel| {
                kernel.suspend_resume_manager.suspend_stats().last_failed_device.unwrap_or_default()
            }),
            read_only_file_mode,
        );
        dir.entry(
            "last_failed_errno",
            create_bytes_file_with_handler(kernel.weak_self.clone(), |kernel| {
                kernel
                    .suspend_resume_manager
                    .suspend_stats()
                    .last_failed_errno
                    .map(|e| format!("-{}", e.code.error_code()))
                    // This matches local linux behavior when no suspends have failed.
                    .unwrap_or_else(|| "0".to_string())
            }),
            read_only_file_mode,
        );
    });
}
