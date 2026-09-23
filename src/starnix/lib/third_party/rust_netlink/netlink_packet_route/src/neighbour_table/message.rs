// SPDX-License-Identifier: MIT

use super::{
    NeighbourTableAttribute, NeighbourTableError, NeighbourTableHeader, NeighbourTableMessageBuffer,
};
use crate::RouteNetlinkMessageParseMode;
use netlink_packet_utils::nla::{HasNlas, NlaParseMode};
use netlink_packet_utils::traits::{Emitable, Parseable, ParseableParametrized};

#[derive(Debug, PartialEq, Eq, Clone, Default)]
#[non_exhaustive]
pub struct NeighbourTableMessage {
    pub header: NeighbourTableHeader,
    pub attributes: Vec<NeighbourTableAttribute>,
}

impl Emitable for NeighbourTableMessage {
    fn buffer_len(&self) -> usize {
        self.header.buffer_len() + self.attributes.as_slice().buffer_len()
    }

    fn emit(&self, buffer: &mut [u8]) {
        self.header.emit(buffer);
        self.attributes.as_slice().emit(&mut buffer[self.header.buffer_len()..]);
    }
}

impl<'a, T: AsRef<[u8]> + 'a>
    ParseableParametrized<NeighbourTableMessageBuffer<&'a T>, RouteNetlinkMessageParseMode>
    for NeighbourTableMessage
{
    type Error = NeighbourTableError;
    fn parse_with_param(
        buf: &NeighbourTableMessageBuffer<&'a T>,
        mode: RouteNetlinkMessageParseMode,
    ) -> Result<Self, NeighbourTableError> {
        Ok(NeighbourTableMessage {
            // unwrap: we always succeed at parsing the header
            header: NeighbourTableHeader::parse(buf).unwrap(),
            attributes: Vec::<NeighbourTableAttribute>::parse_with_param(buf, mode.into())?,
        })
    }
}

impl<'a, T: AsRef<[u8]> + 'a>
    ParseableParametrized<NeighbourTableMessageBuffer<&'a T>, NlaParseMode>
    for Vec<NeighbourTableAttribute>
{
    type Error = NeighbourTableError;
    fn parse_with_param(
        buf: &NeighbourTableMessageBuffer<&'a T>,
        mode: NlaParseMode,
    ) -> Result<Self, NeighbourTableError> {
        buf.parse_attributes(mode, NeighbourTableAttribute::parse)
    }
}
