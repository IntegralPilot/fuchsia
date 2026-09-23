// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::prelude_internal::*;

use core::fmt::{Debug, Formatter};

/// Data type representing 6LoWPAN Context ID information associated with a prefix in Network Data.
/// Functional equivalent of [`otsys::otLowpanContextInfo`](crate::otsys::otLowpanContextInfo).
#[derive(Default, Clone)]
#[repr(transparent)]
pub struct LowpanContextInfo(pub otLowpanContextInfo);

impl_ot_castable!(LowpanContextInfo, otLowpanContextInfo);

impl Debug for LowpanContextInfo {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LowpanContextInfo")
            .field("compress_flag", &self.compress_flag())
            .field("context_id", &self.context_id())
            .field("prefix", &self.prefix())
            .field("is_stable", &self.is_stable())
            .finish()
    }
}

impl LowpanContextInfo {
    /// Returns the compress flag.
    pub fn compress_flag(&self) -> bool {
        self.0.mCompressFlag()
    }

    /// Returns the 6LoWPAN Context ID.
    pub fn context_id(&self) -> u8 {
        self.0.mContextId
    }

    /// Returns the associated IPv6 prefix.
    pub fn prefix(&self) -> &Ip6Prefix {
        (&self.0.mPrefix).into()
    }

    /// Whether the Context TLV is marked as Stable Network Data.
    pub fn is_stable(&self) -> bool {
        self.0.mStable()
    }
}
