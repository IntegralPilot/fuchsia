// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Netstack3 core UDP.
//!
//! This crate contains the UDP implementation for netstack3.

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
    pub(super) mod base;
    pub(super) mod counters;
    pub(super) mod diagnostics;
    pub(super) mod settings;
}

pub use internal::counters::{
    CombinedUdpCounters, UdpCounterContext, UdpCountersWithSocket, UdpCountersWithoutSocket,
};

pub use internal::base::{
    BoundSockets, BoundStateContext, DualStackBoundStateContext, DualStackUdpSocketId,
    NonDualStackBoundStateContext, ReceiveUdpError, SendError, SendToError, Sockets, StateContext,
    UdpApi, UdpBindingsContext, UdpBindingsTypes, UdpIpTransportContext, UdpPacketMeta,
    UdpReceiveBindingsContext, UdpRemotePort, UdpSocketId, UdpSocketSet, UdpSocketState,
    UdpSocketTxMetadata, UdpState, UdpStateContext, UseUdpIpTransportContextBlanket,
};

pub use internal::diagnostics::{UdpSocketDiagnosticTuple, UdpSocketDiagnostics};

pub use internal::settings::UdpSettings;
