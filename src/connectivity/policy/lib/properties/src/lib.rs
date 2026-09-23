// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Utilities to make interacting with `fuchsia.net.policy.properties` more ergonomic.

#![deny(unused)]
#![deny(missing_docs)]

use fidl_fuchsia_net_policy_properties as fnp_properties;

/// Extensions for [`fnp_properties::NetworkToken`].
pub trait NetworkTokenExt: Sized {
    /// Attempt to make a copy of the current [`fnp_properties::NetworkToken`],
    /// to allow it to be passed to a
    /// `fuchsia.net.policy.properties/Networks.WatchProperties` call.
    fn duplicate(&self) -> Result<Self, zx::Status>;

    /// Get the [`zx::Koid`] for the token value.
    fn koid(&self) -> Result<zx::Koid, zx::Status>;
}

impl NetworkTokenExt for fnp_properties::NetworkToken {
    fn duplicate(&self) -> Result<fnp_properties::NetworkToken, zx::Status> {
        Ok(fnp_properties::NetworkToken {
            value: self.value.duplicate_handle(zx::Rights::SAME_RIGHTS)?,
        })
    }

    fn koid(&self) -> Result<zx::Koid, zx::Status> {
        self.value.koid()
    }
}

/// Utilities to extend a [`fnp_properties::NetworksWatchDefaultResponse`].
pub trait NetworksWatchDefaultResponseExt {
    /// Return the resulting ['fnp_properties::NetworkToken`] or None if one isn't present.
    fn take_network(&mut self) -> Option<fnp_properties::NetworkToken>;

    /// Convert the response into its resulting [`fnp_properties::NetworkToken`]
    /// or None if one isn't present.
    fn into_network(self) -> Option<fnp_properties::NetworkToken>;
}

impl NetworksWatchDefaultResponseExt for fnp_properties::NetworksWatchDefaultResponse {
    fn take_network(&mut self) -> Option<fnp_properties::NetworkToken> {
        let mut replace =
            fnp_properties::NetworksWatchDefaultResponse::NoDefaultNetwork(fnp_properties::Empty);
        std::mem::swap(&mut replace, self);
        replace.into_network()
    }

    fn into_network(self) -> Option<fidl_fuchsia_net_policy_properties::NetworkToken> {
        match self {
            fnp_properties::NetworksWatchDefaultResponse::Network(network_token) => {
                Some(network_token)
            }
            fnp_properties::NetworksWatchDefaultResponse::NoDefaultNetwork(_)
            | fnp_properties::NetworksWatchDefaultResponse::__SourceBreaking { .. } => None,
        }
    }
}
