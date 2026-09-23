// SPDX-License-Identifier: MIT

use netlink_packet_utils::nla::HasNlas;
use netlink_packet_utils::traits::{Emitable, Parseable, ParseableParametrized};
use std::convert::TryFrom as _;

use super::NeighbourDiscoveryUserOptionError;
use super::buffer::{
    NEIGHBOUR_DISCOVERY_USER_OPTION_HEADER_LEN, NeighbourDiscoveryUserOptionMessageBuffer,
};
use super::header::NeighbourDiscoveryUserOptionHeader;
use super::nla::Nla;
use crate::RouteNetlinkMessageParseMode;

#[derive(Debug, PartialEq, Eq, Clone)]
#[non_exhaustive]
pub struct NeighbourDiscoveryUserOptionMessage {
    /// The header of the ND_USEROPT message.
    pub header: NeighbourDiscoveryUserOptionHeader,

    /// The body of the NDP option as it was on the wire.
    pub option_body: Vec<u8>,

    pub attributes: Vec<Nla>,
}

impl NeighbourDiscoveryUserOptionMessage {
    pub fn new(
        header: NeighbourDiscoveryUserOptionHeader,
        option_body: Vec<u8>,
        attributes: Vec<Nla>,
    ) -> Self {
        Self { header, option_body, attributes }
    }
}

impl Emitable for NeighbourDiscoveryUserOptionMessage {
    fn buffer_len(&self) -> usize {
        NEIGHBOUR_DISCOVERY_USER_OPTION_HEADER_LEN
            + self.option_body.len()
            + self.attributes.as_slice().buffer_len()
    }

    fn emit(&self, buffer: &mut [u8]) {
        let Self {
            header: NeighbourDiscoveryUserOptionHeader { interface_index, icmp_type },
            option_body,
            attributes,
        } = self;

        let mut packet = NeighbourDiscoveryUserOptionMessageBuffer::new_unchecked(buffer);

        packet.set_address_family(icmp_type.family().into());

        let payload = packet.payload_mut();
        payload[..option_body.len()].copy_from_slice(&option_body[..]);
        attributes.as_slice().emit(&mut payload[option_body.len()..]);

        packet.set_options_length(
            u16::try_from(option_body.len())
                .expect("neighbor discovery options length doesn't fit in u16"),
        );
        packet.set_interface_index(*interface_index);

        let (icmp_type, icmp_code) = icmp_type.into_type_and_code();
        packet.set_icmp_type(icmp_type);
        packet.set_icmp_code(icmp_code);
    }
}

impl<'a, T: AsRef<[u8]> + 'a>
    ParseableParametrized<
        NeighbourDiscoveryUserOptionMessageBuffer<&'a T>,
        RouteNetlinkMessageParseMode,
    > for NeighbourDiscoveryUserOptionMessage
{
    type Error = NeighbourDiscoveryUserOptionError;

    fn parse_with_param(
        buf: &NeighbourDiscoveryUserOptionMessageBuffer<&'a T>,
        mode: RouteNetlinkMessageParseMode,
    ) -> Result<Self, NeighbourDiscoveryUserOptionError> {
        let header = NeighbourDiscoveryUserOptionHeader::parse(buf)
            .map_err(NeighbourDiscoveryUserOptionError::InvalidHeader)?;

        let attributes = buf
            .parse_attributes(mode.into(), Nla::parse)
            .map_err(NeighbourDiscoveryUserOptionError::InvalidNla)?;

        Ok(NeighbourDiscoveryUserOptionMessage {
            header,
            option_body: buf.option_body().to_vec(),
            attributes,
        })
    }
}
