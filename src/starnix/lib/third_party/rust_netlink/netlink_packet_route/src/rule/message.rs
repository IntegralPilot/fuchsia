// SPDX-License-Identifier: MIT

use super::{RuleAttribute, RuleError, RuleHeader, RuleMessageBuffer};
use crate::RouteNetlinkMessageParseMode;
use netlink_packet_utils::ParseableParametrized;
use netlink_packet_utils::nla::{HasNlas, NlaParseMode};
use netlink_packet_utils::traits::{Emitable, Parseable};

#[derive(Debug, PartialEq, Eq, Clone, Default)]
#[non_exhaustive]
pub struct RuleMessage {
    pub header: RuleHeader,
    pub attributes: Vec<RuleAttribute>,
}

impl Emitable for RuleMessage {
    fn buffer_len(&self) -> usize {
        self.header.buffer_len() + self.attributes.as_slice().buffer_len()
    }

    fn emit(&self, buffer: &mut [u8]) {
        self.header.emit(buffer);
        self.attributes.as_slice().emit(&mut buffer[self.header.buffer_len()..]);
    }
}

impl<'a, T: AsRef<[u8]> + 'a>
    ParseableParametrized<RuleMessageBuffer<&'a T>, RouteNetlinkMessageParseMode> for RuleMessage
{
    type Error = RuleError;
    fn parse_with_param(
        buf: &RuleMessageBuffer<&'a T>,
        mode: RouteNetlinkMessageParseMode,
    ) -> Result<Self, RuleError> {
        // unwrap: RuleHeader never fails to parse.
        let header = RuleHeader::parse(buf).unwrap();
        let attributes = Vec::<RuleAttribute>::parse_with_param(buf, mode.into())?;
        Ok(RuleMessage { header, attributes })
    }
}

impl<'a, T: AsRef<[u8]> + 'a> ParseableParametrized<RuleMessageBuffer<&'a T>, NlaParseMode>
    for Vec<RuleAttribute>
{
    type Error = RuleError;
    fn parse_with_param(
        buf: &RuleMessageBuffer<&'a T>,
        mode: NlaParseMode,
    ) -> Result<Self, RuleError> {
        buf.parse_attributes(mode, RuleAttribute::parse)
    }
}
