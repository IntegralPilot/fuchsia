// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Netstack3 core ICMP Echo sockets.
//!
//! This crate contains the ICMP echo sockets implementation for Netstack3.

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
    pub(super) mod settings;
    pub(super) mod socket;
}

pub use internal::socket::{
    BoundSockets, IcmpEchoBindingsContext, IcmpEchoBindingsTypes, IcmpEchoBoundStateContext,
    IcmpEchoContextMarker, IcmpEchoIpTransportContext, IcmpEchoSocketApi, IcmpEchoStateContext,
    IcmpSocketId, IcmpSocketSet, IcmpSocketState, IcmpSocketTxMetadata, IcmpSockets,
    ReceiveIcmpEchoError,
};

pub use internal::settings::IcmpEchoSettings;
