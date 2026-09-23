// SPDX-License-Identifier: MIT

use super::super::AddressFamily;
use super::{NeighbourAttribute, NeighbourError, NeighbourHeader, NeighbourMessageBuffer};
use crate::RouteNetlinkMessageParseMode;
use netlink_packet_utils::nla::{HasNlas, NlaParseMode};
use netlink_packet_utils::traits::{Emitable, Parseable, ParseableParametrized};

#[derive(Debug, PartialEq, Eq, Clone, Default)]
#[non_exhaustive]
pub struct NeighbourMessage {
    pub header: NeighbourHeader,
    pub attributes: Vec<NeighbourAttribute>,
}

impl Emitable for NeighbourMessage {
    fn buffer_len(&self) -> usize {
        self.header.buffer_len() + self.attributes.as_slice().buffer_len()
    }

    fn emit(&self, buffer: &mut [u8]) {
        self.header.emit(buffer);
        self.attributes.as_slice().emit(&mut buffer[self.header.buffer_len()..]);
    }
}

impl<'a, T: AsRef<[u8]> + 'a>
    ParseableParametrized<NeighbourMessageBuffer<&'a T>, RouteNetlinkMessageParseMode>
    for NeighbourMessage
{
    type Error = NeighbourError;
    fn parse_with_param(
        buf: &NeighbourMessageBuffer<&'a T>,
        mode: RouteNetlinkMessageParseMode,
    ) -> Result<Self, NeighbourError> {
        // unwrap: parsing the header is always ok.
        let header = NeighbourHeader::parse(buf).unwrap();
        let address_family = header.family;
        Ok(NeighbourMessage {
            header,
            attributes: Vec::<NeighbourAttribute>::parse_with_param(
                buf,
                (mode.into(), address_family),
            )?,
        })
    }
}

impl<'a, T: AsRef<[u8]> + 'a>
    ParseableParametrized<NeighbourMessageBuffer<&'a T>, (NlaParseMode, AddressFamily)>
    for Vec<NeighbourAttribute>
{
    type Error = NeighbourError;
    fn parse_with_param(
        buf: &NeighbourMessageBuffer<&'a T>,
        (mode, address_family): (NlaParseMode, AddressFamily),
    ) -> Result<Self, NeighbourError> {
        buf.parse_attributes(mode, |nla_buf| {
            NeighbourAttribute::parse_with_param(nla_buf, address_family)
        })
    }
}
