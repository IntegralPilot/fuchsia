// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::prelude_internal::*;

/// This structure represents a peer Border Router found in the Network Data.
///
/// Functional equivalent of [`otsys::otBorderRoutingPeerBorderRouterEntry`](crate::otsys::otBorderRoutingPeerBorderRouterEntry).
#[derive(Debug, Default, Clone)]
#[repr(transparent)]
pub struct BorderRoutingPeer(pub otBorderRoutingPeerBorderRouterEntry);

impl_ot_castable!(BorderRoutingPeer, otBorderRoutingPeerBorderRouterEntry);

impl BorderRoutingPeer {
    /// The RLOC16 of BR.
    pub fn rloc16(&self) -> u16 {
        self.0.mRloc16
    }

    /// Seconds since the BR appeared in the Network Data.
    pub fn age(&self) -> u32 {
        self.0.mAge
    }
}
