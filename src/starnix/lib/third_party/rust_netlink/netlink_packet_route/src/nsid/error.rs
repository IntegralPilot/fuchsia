// SPDX-License-Identifier: MIT

use netlink_packet_utils::DecodeError;
use netlink_packet_utils::nla::NlaError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum NsidError {
    #[error("Invalid {kind}")]
    InvalidValue { kind: &'static str, error: DecodeError },

    #[error("Unknown NLA type: {kind}")]
    UnknownNLA { kind: u16, error: DecodeError },

    #[error(transparent)]
    ParseNla(#[from] NlaError),
}
