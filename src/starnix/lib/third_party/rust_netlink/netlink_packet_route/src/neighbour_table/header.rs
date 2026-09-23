// SPDX-License-Identifier: MIT

use netlink_packet_utils::DecodeError;
use netlink_packet_utils::nla::{HasNlas, NlaBuffer, NlaError, NlasIterator};
use netlink_packet_utils::traits::{Emitable, Parseable};

use crate::AddressFamily;

const NEIGHBOUR_TABLE_HEADER_LEN: usize = 4;

buffer!(NeighbourTableMessageBuffer(NEIGHBOUR_TABLE_HEADER_LEN) {
    family: (u8, 0),
    payload: (slice, NEIGHBOUR_TABLE_HEADER_LEN..),
});

impl<'a, T: AsRef<[u8]> + ?Sized> HasNlas for NeighbourTableMessageBuffer<&'a T> {
    fn attributes(&self) -> impl Iterator<Item = Result<NlaBuffer<&[u8]>, NlaError>> {
        NlasIterator::new(self.payload())
    }
}

// kernel code is `struct rtgenmsg`
#[derive(Debug, PartialEq, Eq, Clone, Copy, Default)]
pub struct NeighbourTableHeader {
    pub family: AddressFamily,
}

impl<T: AsRef<[u8]>> Parseable<NeighbourTableMessageBuffer<T>> for NeighbourTableHeader {
    type Error = ();
    fn parse(buf: &NeighbourTableMessageBuffer<T>) -> Result<Self, ()> {
        Ok(Self { family: buf.family().into() })
    }
}

impl Emitable for NeighbourTableHeader {
    fn buffer_len(&self) -> usize {
        NEIGHBOUR_TABLE_HEADER_LEN
    }

    fn emit(&self, buffer: &mut [u8]) {
        let mut packet = NeighbourTableMessageBuffer::new_unchecked(buffer);
        packet.set_family(self.family.into());
    }
}
