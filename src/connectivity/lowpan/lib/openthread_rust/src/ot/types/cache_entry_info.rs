// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::prelude_internal::*;
use std::fmt::{Debug, Formatter};

/// Represents an EID cache entry.
#[derive(Default, Clone, Copy)]
#[repr(transparent)]
pub struct CacheEntryInfo(pub otCacheEntryInfo);

impl_ot_castable!(CacheEntryInfo, otCacheEntryInfo);

/// Defines the EID cache entry state.
///
/// Functional equivalent of [`otsys::otCacheEntryState`](crate::otsys::otCacheEntryState).
#[derive(Debug, Copy, Clone, Eq, Ord, PartialOrd, PartialEq, num_derive::FromPrimitive)]
#[allow(missing_docs)]
pub enum CacheEntryState {
    Cached = OT_CACHE_ENTRY_STATE_CACHED as isize,
    Snooped = OT_CACHE_ENTRY_STATE_SNOOPED as isize,
    Query = OT_CACHE_ENTRY_STATE_QUERY as isize,
    Retry = OT_CACHE_ENTRY_STATE_RETRY_QUERY as isize,
    Unknown,
}

impl From<otCacheEntryState> for CacheEntryState {
    fn from(x: otCacheEntryState) -> Self {
        use num::FromPrimitive;
        Self::from_u32(x).unwrap_or_else(|| {
            warn!("Unknown otCacheEntryState value: {x}. Falling back to default state.");
            CacheEntryState::Unknown
        })
    }
}

impl Debug for CacheEntryInfo {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CacheEntryInfo")
            .field("target", &self.target())
            .field("rloc16", &self.rloc16())
            .field("state", &self.state())
            .field("can_evict", &self.can_evict())
            .field("ramp_down", &self.ramp_down())
            .field("valid_last_trans", &self.valid_last_trans())
            .field("last_trans_time", &self.last_trans_time())
            .field("mesh_local_eid", &self.mesh_local_eid())
            .field("timeout", &self.timeout())
            .field("retry_delay", &self.retry_delay())
            .finish()
    }
}

impl CacheEntryInfo {
    /// Returns the target EID.
    pub fn target(&self) -> &Ip6Address {
        Ip6Address::ref_from_ot_ref(&self.0.mTarget)
    }

    /// Returns the RLOC16.
    pub fn rloc16(&self) -> ShortAddress {
        self.0.mRloc16
    }

    /// Returns the Entry state.
    pub fn state(&self) -> CacheEntryState {
        CacheEntryState::from(self.0.mState)
    }

    /// Indicates whether the entry can be evicted.
    pub fn can_evict(&self) -> bool {
        self.0.mCanEvict()
    }

    /// Indicates whether in ramp-down mode while in `OT_CACHE_ENTRY_STATE_RETRY_QUERY`.
    pub fn ramp_down(&self) -> bool {
        self.0.mRampDown()
    }

    /// Indicates whether last transaction time and ML-EID are valid.
    pub fn valid_last_trans(&self) -> bool {
        self.0.mValidLastTrans()
    }

    /// Returns the last transaction time (applicable in cached state).
    pub fn last_trans_time(&self) -> u32 {
        self.0.mLastTransTime
    }

    /// Returns the Mesh Local EID (applicable if entry in cached state).
    pub fn mesh_local_eid(&self) -> &Ip6Address {
        Ip6Address::ref_from_ot_ref(&self.0.mMeshLocalEid)
    }

    /// Returns the timeout in seconds (applicable if in snooped/query/retry-query states).
    pub fn timeout(&self) -> u16 {
        self.0.mTimeout
    }

    /// Returns retry delay in seconds (applicable if in query-retry state).
    pub fn retry_delay(&self) -> u16 {
        self.0.mRetryDelay
    }
}
