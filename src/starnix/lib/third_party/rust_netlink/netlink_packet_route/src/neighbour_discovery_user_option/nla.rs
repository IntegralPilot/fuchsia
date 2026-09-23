// SPDX-License-Identifier: MIT

use anyhow::Context;

use netlink_packet_utils::DecodeError;
use netlink_packet_utils::nla::{self, DefaultNla, NlaBuffer};
use netlink_packet_utils::traits::Parseable;

use super::constants;

/// Neighbor discovery options, which may be included as attributes for
/// `RTM_NEWNDUSEROPT` messages.
#[derive(Debug, PartialEq, Eq, Clone)]
#[non_exhaustive]
pub enum Nla {
    /// The source link-layer address, as defined by RFC 2461 section 4.6.1.
    SourceLinkLocalAddress(Vec<u8>),
    Other(DefaultNla),
}

impl nla::Nla for Nla {
    fn value_len(&self) -> usize {
        use self::Nla::*;
        match *self {
            SourceLinkLocalAddress(ref bytes) => bytes.len(),
            Other(ref attr) => attr.value_len(),
        }
    }

    fn emit_value(&self, buffer: &mut [u8]) {
        use self::Nla::*;
        match *self {
            SourceLinkLocalAddress(ref bytes) => buffer.copy_from_slice(bytes.as_slice()),
            Other(ref attr) => attr.emit_value(buffer),
        }
    }

    fn kind(&self) -> u16 {
        use self::Nla::*;
        use constants::*;
        match *self {
            SourceLinkLocalAddress(_) => ND_OPT_SOURCE_LL_ADDR,
            Other(ref attr) => attr.kind(),
        }
    }
}

impl<'a, T: AsRef<[u8]> + ?Sized> Parseable<NlaBuffer<&'a T>> for Nla {
    type Error = DecodeError;

    fn parse(buf: &NlaBuffer<&'a T>) -> Result<Self, DecodeError> {
        use self::Nla::*;

        let payload = buf.value();
        Ok(match buf.kind() {
            constants::ND_OPT_SOURCE_LL_ADDR => SourceLinkLocalAddress(payload.to_vec()),
            _ => Other(DefaultNla::parse(buf).context("invalid NLA (unknown kind)")?),
        })
    }
}
