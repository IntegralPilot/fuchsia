// SPDX-License-Identifier: MIT

use crate::address::{AddressAttribute, AddressError, AddressHeaderFlags, AddressScope};
use crate::{AddressFamily, RouteNetlinkMessageParseMode};
use netlink_packet_utils::nla::{HasNlas, NlaBuffer, NlaError, NlaParseMode, NlasIterator};
use netlink_packet_utils::traits::{Emitable, Parseable};
use zerocopy::byteorder::native_endian::U32;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned};

pub const ADDRESS_HEADER_LEN: usize = 8;

#[derive(FromBytes, IntoBytes, KnownLayout, Immutable, Unaligned)]
#[repr(C)]
pub struct AddressMessageBuffer {
    header: AddressHeader,
    payload: [u8],
}

use netlink_packet_utils::{DecodeError, ParseableParametrized};

#[derive(
    FromBytes,
    IntoBytes,
    KnownLayout,
    Immutable,
    Unaligned,
    Debug,
    PartialEq,
    Eq,
    Clone,
    Copy,
    Default,
)]
#[repr(C)]
pub struct AddressHeader {
    pub family: u8,
    pub prefix_len: u8,
    pub flags: u8,
    pub scope: u8,
    pub index: U32,
}

impl AddressHeader {
    pub fn family(&self) -> AddressFamily {
        self.family.into()
    }

    pub fn flags(&self) -> AddressHeaderFlags {
        AddressHeaderFlags::from_bits_truncate(self.flags)
    }

    pub fn scope(&self) -> AddressScope {
        self.scope.into()
    }
}

impl AddressMessageBuffer {
    pub fn new(bytes: &[u8]) -> Result<&AddressMessageBuffer, DecodeError> {
        AddressMessageBuffer::ref_from_prefix(bytes).map(|(buffer, _rest)| buffer).map_err(|_e| {
            DecodeError::InvalidBufferLength {
                name: "AddressMessageBuffer",
                len: bytes.len(),
                buffer_len: ADDRESS_HEADER_LEN,
            }
        })
    }

    pub fn new_mut(bytes: &mut [u8]) -> Result<&mut AddressMessageBuffer, DecodeError> {
        let len = bytes.len();
        AddressMessageBuffer::mut_from_prefix(bytes).map(|(buffer, _rest)| buffer).map_err(|_e| {
            DecodeError::InvalidBufferLength {
                name: "AddressMessageBuffer",
                len,
                buffer_len: ADDRESS_HEADER_LEN,
            }
        })
    }
}

impl HasNlas for AddressMessageBuffer {
    fn attributes(&self) -> impl Iterator<Item = Result<NlaBuffer<&[u8]>, NlaError>> {
        NlasIterator::new(&self.payload)
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Default)]
#[non_exhaustive]
pub struct AddressMessage {
    pub header: AddressHeader,
    pub attributes: Vec<AddressAttribute>,
}

impl Emitable for AddressHeader {
    fn buffer_len(&self) -> usize {
        ADDRESS_HEADER_LEN
    }

    fn emit(&self, buffer: &mut [u8]) {
        let packet =
            AddressMessageBuffer::new_mut(buffer).expect("buffer has incorrect size/alignment");
        packet.header = *self
    }
}

impl Emitable for AddressMessage {
    fn buffer_len(&self) -> usize {
        self.header.buffer_len() + self.attributes.as_slice().buffer_len()
    }

    fn emit(&self, buffer: &mut [u8]) {
        self.header.emit(buffer);
        self.attributes.as_slice().emit(&mut buffer[self.header.buffer_len()..]);
    }
}

impl Parseable<AddressMessageBuffer> for AddressHeader {
    type Error = ();
    fn parse(buf: &AddressMessageBuffer) -> Result<Self, ()> {
        Ok(Self {
            family: buf.header.family.into(),
            prefix_len: buf.header.prefix_len,
            flags: AddressHeaderFlags::from_bits_retain(buf.header.flags).bits(),
            scope: buf.header.scope.into(),
            index: buf.header.index.into(),
        })
    }
}

impl<'a> ParseableParametrized<AddressMessageBuffer, RouteNetlinkMessageParseMode>
    for AddressMessage
{
    type Error = AddressError;
    fn parse_with_param(
        buf: &AddressMessageBuffer,
        mode: RouteNetlinkMessageParseMode,
    ) -> Result<Self, AddressError> {
        Ok(AddressMessage {
            // ok to unwrap, we never fail parsing the header.
            header: AddressHeader::parse(buf).unwrap(),
            attributes: Vec::<AddressAttribute>::parse_with_param(buf, mode.into())?,
        })
    }
}

impl<'a> ParseableParametrized<AddressMessageBuffer, NlaParseMode> for Vec<AddressAttribute> {
    type Error = AddressError;
    fn parse_with_param(
        buf: &AddressMessageBuffer,
        mode: NlaParseMode,
    ) -> Result<Self, AddressError> {
        buf.parse_attributes(mode, AddressAttribute::parse)
    }
}
