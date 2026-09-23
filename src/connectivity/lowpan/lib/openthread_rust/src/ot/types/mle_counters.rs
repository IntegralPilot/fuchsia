// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::prelude_internal::*;

/// This structure represents the MLE layer counters.
///
/// Functional equivalent of [`otsys::otMleCounters`](crate::otsys::otMleCounters).
#[derive(Debug, Default, Clone)]
#[repr(transparent)]
pub struct MleCounters(pub otMleCounters);

impl_ot_castable!(MleCounters, otMleCounters);

impl MleCounters {
    /// Number of times device entered OT_DEVICE_ROLE_DISABLED role.
    pub fn disabled_role(&self) -> u16 {
        self.0.mDisabledRole
    }

    /// Number of times device entered OT_DEVICE_ROLE_DETACHED role.
    pub fn detached_role(&self) -> u16 {
        self.0.mDetachedRole
    }

    /// Number of times device entered OT_DEVICE_ROLE_CHILD role.
    pub fn child_role(&self) -> u16 {
        self.0.mChildRole
    }

    /// Number of times device entered OT_DEVICE_ROLE_ROUTER role.
    pub fn router_role(&self) -> u16 {
        self.0.mRouterRole
    }

    /// Number of times device entered OT_DEVICE_ROLE_LEADER role.
    pub fn leader_role(&self) -> u16 {
        self.0.mLeaderRole
    }

    /// Number of attach attempts while device was detached.
    pub fn attach_attempts(&self) -> u16 {
        self.0.mAttachAttempts
    }

    /// Number of changes to partition ID.
    pub fn partition_id_changes(&self) -> u16 {
        self.0.mPartitionIdChanges
    }

    /// Number of attempts to attach to a better partition.
    pub fn better_partition_attach_attempts(&self) -> u16 {
        self.0.mBetterPartitionAttachAttempts
    }

    /// Number of attempts to attach to find a better parent (parent search).
    pub fn better_parent_attach_attempts(&self) -> u16 {
        self.0.mBetterParentAttachAttempts
    }

    /// Number of milliseconds device has been in OT_DEVICE_ROLE_DISABLED role.
    pub fn disabled_time(&self) -> u64 {
        self.0.mDisabledTime
    }

    /// Number of milliseconds device has been in OT_DEVICE_ROLE_DETACHED role.
    pub fn detached_time(&self) -> u64 {
        self.0.mDetachedTime
    }

    /// Number of milliseconds device has been in OT_DEVICE_ROLE_CHILD role.
    pub fn child_time(&self) -> u64 {
        self.0.mChildTime
    }

    /// Number of milliseconds device has been in OT_DEVICE_ROLE_ROUTER role.
    pub fn router_time(&self) -> u64 {
        self.0.mRouterTime
    }

    /// Number of milliseconds device has been in OT_DEVICE_ROLE_LEADER role.
    pub fn leader_time(&self) -> u64 {
        self.0.mLeaderTime
    }

    /// Number of milliseconds tracked by previous counters.
    pub fn tracked_time(&self) -> u64 {
        self.0.mTrackedTime
    }

    /// Number of times device changed its parent.
    pub fn parent_changes(&self) -> u16 {
        self.0.mParentChanges
    }
}
