// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Definitions for datagram socket types that implement socket diagnostics.

use core::marker::PhantomData;

use net_types::Witness;
use netstack3_base::{
    IpSocketProperties, Matcher, SocketTransportProtocolMatcher, WeakDeviceIdentifier,
};

use crate::internal::datagram::{
    DatagramBindingsTypes, DatagramBoundStateContext, DatagramSocketSpec, IpExt, SocketState,
};

/// Implementation-specific functionality for matching datagram sockets.
///
/// Implementing this trait means that [`SocketStateForMatching`] will
/// automatically implement [`IpSocketProperties`] for that datagram socket
/// protocol.
pub trait DatagramSocketDiagnosticsSpec: DatagramSocketSpec {
    /// The device class of physical interfaces used by the sockets.
    type DeviceClass;

    /// Returns whether the provided `state` is matched by the transport
    /// protocol matcher `matcher`.
    fn transport_protocol_matches<I: IpExt, D: WeakDeviceIdentifier>(
        state: &SocketState<I, D, Self>,
        matcher: &SocketTransportProtocolMatcher,
    ) -> bool;

    /// Returns whether the provided `id` is matched by the cookie matcher
    /// `matcher`.
    fn cookie_matches<I: IpExt, D: WeakDeviceIdentifier>(
        id: &Self::SocketId<I, D>,
        matcher: &netstack3_base::SocketCookieMatcher,
    ) -> bool;
}

/// State required for matching a socket, gathered into one struct to allow
/// implementing traits against the collection.
pub struct SocketStateForMatching<'a, I, D, S, BC, CC>
where
    I: IpExt,
    D: WeakDeviceIdentifier,
    S: DatagramSocketSpec,
    BC: DatagramBindingsTypes,
{
    state: &'a SocketState<I, D, S>,
    id: &'a S::SocketId<I, D>,
    ctx: &'a CC,
    _bindings_ctx: PhantomData<BC>,
}

impl<'a, I, D, S, BC, CC> SocketStateForMatching<'a, I, D, S, BC, CC>
where
    I: IpExt,
    D: WeakDeviceIdentifier,
    S: DatagramSocketSpec,
    BC: DatagramBindingsTypes,
{
    /// Wraps the required state into a [`SocketStateForMatching`].
    pub fn new(state: &'a SocketState<I, D, S>, id: &'a S::SocketId<I, D>, ctx: &'a CC) -> Self {
        Self { state, id, ctx, _bindings_ctx: PhantomData }
    }
}

impl<I, D, S, BC, CC> IpSocketProperties<S::DeviceClass>
    for SocketStateForMatching<'_, I, D, S, BC, CC>
where
    I: IpExt,
    D: WeakDeviceIdentifier,
    S: DatagramSocketDiagnosticsSpec,
    BC: DatagramBindingsTypes,
    CC: DatagramBoundStateContext<I, BC, S, WeakDeviceId = D>,
    D::Strong: netstack3_base::InterfaceProperties<S::DeviceClass>,
{
    fn family_matches(&self, family: &net_types::ip::IpVersion) -> bool {
        I::VERSION == *family
    }

    fn src_addr_matches(&self, addr: &netstack3_base::BoundAddressMatcherEither) -> bool {
        addr.matches(&self.state.local_ip().map(|addr| addr.addr().get().into()))
    }

    fn dst_addr_matches(&self, addr: &netstack3_base::BoundAddressMatcherEither) -> bool {
        addr.matches(&self.state.remote_ip().map(|addr| addr.addr().get().into()))
    }

    fn transport_protocol_matches(&self, proto: &SocketTransportProtocolMatcher) -> bool {
        S::transport_protocol_matches(self.state, proto)
    }

    fn bound_interface_matches(
        &self,
        iface: &netstack3_base::BoundInterfaceMatcher<S::DeviceClass>,
    ) -> bool {
        let device = self.state.get_device(self.ctx).as_ref().and_then(|weak| weak.upgrade());
        iface.matches(&device.as_ref())
    }

    fn cookie_matches(&self, cookie: &netstack3_base::SocketCookieMatcher) -> bool {
        S::cookie_matches(self.id, cookie)
    }

    fn mark_matches(&self, mark: &netstack3_base::MarkInDomainMatcher) -> bool {
        let options = self.state.options();
        mark.matcher.matches(options.marks().get(mark.domain))
    }
}
