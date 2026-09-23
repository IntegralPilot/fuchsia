// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::prelude_internal::*;

/// This structure represents SRP server response counters.
///
/// Functional equivalent of [`otsys::otSrpServerResponseCounters`](crate::otsys::otSrpServerResponseCounters).
#[derive(Debug, Default, Clone)]
#[repr(transparent)]
pub struct SrpServerResponseCounters(pub otSrpServerResponseCounters);

impl_ot_castable!(SrpServerResponseCounters, otSrpServerResponseCounters);

impl SrpServerResponseCounters {
    /// The number of successful responses.
    pub fn success(&self) -> u32 {
        self.0.mSuccess
    }

    /// The number of server failure responses.
    pub fn server_failure(&self) -> u32 {
        self.0.mServerFailure
    }

    /// The number of format error responses.
    pub fn format_error(&self) -> u32 {
        self.0.mFormatError
    }

    /// The number of name exists responses.
    pub fn name_exists(&self) -> u32 {
        self.0.mNameExists
    }

    /// The number of refused responses.
    pub fn refused(&self) -> u32 {
        self.0.mRefused
    }

    /// The number of other responses.
    pub fn other(&self) -> u32 {
        self.0.mOther
    }
}
