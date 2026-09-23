// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Netstack3 core Datagram sockets.
//!
//! This crate contains the shared base implementation between UDP and ICMP Echo
//! sockets.

#![no_std]
#![warn(
    missing_docs,
    unreachable_patterns,
    clippy::useless_conversion,
    clippy::redundant_clone,
    clippy::precedence
)]

extern crate alloc;

#[path = "."]
mod internal {
    pub(super) mod datagram;
    pub(super) mod diagnostics;
    pub(super) mod settings;
    pub(super) mod sndbuf;
    pub(super) mod spec_context;
    pub(super) mod uninstantiable;
}

pub use internal::datagram::{
    BoundDatagramSocketMap, BoundSocketState, BoundSocketStateType, ConnInfo, ConnState,
    ConnectError, DatagramApi, DatagramBindingsContext, DatagramBindingsTypes,
    DatagramBoundStateContext, DatagramFlowId, DatagramIpSpecificSocketOptions,
    DatagramSocketMapSpec, DatagramSocketSet, DatagramSocketSpec, DatagramStateContext,
    DualStackBaseIpExt, DualStackConnState, DualStackConverter, DualStackDatagramBoundStateContext,
    DualStackIpExt, EitherIpSocket, ExpectedConnError, ExpectedUnboundError, InUseError, IpExt,
    IpOptions, ListenerInfo, MulticastInterfaceSelector, MulticastMembershipInterfaceSelector,
    NonDualStackConverter, NonDualStackDatagramBoundStateContext, PendingDatagramSocketError,
    ReferenceState, SendError, SendToError, SetMulticastMembershipError, SocketInfo, SocketState,
    SocketStateInner, StrongRc, WeakRc, WrapOtherStackIpOptions, WrapOtherStackIpOptionsMut,
};
pub use internal::diagnostics::{DatagramSocketDiagnosticsSpec, SocketStateForMatching};
pub use internal::settings::DatagramSettings;
pub use internal::sndbuf::TxMetadata;
pub use internal::spec_context::{
    DatagramSpecBoundStateContext, DatagramSpecStateContext,
    DualStackDatagramSpecBoundStateContext, NonDualStackDatagramSpecBoundStateContext,
};

/// Datagram socket test utilities.
#[cfg(any(test, feature = "testutils"))]
pub mod testutil {
    pub use crate::internal::datagram::create_primary_id;
    pub use crate::internal::datagram::testutil::setup_fake_ctx_with_dualstack_conn_addrs;
}
