// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::prelude_internal::*;

use core::fmt::{Debug, Formatter};

/// This structure represents discovered Recursive DNS Server (RDNSS) address entry.
///
/// Functional equivalent of [`otsys::otBorderRoutingRdnssAddrEntry`]
/// (crate::otsys::otBorderRoutingRdnssAddrEntry).
#[derive(Default, Clone)]
#[repr(transparent)]
pub struct BorderRoutingRdnss(pub otBorderRoutingRdnssAddrEntry);

impl_ot_castable!(BorderRoutingRdnss, otBorderRoutingRdnssAddrEntry);

impl Debug for BorderRoutingRdnss {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BorderRoutingRdnssAddrEntry")
            .field("router", &self.router())
            .field("address", &self.address())
            .field("msec_since_last_update", &self.msec_since_last_update())
            .field("lifetime", &self.lifetime())
            .finish()
    }
}

impl BorderRoutingRdnss {
    /// Returns the information about the router advertising this address.
    pub fn router(&self) -> BorderRoutingRouter {
        BorderRoutingRouter(self.0.mRouter)
    }

    /// Returns the DNS Server IPv6 address.
    pub fn address(&self) -> &Ip6Address {
        Ip6Address::ref_from_ot_ref(&self.0.mAddress)
    }

    /// Returns the milliseconds since last update of this address.
    pub fn msec_since_last_update(&self) -> u32 {
        self.0.mMsecSinceLastUpdate
    }

    /// Returns the lifetime of the address (in seconds).
    pub fn lifetime(&self) -> u32 {
        self.0.mLifetime
    }
}
