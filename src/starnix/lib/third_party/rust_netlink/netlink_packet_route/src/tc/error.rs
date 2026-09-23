// SPDX-License-Identifier: MIT

use netlink_packet_utils::DecodeError;
use netlink_packet_utils::nla::NlaError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TcError {
    #[error("Invalid {kind}")]
    InvalidValue {
        kind: &'static str,
        #[source]
        error: DecodeError,
    },

    #[error("failed to parse {kind} TCA_OPTIONS attributes")]
    ParseTcaOptionAttributes {
        kind: &'static str,
        #[source]
        error: DecodeError,
    },

    #[error("failed to parse {kind}")]
    ParseFilterMatchallOption {
        kind: &'static str,
        #[source]
        error: DecodeError,
    },

    #[error("failed to parse {kind}")]
    ParseAction {
        kind: &'static str,
        #[source]
        error: DecodeError,
    },

    #[error("failed to parse TCA_ACT_OPTIONS for kind {kind}")]
    ParseActOptions {
        kind: String,
        #[source]
        error: DecodeError,
    },

    #[error("failed to parse mirror action")]
    ParseMirrorAction(#[source] DecodeError),

    #[error("Unknown matchall option: {kind}")]
    UnknownFilterMatchAllOption {
        kind: String,
        #[source]
        error: DecodeError,
    },

    #[error("Unknown NLA type: {kind}")]
    UnknownNla {
        kind: u16,
        #[source]
        error: DecodeError,
    },

    #[error("Unknown TC_OPTIONS: {kind}")]
    UnknownOption {
        kind: String,
        #[source]
        error: DecodeError,
    },

    #[error(transparent)]
    ParseNla(#[from] NlaError),

    #[error("failed to parse TCA_STATS2 for kind {kind}")]
    ParseTcaStats2 {
        kind: String,
        #[source]
        error: DecodeError,
    },

    #[error("failed to parse TCA_STATS2 attribute {kind}")]
    ParseTcaStats2Attribute {
        kind: &'static str,
        #[source]
        error: DecodeError,
    },

    #[error("failed to parse TC_FQ_CODEL_QD_STATS option {kind}")]
    ParseFqCodelXstatsOption {
        kind: &'static str,
        #[source]
        error: DecodeError,
    },

    #[error("Invalid u32 key")]
    InvalidU32Key(#[source] DecodeError),

    #[error("Invalid TcFqCodelXstats length: {0}")]
    InvalidXstatsLength(usize),
}
