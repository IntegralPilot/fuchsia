// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::power::WakeupSourceOrigin;
use crate::security;
use crate::task::CurrentTask;
use crate::vfs::FsNodeOps;
use crate::vfs::pseudo::simple_file::{BytesFile, BytesFileOps};
use itertools::Itertools;
use starnix_uapi::auth::CAP_BLOCK_SUSPEND;
use starnix_uapi::errors::Errno;
use starnix_uapi::{errno, error};
use std::borrow::Cow;

pub struct PowerWakeLockFile;

impl PowerWakeLockFile {
    pub fn new_node() -> impl FsNodeOps {
        BytesFile::new_node(Self {})
    }
}

impl BytesFileOps for PowerWakeLockFile {
    /// Writing a string activates a "wakeup source" preventing the system from
    /// entering a low-power state.
    ///
    /// 1. Simple string (no whitespace): Activates or creates a wakeup source with that name.
    /// 2. String with whitespace: The first part (before the whitespace) is the wakeup source name.
    ///    The second part is a timeout in nanoseconds, after which the wakeup source is
    ///    automatically deactivated.
    fn write(&self, current_task: &CurrentTask, data: Vec<u8>) -> Result<(), Errno> {
        security::check_task_capable(current_task, CAP_BLOCK_SUSPEND)?;
        let lock_str = std::str::from_utf8(&data).map_err(|_| errno!(EINVAL))?;
        let clean_str = lock_str.trim_end_matches('\n');
        let mut clean_str_split = clean_str.split(' ');
        let Some(clean_lock_str) = clean_str_split.next() else {
            return error!(EINVAL);
        };

        // Check if there is a timeout.
        let target_monotonic = match clean_str_split.next() {
            Some(timeout_str) => Some(
                zx::MonotonicInstant::get() // now
                    + zx::MonotonicDuration::from_nanos(
                        timeout_str
                            .parse()
                            .map_err(|_| errno!(EINVAL, "Failed to parse the timeout string"))?,
                    ),
            ),
            None => None,
        };

        current_task
            .kernel()
            .suspend_resume_manager
            .activate_wakeup_source(WakeupSourceOrigin::WakeLock(clean_lock_str.to_owned()));

        // Set a timer to disable the wake lock when expired.
        if let Some(target_monotonic) = target_monotonic {
            let kernel_ref = current_task.kernel().clone();
            let clean_lock_string = clean_lock_str.to_string();
            current_task.kernel().kthreads.spawn_future(
                move || async move {
                    fuchsia_async::Timer::new(target_monotonic).await;
                    kernel_ref
                        .suspend_resume_manager
                        .timeout_wakeup_source(&WakeupSourceOrigin::WakeLock(clean_lock_string));
                },
                "wake-lock-timeout",
            );
        }

        Ok(())
    }

    fn read(&self, current_task: &CurrentTask) -> Result<Cow<'_, [u8]>, Errno> {
        let wake_locks = current_task.kernel().suspend_resume_manager.lock().active_wake_locks();
        let content = wake_locks.iter().map(|o| o.to_string()).join(" ") + "\n";
        Ok(content.as_bytes().to_owned().into())
    }
}

pub struct PowerWakeUnlockFile;

impl PowerWakeUnlockFile {
    pub fn new_node() -> impl FsNodeOps {
        BytesFile::new_node(Self {})
    }
}

impl BytesFileOps for PowerWakeUnlockFile {
    /// Writing a string to this file deactivates the wakeup source with that name.
    fn write(&self, current_task: &CurrentTask, data: Vec<u8>) -> Result<(), Errno> {
        security::check_task_capable(current_task, CAP_BLOCK_SUSPEND)?;
        let lock_str = std::str::from_utf8(&data).map_err(|_| errno!(EINVAL))?;
        let clean_lock_str = lock_str.trim_end_matches('\n').to_string();
        if !current_task
            .kernel()
            .suspend_resume_manager
            .deactivate_wakeup_source(&WakeupSourceOrigin::WakeLock(clean_lock_str))
        {
            return error!(EPERM);
        }
        Ok(())
    }

    /// Returns a space-separated list of inactive wakeup source names previously created
    /// via `PowerWakeLockFile`.
    fn read(&self, current_task: &CurrentTask) -> Result<Cow<'_, [u8]>, Errno> {
        let wake_locks = current_task.kernel().suspend_resume_manager.lock().inactive_wake_locks();
        let content = wake_locks.iter().map(|o| o.to_string()).join(" ") + "\n";
        Ok(content.as_bytes().to_owned().into())
    }
}
