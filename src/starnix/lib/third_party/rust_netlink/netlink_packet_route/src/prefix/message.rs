// SPDX-License-Identifier: MIT

use super::attribute::PrefixAttribute;
use super::error::PrefixError;
use super::header::{PrefixHeader, PrefixMessageBuffer};
use crate::RouteNetlinkMessageParseMode;
use netlink_packet_utils::nla::{HasNlas, NlaParseMode};
use netlink_packet_utils::traits::{Emitable, Parseable, ParseableParametrized};

#[derive(Debug, PartialEq, Eq, Clone, Default)]
pub struct PrefixMessage {
    pub header: PrefixHeader,
    pub attributes: Vec<PrefixAttribute>,
}

impl Emitable for PrefixMessage {
    fn buffer_len(&self) -> usize {
        self.header.buffer_len() + self.attributes.as_slice().buffer_len()
    }

    fn emit(&self, buffer: &mut [u8]) {
        self.header.emit(buffer);
        self.attributes.as_slice().emit(&mut buffer[self.header.buffer_len()..]);
    }
}

impl<T: AsRef<[u8]>> Parseable<PrefixMessageBuffer<T>> for PrefixHeader {
    type Error = ();
    fn parse(buf: &PrefixMessageBuffer<T>) -> Result<Self, ()> {
        Ok(Self {
            prefix_family: buf.prefix_family(),
            ifindex: buf.ifindex(),
            prefix_type: buf.prefix_type(),
            prefix_len: buf.prefix_len(),
            flags: buf.flags(),
        })
    }
}

impl<'a, T: AsRef<[u8]> + 'a>
    ParseableParametrized<PrefixMessageBuffer<&'a T>, RouteNetlinkMessageParseMode>
    for PrefixMessage
{
    type Error = PrefixError;
    fn parse_with_param(
        buf: &PrefixMessageBuffer<&'a T>,
        mode: RouteNetlinkMessageParseMode,
    ) -> Result<Self, PrefixError> {
        Ok(Self {
            // Unwrap: ok, we never return an error above.
            header: PrefixHeader::parse(buf).unwrap(),
            attributes: Vec::<PrefixAttribute>::parse_with_param(buf, mode.into())?,
        })
    }
}

impl<'a, T: AsRef<[u8]> + 'a> ParseableParametrized<PrefixMessageBuffer<&'a T>, NlaParseMode>
    for Vec<PrefixAttribute>
{
    type Error = PrefixError;
    fn parse_with_param(
        buf: &PrefixMessageBuffer<&'a T>,
        mode: NlaParseMode,
    ) -> Result<Self, PrefixError> {
        buf.parse_attributes(mode, PrefixAttribute::parse)
    }
}
