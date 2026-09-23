// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bitflags::bitflags;

bitflags! {
    /// Bitflags type for transaction header at-rest flags.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct AtRestFlags: u16 {
        /// Indicates that the V2 wire format should be used instead of the V1
        /// wire format.
        /// This includes the following RFCs:
        /// - Efficient envelopes
        /// - Inlining small values in FIDL envelopes
        const USE_V2_WIRE_FORMAT = 2;
    }
}

bitflags! {
    /// Bitflags type to flags that aid in dynamically identifying features of
    /// the request.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct DynamicFlags: u8 {
        /// Indicates that the request is for a flexible method.
        const FLEXIBLE = 1 << 7;
    }
}

impl From<AtRestFlags> for [u8; 2] {
    #[inline]
    fn from(value: AtRestFlags) -> Self {
        value.bits().to_le_bytes()
    }
}

/// Header for transactional FIDL messages
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct TransactionHeader {
    /// Transaction ID which identifies a request-response pair
    pub tx_id: u32,
    /// Flags set for this message. MUST NOT be validated by bindings. Usually
    /// temporarily during migrations.
    pub at_rest_flags: [u8; 2],
    /// Flags used for dynamically interpreting the request if it is unknown to
    /// the receiver.
    pub dynamic_flags: u8,
    /// Magic number indicating the message's wire format. Two sides with
    /// different magic numbers are incompatible with each other.
    pub magic_number: u8,
    /// Ordinal which identifies the FIDL method
    pub ordinal: u64,
}

impl TransactionHeader {
    fn from_bytes(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.len() < std::mem::size_of::<Self>() {
            return Err("not enough bytes to decode header");
        }
        Ok(unsafe {
            let bytes_ptr = bytes.as_ptr();
            let header_ptr = bytes_ptr as *const Self;
            std::ptr::read_unaligned(header_ptr)
        })
    }
}

/// Decodes the transaction header from a message.
/// Returns the header and a reference to the tail of the message.
pub(crate) fn decode_transaction_header(
    bytes: &[u8],
) -> Result<(TransactionHeader, &[u8]), &'static str> {
    const HEADER_SIZE: usize = std::mem::size_of::<TransactionHeader>();

    if bytes.len() < HEADER_SIZE {
        return Err("not enough bytes to decode header");
    }

    let (header_bytes, payload_bytes) = bytes.split_at(HEADER_SIZE);

    Ok((TransactionHeader::from_bytes(header_bytes)?, payload_bytes))
}
