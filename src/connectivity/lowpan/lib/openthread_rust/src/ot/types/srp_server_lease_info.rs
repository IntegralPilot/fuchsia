// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::prelude_internal::*;

/// This structure represents the SRP Server Lease Info.
///
/// Functional equivalent of [`otsys::otSrpServerLeaseInfo`](crate::otsys::otSrpServerLeaseInfo).
#[derive(Debug, Default, Clone)]
#[repr(transparent)]
pub struct SrpServerLeaseInfo(pub otSrpServerLeaseInfo);

impl_ot_castable!(SrpServerLeaseInfo, otSrpServerLeaseInfo);

impl SrpServerLeaseInfo {
    /// The lease time of a host/service in milliseconds.
    pub fn lease(&self) -> zx::MonotonicDuration {
        zx::MonotonicDuration::from_millis(self.0.mLease.into())
    }

    /// The key lease time of a host/service in milliseconds.
    pub fn key_lease(&self) -> zx::MonotonicDuration {
        zx::MonotonicDuration::from_millis(self.0.mKeyLease.into())
    }

    /// The remaining lease time of the host/service in milliseconds.
    pub fn remaining_lease(&self) -> zx::MonotonicDuration {
        zx::MonotonicDuration::from_millis(self.0.mRemainingLease.into())
    }

    /// The remaining key lease time of a host/service in milliseconds.
    pub fn remaining_key_lease(&self) -> zx::MonotonicDuration {
        zx::MonotonicDuration::from_millis(self.0.mRemainingKeyLease.into())
    }
}
