// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use starnix_uapi::errors::Errno;

/// Suspend statistics collection.
///
/// This is a Starnix version of `fuchsia.power.suspend.SuspendStats`, plus
/// additional observability context.
#[derive(Debug, Default, Clone, Eq, PartialEq)]
pub struct SuspendStats {
    /// The number of times the device has successfully suspended.
    pub success_count: u64,
    /// The number of times the device has failed to suspend.
    pub fail_count: u64,
    /// The error code logged after the last failed suspend attempt.
    pub last_failed_errno: Option<Errno>,
    /// The name of the device that last failed suspend.
    pub last_failed_device: Option<String>,
    /// Last reason for resume.
    pub last_resume_reason: Option<String>,
    /// The amount of time spent in the previous suspend state.
    /// May not be available on all platforms.
    pub last_time_in_sleep: zx::BootDuration,
    /// The amount of time spent performing suspend and resume operations for
    /// the previous suspend state.
    /// Suspend and resume operations are those actions taken by the platform in
    /// order to enter and exit, respectively, a suspended state.
    pub last_time_in_suspend_operations: zx::BootDuration,
}
