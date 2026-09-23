// SPDX-License-Identifier: MIT

use netlink_packet_utils::DecodeError;
use thiserror::Error;

mod buffer;
mod constants;
mod header;
mod message;
mod nla;
#[cfg(test)]
mod tests;

pub use self::buffer::NeighbourDiscoveryUserOptionMessageBuffer;
pub use self::constants::*;
pub use self::header::{
    NeighbourDiscoveryIcmpType, NeighbourDiscoveryIcmpV6Type, NeighbourDiscoveryUserOptionHeader,
};
pub use self::message::NeighbourDiscoveryUserOptionMessage;
pub use self::nla::Nla;

#[derive(Error, Debug)]
#[non_exhaustive]
pub enum NeighbourDiscoveryUserOptionError {
    #[error("Failed to initialize nduseropt message buffer: {0}")]
    ParseBuffer(#[source] DecodeError),
    #[error("Invalid nduseropt message header: {0}")]
    InvalidHeader(#[source] DecodeError),
    #[error("Invalid NLA in nduseropt message: {0}")]
    InvalidNla(#[source] DecodeError),
}
