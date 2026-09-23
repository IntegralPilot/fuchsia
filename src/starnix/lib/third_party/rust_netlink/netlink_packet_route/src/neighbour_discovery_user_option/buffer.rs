// SPDX-License-Identifier: MIT

use netlink_packet_utils::DecodeError;
use netlink_packet_utils::nla::{HasNlas, NlaBuffer, NlaError, NlasIterator};

pub const NEIGHBOUR_DISCOVERY_USER_OPTION_HEADER_LEN: usize = 16;

buffer!(NeighbourDiscoveryUserOptionMessageBuffer() {
    address_family: (u8, 0),
    padding_1: (u8, 1),
    options_length: (u16, 2..4),
    interface_index: (u32, 4..8),
    icmp_type: (u8, 8),
    icmp_code: (u8, 9),
    padding_2: (u16, 10..12),
    padding_3: (u32, 12..16),
    payload: (slice, NEIGHBOUR_DISCOVERY_USER_OPTION_HEADER_LEN..),
});

impl<'a, T: AsRef<[u8]> + ?Sized> HasNlas for NeighbourDiscoveryUserOptionMessageBuffer<&'a T> {
    fn attributes(&self) -> impl Iterator<Item = Result<NlaBuffer<&[u8]>, NlaError>> {
        NlasIterator::new(&self.payload()[self.options_length() as usize..])
    }
}

impl<T: AsRef<[u8]>> NeighbourDiscoveryUserOptionMessageBuffer<T> {
    pub fn new(buffer: T) -> Result<Self, DecodeError> {
        let packet = Self::new_unchecked(buffer);
        packet.check_buffer_length()?;
        Ok(packet)
    }

    fn check_buffer_length(&self) -> Result<(), DecodeError> {
        let len = self.buffer.as_ref().len();
        let required_len =
            NEIGHBOUR_DISCOVERY_USER_OPTION_HEADER_LEN + self.options_length() as usize;
        if len < required_len {
            Err(DecodeError::InvalidBufferLength {
                name: "NeighbourDiscoveryUserOptionMessageBuffer",
                len,
                buffer_len: required_len,
            })
        } else {
            Ok(())
        }
    }
}

impl<'a, T: AsRef<[u8]> + ?Sized> NeighbourDiscoveryUserOptionMessageBuffer<&'a T> {
    pub fn option_body(&self) -> &[u8] {
        &self.payload()[..self.options_length() as usize]
    }
}
