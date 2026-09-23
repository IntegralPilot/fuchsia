// SPDX-License-Identifier: MIT

use netlink_packet_utils::DecodeError;
use netlink_packet_utils::nla::NlaError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum NeighbourTableError {
    #[error("Invalid {kind}")]
    InvalidValue { kind: &'static str, error: DecodeError },

    #[error("Invalid {kind} value")]
    InvalidParameter { kind: &'static str, error: DecodeError },

    #[error("Unknown NLA type: {kind}")]
    UnknownNla { kind: u16, error: DecodeError },

    #[error(transparent)]
    ParseNla(#[from] NlaError),
}
