// SPDX-License-Identifier: MIT

use super::{TcAttribute, TcError, TcHeader, TcMessageBuffer};
use crate::RouteNetlinkMessageParseMode;
use netlink_packet_utils::nla::{HasNlas, NlaParseMode};
use netlink_packet_utils::traits::{Emitable, Parseable, ParseableParametrized};

#[derive(Debug, PartialEq, Eq, Clone, Default)]
#[non_exhaustive]
pub struct TcMessage {
    pub header: TcHeader,
    pub attributes: Vec<TcAttribute>,
}

impl TcMessage {
    pub fn into_parts(self) -> (TcHeader, Vec<TcAttribute>) {
        (self.header, self.attributes)
    }

    pub fn from_parts(header: TcHeader, attributes: Vec<TcAttribute>) -> Self {
        TcMessage { header, attributes }
    }

    /// Create a new `TcMessage` with the given index
    pub fn with_index(index: i32) -> Self {
        Self { header: TcHeader { index, ..Default::default() }, attributes: Vec::new() }
    }
}

impl<'a, T: AsRef<[u8]> + 'a>
    ParseableParametrized<TcMessageBuffer<&'a T>, RouteNetlinkMessageParseMode> for TcMessage
{
    type Error = TcError;
    fn parse_with_param(
        buf: &TcMessageBuffer<&'a T>,
        mode: RouteNetlinkMessageParseMode,
    ) -> Result<Self, TcError> {
        Ok(Self {
            header: TcHeader::parse(buf).unwrap(),
            attributes: Vec::<TcAttribute>::parse_with_param(buf, mode.into())?,
        })
    }
}

impl<'a, T: AsRef<[u8]> + 'a> ParseableParametrized<TcMessageBuffer<&'a T>, NlaParseMode>
    for Vec<TcAttribute>
{
    type Error = TcError;
    fn parse_with_param(buf: &TcMessageBuffer<&'a T>, mode: NlaParseMode) -> Result<Self, TcError> {
        let mut kind = String::new();
        buf.parse_attributes(mode, |nla_buf| {
            let attribute = TcAttribute::parse_with_param(nla_buf, kind.as_str())?;
            if let TcAttribute::Kind(s) = &attribute {
                kind = s.to_string();
            }
            Ok(attribute)
        })
    }
}

impl Emitable for TcMessage {
    fn buffer_len(&self) -> usize {
        self.header.buffer_len() + self.attributes.as_slice().buffer_len()
    }

    fn emit(&self, buffer: &mut [u8]) {
        self.header.emit(buffer);
        self.attributes.as_slice().emit(&mut buffer[self.header.buffer_len()..]);
    }
}
