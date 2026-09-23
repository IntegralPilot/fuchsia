// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(test)]
pub mod testing;

use crate::att::pdu::ErrorCode;
use sapphire_common::{PeerId, Uuid};

/// Bluetooth Attribute Protocol (ATT) Attribute.
///
/// An attribute is an addressable value of a specific type (represented by a UUID)
/// that can be read or written to by a peer.
pub trait Attribute {
    /// The UUID representing the meaning of this attribute.
    fn uuid(&self) -> &Uuid;

    /// Initiates a read action, writing a chunk of populated bytes starting at `offset`
    /// (the byte offset from the start of the attribute's value) into `buf`.
    ///
    /// `peer_id` identifies the client, allowing peer-specific values (e.g. CCCD states).
    ///
    /// `buf` is typically sized according to the negotiated ATT MTU size (or up to the maximum
    /// spec-defined attribute value size of 512 bytes under BT Spec Vol 3, Part F, Section 3.2.9).
    /// The read operation is capped by and will write up to `buf.len()` bytes.
    ///
    /// # Returns
    ///
    /// * `Ok(usize)`: The number of bytes written to `buf` (capped at `buf.len()`).
    /// * `Err(ErrorCode)`: The ATT error code if the read failed.
    async fn read_chunk(
        &self,
        peer_id: PeerId,
        offset: u16,
        buf: &mut [u8],
    ) -> Result<usize, ErrorCode>;

    /// Initiates an atomic write of a chunk of data starting at `offset`
    /// (the byte offset from the start of the attribute's value).
    ///
    /// `peer_id` identifies the client, allowing peer-specific configuration updates.
    ///
    /// # Returns
    ///
    /// * `Ok(())` if the write succeeded (triggers a Write Response if requested).
    /// * `Err(ErrorCode)` indicating the reason for failure.
    async fn write_chunk(&self, peer_id: PeerId, offset: u16, data: &[u8])
    -> Result<(), ErrorCode>;

    /// Returns the ending handle of the group if this attribute is a group declaration.
    fn group_end_handle(&self) -> Option<u16> {
        None
    }
}
