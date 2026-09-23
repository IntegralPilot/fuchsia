// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Datagram socket sendbuffer definitions.

use core::borrow::Borrow;
use core::mem::ManuallyDrop;

use derivative::Derivative;
use net_types::ip::{GenericOverIp, Ip, IpVersion};
use netstack3_base::socket::{SendBufferFullError, SendBufferSpace};
use netstack3_base::{ChecksumOffloadResult, PositiveIsize, WeakDeviceIdentifier};
use packet::FragmentedBuffer;

use crate::internal::datagram::{DatagramSocketSpec, IpExt};
use crate::internal::settings::DatagramSettings;

#[derive(Derivative)]
#[derivative(Debug(bound = ""))]
pub(crate) struct SendBufferTracking<S: DatagramSocketSpec>(
    netstack3_base::socket::SendBufferTracking<S::SocketWritableListener>,
);

pub(crate) enum SendBufferError {
    SendBufferFull,
    InvalidLength,
}

impl From<SendBufferFullError> for SendBufferError {
    fn from(SendBufferFullError: SendBufferFullError) -> Self {
        Self::SendBufferFull
    }
}

impl<S: DatagramSocketSpec> SendBufferTracking<S> {
    pub(crate) fn new(listener: S::SocketWritableListener, settings: &DatagramSettings) -> Self {
        Self(netstack3_base::socket::SendBufferTracking::new(
            settings.send_buffer.default(),
            listener,
        ))
    }

    pub(crate) fn set_capacity(&self, capacity: usize, settings: &DatagramSettings) {
        let Self(tracking) = self;
        let capacity = PositiveIsize::new_unsigned(capacity.max(settings.send_buffer.min().into()))
            .unwrap_or_else(|| settings.send_buffer.max())
            .min(settings.send_buffer.max());
        tracking.set_capacity(capacity);
    }

    pub(crate) fn capacity(&self) -> usize {
        let Self(tracking) = self;
        tracking.capacity().into()
    }

    #[cfg(any(test, feature = "testutils"))]
    pub(crate) fn available(&self) -> usize {
        let Self(tracking) = self;
        tracking.available().map(Into::into).unwrap_or(0)
    }

    pub(crate) fn prepare_for_send<
        WireI: Ip,
        SocketI: IpExt,
        D: WeakDeviceIdentifier,
        B: FragmentedBuffer,
    >(
        &self,
        id: &S::SocketId<SocketI, D>,
        buffer: &B,
        send_token: S::SendToken,
    ) -> Result<TxMetadata<SocketI, D, S>, SendBufferError> {
        // Always penalize the send buffer by the cost of a fixed header.
        let header_len = match WireI::VERSION {
            IpVersion::V4 => packet_formats::ipv4::HDR_PREFIX_LEN,
            IpVersion::V6 => packet_formats::ipv6::IPV6_FIXED_HDR_LEN,
        } + S::FIXED_HEADER_SIZE;
        self.prepare_for_send_inner(buffer.len() + header_len, id, send_token)
    }

    fn prepare_for_send_inner<I: IpExt, D: WeakDeviceIdentifier>(
        &self,
        size: usize,
        id: &S::SocketId<I, D>,
        send_token: S::SendToken,
    ) -> Result<TxMetadata<I, D, S>, SendBufferError> {
        let Self(tracking) = self;
        // System imposes a limit of isize::max length for a single datagram.
        let size = PositiveIsize::new_unsigned(size).ok_or(SendBufferError::InvalidLength)?;
        let space = tracking.acquire(size)?;
        Ok(TxMetadata {
            socket: S::downgrade_socket_id(id),
            space: ManuallyDrop::new(space),
            _send_token: send_token,
            checksum_offload_result: None,
        })
    }
}

/// The tx metadata associated with a datagram socket.
#[derive(Derivative, GenericOverIp)]
#[generic_over_ip(I, Ip)]
#[derivative(Debug(bound = ""))]
pub struct TxMetadata<I: IpExt, D: WeakDeviceIdentifier, S: DatagramSocketSpec> {
    socket: S::WeakSocketId<I, D>,
    space: ManuallyDrop<SendBufferSpace>,
    _send_token: S::SendToken,
    checksum_offload_result: Option<ChecksumOffloadResult>,
}

impl<I: IpExt, D: WeakDeviceIdentifier, S: DatagramSocketSpec> TxMetadata<I, D, S> {
    /// Gets the socket from which the packet originates.
    pub fn socket(&self) -> &S::WeakSocketId<I, D> {
        &self.socket
    }

    /// Returns the checksum offload result.
    pub fn checksum_offload_result(&self) -> Option<ChecksumOffloadResult> {
        self.checksum_offload_result.clone()
    }

    /// Sets the checksum offload result.
    pub fn set_checksum_offload_result(&mut self, result: Option<ChecksumOffloadResult>) {
        self.checksum_offload_result = result;
    }
}

impl<I: IpExt, D: WeakDeviceIdentifier, S: DatagramSocketSpec> Drop for TxMetadata<I, D, S> {
    fn drop(&mut self) {
        let Self { socket, space, _send_token: _, checksum_offload_result: _ } = self;
        // Take space out and leave the slot in uninitialized state so drop is
        // not called.
        //
        // SAFETY: space is not used again (shadowed here).
        let space = unsafe { ManuallyDrop::take(space) };
        match S::upgrade_socket_id(socket) {
            Some(socket) => {
                let SendBufferTracking(tracking) = &socket.borrow().send_buffer;
                tracking.release(space)
            }
            None => {
                // Failed to upgrade the socket, acknowledge the space being
                // dropped.
                space.acknowledge_drop();
            }
        }
    }
}

#[cfg(any(test, feature = "testutils"))]
impl<I: IpExt, D: WeakDeviceIdentifier, S: DatagramSocketSpec> PartialEq for TxMetadata<I, D, S> {
    fn eq(&self, other: &Self) -> bool {
        // Tx metadata is always a unique instance accompanying a frame and it's
        // not copiable. So it may only be equal to another instance if they're
        // the exact same object.
        core::ptr::eq(self, other)
    }
}
