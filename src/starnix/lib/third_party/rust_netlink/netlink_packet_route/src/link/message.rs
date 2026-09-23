// SPDX-License-Identifier: MIT

use anyhow::Context;
use netlink_packet_utils::DecodeError;
use netlink_packet_utils::traits::{Emitable, Parseable, ParseableParametrized};

use crate::link::{LinkAttribute, LinkHeader, LinkMessageBuffer};
use crate::{AddressFamily, RouteNetlinkMessageParseMode};
use netlink_packet_utils::nla::{HasNlas, NlaParseMode};

#[derive(Debug, PartialEq, Eq, Clone, Default)]
#[non_exhaustive]
pub struct LinkMessage {
    pub header: LinkHeader,
    pub attributes: Vec<LinkAttribute>,
}

impl Emitable for LinkMessage {
    fn buffer_len(&self) -> usize {
        self.header.buffer_len() + self.attributes.as_slice().buffer_len()
    }

    fn emit(&self, buffer: &mut [u8]) {
        self.header.emit(buffer);
        self.attributes.as_slice().emit(&mut buffer[self.header.buffer_len()..]);
    }
}

impl<'a, T: AsRef<[u8]> + 'a>
    ParseableParametrized<LinkMessageBuffer<&'a T>, RouteNetlinkMessageParseMode> for LinkMessage
{
    type Error = DecodeError;
    fn parse_with_param(
        buf: &LinkMessageBuffer<&'a T>,
        mode: RouteNetlinkMessageParseMode,
    ) -> Result<Self, DecodeError> {
        let header = LinkHeader::parse(buf).context("failed to parse link message header")?;
        let interface_family = header.interface_family;
        let attributes =
            Vec::<LinkAttribute>::parse_with_param(buf, (mode.into(), interface_family))
                .context("failed to parse link message NLAs")?;
        Ok(LinkMessage { header, attributes })
    }
}

impl<'a, T: AsRef<[u8]> + 'a>
    ParseableParametrized<LinkMessageBuffer<&'a T>, (NlaParseMode, AddressFamily)>
    for Vec<LinkAttribute>
{
    type Error = DecodeError;
    fn parse_with_param(
        buf: &LinkMessageBuffer<&'a T>,
        (mode, family): (NlaParseMode, AddressFamily),
    ) -> Result<Self, DecodeError> {
        buf.parse_attributes(mode, |nla_buf| LinkAttribute::parse_with_param(nla_buf, family))
    }
}
