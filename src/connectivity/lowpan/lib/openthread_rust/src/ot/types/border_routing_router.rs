// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::prelude_internal::*;

use core::fmt::{Debug, Formatter};

/// This structure represents a peer Border Router found in the Network Data.
///
/// Functional equivalent of [`otsys::otBorderRoutingRouterEntry`](crate::otsys::otBorderRoutingRouterEntry).
#[derive(Default, Clone)]
#[repr(transparent)]
pub struct BorderRoutingRouter(pub otBorderRoutingRouterEntry);

impl_ot_castable!(BorderRoutingRouter, otBorderRoutingRouterEntry);

impl Debug for BorderRoutingRouter {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("otBorderRoutingRouterEntry")
            .field("address", &self.address())
            .field("msec_since_last_update", &self.msec_since_last_update())
            .field("age", &self.age())
            .field("managed_address_config_flag", &self.managed_address_config_flag())
            .field("other_config_flag", &self.other_config_flag())
            .field("snac_router_flag", &self.snac_router_flag())
            .field("is_local_device", &self.is_local_device())
            .field("is_reachable", &self.is_reachable())
            .field("is_peer_br", &self.is_peer_br())
            .finish()
    }
}

impl BorderRoutingRouter {
    /// IPv6 address of the router.
    pub fn address(&self) -> &Ip6Address {
        Ip6Address::ref_from_ot_ref(&self.0.mAddress)
    }

    /// Milliseconds since last update (any message rx) from this router.
    pub fn msec_since_last_update(&self) -> u32 {
        self.0.mMsecSinceLastUpdate
    }

    /// The router's age in seconds (duration since its first discovery).
    pub fn age(&self) -> u32 {
        self.0.mAge
    }

    /// The router's Managed Address Config flag (`M` flag).
    pub fn managed_address_config_flag(&self) -> bool {
        self.0.mManagedAddressConfigFlag()
    }

    /// The router's Other Config flag (`O` flag).
    pub fn other_config_flag(&self) -> bool {
        self.0.mOtherConfigFlag()
    }

    /// The router's SNAC Router flag (`S` flag).
    pub fn snac_router_flag(&self) -> bool {
        self.0.mSnacRouterFlag()
    }

    /// This router is the local device (this BR).
    pub fn is_local_device(&self) -> bool {
        self.0.mIsLocalDevice()
    }

    /// This router is reachable.
    pub fn is_reachable(&self) -> bool {
        self.0.mIsReachable()
    }

    /// This router is (likely) a peer BR.
    pub fn is_peer_br(&self) -> bool {
        self.0.mIsPeerBr()
    }
}
