// SPDX-License-Identifier: MIT

use crate::RouteNetlinkMessageParseMode;
use crate::nsid::{NsidAttribute, NsidError, NsidHeader, NsidMessageBuffer};
use netlink_packet_utils::nla::{HasNlas, NlaParseMode};
use netlink_packet_utils::traits::{Emitable, Parseable, ParseableParametrized};

#[derive(Debug, PartialEq, Eq, Clone, Default)]
#[non_exhaustive]
pub struct NsidMessage {
    pub header: NsidHeader,
    pub attributes: Vec<NsidAttribute>,
}

impl<'a, T: AsRef<[u8]> + 'a>
    ParseableParametrized<NsidMessageBuffer<&'a T>, RouteNetlinkMessageParseMode> for NsidMessage
{
    type Error = NsidError;
    fn parse_with_param(
        buf: &NsidMessageBuffer<&'a T>,
        mode: RouteNetlinkMessageParseMode,
    ) -> Result<Self, NsidError> {
        Ok(Self {
            // unwrap: parsing the header can't fail
            header: NsidHeader::parse(buf).unwrap(),
            attributes: Vec::<NsidAttribute>::parse_with_param(buf, mode.into())?,
        })
    }
}

impl<'a, T: AsRef<[u8]> + 'a> ParseableParametrized<NsidMessageBuffer<&'a T>, NlaParseMode>
    for Vec<NsidAttribute>
{
    type Error = NsidError;
    fn parse_with_param(
        buf: &NsidMessageBuffer<&'a T>,
        mode: NlaParseMode,
    ) -> Result<Self, NsidError> {
        buf.parse_attributes(mode, NsidAttribute::parse)
    }
}

impl Emitable for NsidMessage {
    fn buffer_len(&self) -> usize {
        self.header.buffer_len() + self.attributes.as_slice().buffer_len()
    }

    fn emit(&self, buffer: &mut [u8]) {
        self.header.emit(buffer);
        self.attributes.as_slice().emit(&mut buffer[self.header.buffer_len()..]);
    }
}
