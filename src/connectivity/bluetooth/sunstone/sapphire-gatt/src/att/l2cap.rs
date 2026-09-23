// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// TODO(https://fxbug.dev/530178022): Remove once it's being used in ATT
#![allow(unused)]

pub mod cid;

use core::mem::MaybeUninit;

pub use cid::{Cid, FixedCid};

#[cfg(test)]
pub mod mock;

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum L2CapEstablishChannelError {
    /// The requested channel already has an owner
    AlreadyInUse,
    /// Underlying logical link was closed
    LinkClosed,
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum L2CapSendError {
    /// Underlying logical link was closed
    LinkClosed,
    /// The outgoing SDU exceeds the SDU size limit
    SduTooLarge,
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum L2CapRecvError {
    /// Underlying logical link was closed
    LinkClosed,
    /// The incoming SDU won't fit in the provided buffer
    BufferTooSmall,
}

/// Represents an L2CAP logical link to a peer
pub trait L2CapLogicalLink {
    type Tx: L2CapChannelTx;
    type Rx: L2CapChannelRx;

    /// Establishes a connection to the given channel
    async fn claim_fixed_channel(
        &mut self,
        channel: FixedCid,
    ) -> Result<L2CapChannel<Self::Tx, Self::Rx>, L2CapEstablishChannelError>;
}

/// Sender and receiver ends to to an L2CAP channel
pub struct L2CapChannel<Tx, Rx> {
    pub sender: Tx,
    pub receiver: Rx,
}

/// Sender end to an L2CAP channel
pub trait L2CapChannelTx: Clone {
    /// Sends the given SDU to the channel
    async fn send(&mut self, sdu: &[u8]) -> Result<(), L2CapSendError>;
}

/// Receiver end to an L2CAP channel
pub trait L2CapChannelRx {
    /// Receives the next SDU sent by the peer
    async fn recv<'a>(
        &mut self,
        buffer: &'a mut [MaybeUninit<u8>],
    ) -> Result<&'a mut [u8], L2CapRecvError>;
}
