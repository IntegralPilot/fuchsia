// SPDX-License-Identifier: MIT

use netlink_packet_utils::DecodeError;
use netlink_packet_utils::nla::NlaError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PrefixError {
    #[error("Invalid PREFIX_ADDRESS, unexpected payload length: {payload_length}")]
    InvalidPrefixAddress { payload_length: usize },

    #[error("Invalid PREFIX_CACHEINFO: {0:?}")]
    InvalidPrefixCacheInfo(DecodeError),

    #[error(transparent)]
    ParseNla(#[from] NlaError),

    #[error(transparent)]
    Other(#[from] DecodeError),
}
