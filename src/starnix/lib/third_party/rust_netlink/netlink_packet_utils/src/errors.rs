// SPDX-License-Identifier: MIT

use anyhow::anyhow;
use thiserror::Error;

#[derive(Debug, Error)]
#[error("Encode error occurred: {inner}")]
pub struct EncodeError {
    inner: anyhow::Error,
}

impl From<&'static str> for EncodeError {
    fn from(msg: &'static str) -> Self {
        EncodeError { inner: anyhow!(msg) }
    }
}

impl From<String> for EncodeError {
    fn from(msg: String) -> Self {
        EncodeError { inner: anyhow!(msg) }
    }
}

impl From<anyhow::Error> for EncodeError {
    fn from(inner: anyhow::Error) -> EncodeError {
        EncodeError { inner }
    }
}

#[derive(Debug, Error)]
pub enum DecodeError {
    #[error("Invalid MAC address")]
    InvalidMACAddress,

    #[error("Invalid IP address")]
    InvalidIPAddress,

    #[error("Invalid string")]
    Utf8Error(#[from] std::string::FromUtf8Error),

    #[error("Invalid u8")]
    InvalidU8,

    #[error("Invalid u16")]
    InvalidU16,

    #[error("Invalid u32")]
    InvalidU32,

    #[error("Invalid u64")]
    InvalidU64,

    #[error("Invalid u128")]
    InvalidU128,

    #[error("Invalid i32")]
    InvalidI32,

    #[error("Invalid {name}: length {len} < {buffer_len}")]
    InvalidBufferLength { name: &'static str, len: usize, buffer_len: usize },

    #[error(transparent)]
    Nla(#[from] crate::nla::NlaError),

    #[error(transparent)]
    Other(#[from] anyhow::Error),

    #[error("Failed to parse NLMSG_ERROR: {0}")]
    FailedToParseNlMsgError(Box<DecodeError>),

    #[error("Failed to parse NLMSG_DONE: {0}")]
    FailedToParseNlMsgDone(Box<DecodeError>),

    #[error("Failed to parse message with type {message_type}: {source}")]
    FailedToParseMessageWithType { message_type: u16, source: Box<DecodeError> },

    #[error("Failed to parse netlink header: {0}")]
    FailedToParseNetlinkHeader(Box<DecodeError>),
}

impl From<&'static str> for DecodeError {
    fn from(msg: &'static str) -> Self {
        DecodeError::Other(anyhow!(msg))
    }
}

impl From<String> for DecodeError {
    fn from(msg: String) -> Self {
        DecodeError::Other(anyhow!(msg))
    }
}
