// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Error (common to all fidl operations)

use crate::handle::{ObjectType, Rights};
use std::sync::Arc;
use zx_status::Status;

/// A specialized `Result` type for FIDL operations.
pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Clone, thiserror::Error)]
#[allow(missing_docs)]
pub enum TransportError {
    #[error(transparent)]
    Status(#[from] Status),
    #[error(transparent)]
    Other(Arc<dyn std::error::Error + Send + Sync>),
}

// This prevents some binary size bloat.
impl Drop for TransportError {
    #[inline(never)]
    fn drop(&mut self) {}
}

/// Represents the closure condition of a FIDL channel, either via an explicit epitaph or a direct
/// peer closure.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Epitaph {
    /// The channel was closed with an explicit epitaph status (`Ok(())` for `ZX_OK` success, or
    /// `Err(Status)` for an error).
    Explicit(std::result::Result<(), zx_status::Status>),
    /// The channel was closed without an epitaph message (`PEER_CLOSED`).
    PeerClosed,
}

impl std::fmt::Display for Epitaph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Epitaph::Explicit(Ok(())) => write!(f, "ZX_OK (0)"),
            Epitaph::Explicit(Err(s)) => write!(f, "{}", s),
            Epitaph::PeerClosed => write!(f, "{}", zx_status::Status::PEER_CLOSED),
        }
    }
}

impl std::error::Error for Epitaph {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Epitaph::Explicit(Ok(())) | Epitaph::PeerClosed => None,
            Epitaph::Explicit(Err(s)) => Some(s),
        }
    }
}

impl Epitaph {
    /// Returns `true` if an explicit epitaph was received or set (`Explicit`).
    pub fn is_explicit(&self) -> bool {
        matches!(self, Epitaph::Explicit(_))
    }

    /// Returns `true` if the channel was closed without an epitaph (`PEER_CLOSED`)
    /// or with an explicit `PEER_CLOSED` epitaph.
    pub fn is_peer_closed(&self) -> bool {
        matches!(self, Epitaph::PeerClosed | Epitaph::Explicit(Err(zx_status::Status::PEER_CLOSED)))
    }

    /// Returns `true` if an explicit `ZX_OK` (`Ok(())`) epitaph was received.
    pub fn is_ok(&self) -> bool {
        matches!(self, Epitaph::Explicit(Ok(())))
    }
}

impl PartialEq<zx_status::Status> for Epitaph {
    fn eq(&self, other: &zx_status::Status) -> bool {
        match self {
            Epitaph::Explicit(Ok(())) => zx_status::Status::ok(other.into_raw()).is_ok(),
            Epitaph::Explicit(Err(s)) => s == other,
            Epitaph::PeerClosed => *other == zx_status::Status::PEER_CLOSED,
        }
    }
}

impl PartialEq<Epitaph> for zx_status::Status {
    fn eq(&self, other: &Epitaph) -> bool {
        other.eq(self)
    }
}

impl PartialEq<std::result::Result<(), zx_status::Status>> for Epitaph {
    fn eq(&self, other: &std::result::Result<(), zx_status::Status>) -> bool {
        let res: std::result::Result<(), zx_status::Status> = (*self).into();
        &res == other
    }
}

impl PartialEq<Epitaph> for std::result::Result<(), zx_status::Status> {
    fn eq(&self, other: &Epitaph) -> bool {
        other.eq(self)
    }
}

impl From<Epitaph> for std::result::Result<(), zx_status::Status> {
    fn from(epitaph: Epitaph) -> Self {
        match epitaph {
            Epitaph::Explicit(res) => res,
            Epitaph::PeerClosed => Err(zx_status::Status::PEER_CLOSED),
        }
    }
}

impl From<&Epitaph> for std::result::Result<(), zx_status::Status> {
    fn from(epitaph: &Epitaph) -> Self {
        (*epitaph).into()
    }
}

impl From<zx_status::Status> for Epitaph {
    fn from(status: zx_status::Status) -> Self {
        if zx_status::Status::ok(status.into_raw()).is_ok() {
            Epitaph::Explicit(Ok(()))
        } else {
            Epitaph::Explicit(Err(status))
        }
    }
}

impl From<std::result::Result<(), zx_status::Status>> for Epitaph {
    fn from(res: std::result::Result<(), zx_status::Status>) -> Self {
        Epitaph::Explicit(res)
    }
}

/// The error type used by FIDL operations.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
#[allow(missing_docs)]
pub enum Error {
    #[error("Unexpected response to synchronous FIDL query.")]
    UnexpectedSyncResponse,

    #[error("Invalid FIDL boolean.")]
    InvalidBoolean,

    #[error("Invalid header for a FIDL buffer.")]
    InvalidHeader,

    #[error("Incompatible wire format magic number: {0}.")]
    IncompatibleMagicNumber(u8),

    #[error("Unsupported wire format version")]
    UnsupportedWireFormatVersion,

    #[error("Invalid FIDL buffer.")]
    Invalid,

    #[error(
        "The FIDL object of size {expected} could not fit within the provided \
        buffer range of size {actual}."
    )]
    OutOfRange { expected: usize, actual: usize },

    #[error(
        "The FIDL object requested more handles during decoding than were \
        provided with the message."
    )]
    OutOfHandles,

    #[error("Decoding the FIDL object did not use all of the bytes provided.")]
    ExtraBytes,

    #[error("Decoding the FIDL object did not use all of the handles provided.")]
    ExtraHandles,

    #[error(
        "Decoding the FIDL object observed non-zero value in the padding region \
        starting at byte {padding_start}."
    )]
    NonZeroPadding {
        /// Index of the first byte of the padding, relative to the beginning of the message.
        padding_start: usize,
    },

    #[error("The FIDL object had too many layers of out-of-line recursion.")]
    MaxRecursionDepth,

    #[error(
        "There was an attempt to read or write a null-valued object as a non-nullable FIDL type."
    )]
    NotNullable,

    #[error("A FIDL object reference with nonzero byte length had a null data pointer.")]
    UnexpectedNullRef,

    #[error("A FIDL message contained incorrectly encoded UTF8.")]
    Utf8Error,

    #[error("Vector was too long. Expected at most {max_length} elements, got {actual_length}.")]
    VectorTooLong {
        /// Maximum length, i.e. the `N` in `vector<T>:N`.
        max_length: usize,
        /// Actual length of the vector (number of elements).
        actual_length: usize,
    },

    #[error("String was too long. Expected at most {max_bytes} bytes, got {actual_bytes}.")]
    StringTooLong {
        /// Maximum length in bytes, i.e. the `N` in `string:N`.
        max_bytes: usize,
        /// Actual length of the string in bytes.
        actual_bytes: usize,
    },

    #[error(
        "A message was received for ordinal value {ordinal} that the FIDL \
        protocol {protocol_name} does not understand."
    )]
    UnknownOrdinal { ordinal: u64, protocol_name: &'static str },

    #[error("Server for the FIDL protocol {protocol_name} did not recognize method {method_name}.")]
    UnsupportedMethod { method_name: &'static str, protocol_name: &'static str },

    #[error("Invalid bits value for a strict bits type.")]
    InvalidBitsValue,

    #[error("Invalid enum value for a strict enum type.")]
    InvalidEnumValue,

    #[error("Unrecognized descriminant for a FIDL union type.")]
    UnknownUnionTag,

    #[error("A FIDL future was polled after it had already completed.")]
    PollAfterCompletion,

    #[error(
        "Received request with zero txid for two-way method ordinal, \
        or nonzero txid for one-way method ordinal."
    )]
    InvalidRequestTxid,

    #[error("Received response with unknown txid.")]
    InvalidResponseTxid,

    #[error("Received response with unexpected ordinal.")]
    InvalidResponseOrdinal,

    #[error("Invalid presence indicator.")]
    InvalidPresenceIndicator,

    #[error("Invalid inline bit in envelope.")]
    InvalidInlineBitInEnvelope,

    #[error("Invalid inline marker in envelope.")]
    InvalidInlineMarkerInEnvelope,

    #[error("Invalid number of bytes in FIDL envelope.")]
    InvalidNumBytesInEnvelope,

    #[error("Invalid number of handles in FIDL envelope.")]
    InvalidNumHandlesInEnvelope,

    #[error("Invalid FIDL handle used on the host.")]
    InvalidHostHandle,

    #[error("Incorrect handle subtype. Expected {}, but received {}", .expected.into_raw(), .received.into_raw())]
    IncorrectHandleSubtype { expected: ObjectType, received: ObjectType },

    #[error("Some expected handle rights are missing: {}", .missing_rights.bits())]
    MissingExpectedHandleRights { missing_rights: Rights },

    #[error("An error was encountered during handle replace()")]
    HandleReplace(#[source] Status),

    #[error("A server encountered an IO error writing a FIDL response to a channel: {0}")]
    ServerResponseWrite(#[source] TransportError),

    #[error(
        "A FIDL server encountered an IO error reading incoming FIDL requests from a channel: {0}"
    )]
    ServerRequestRead(#[source] TransportError),

    #[error("A FIDL server encountered an IO error writing an epitaph into a channel: {0}")]
    ServerEpitaphWrite(#[source] TransportError),

    #[error("A FIDL client encountered an IO error reading a response from a channel: {0}")]
    ClientRead(#[source] TransportError),

    #[error("A FIDL client encountered an IO error writing a request into a channel: {0}")]
    ClientWrite(#[source] TransportError),

    #[error("A FIDL client encountered an IO error issuing a channel call: {0}")]
    ClientCall(#[source] Status),

    #[error("A FIDL client encountered an IO error issuing a channel call: {0}")]
    ClientEvent(#[source] Status),

    #[cfg(not(target_os = "fuchsia"))]
    #[error(
        "A FIDL client's channel to the protocol {protocol_name} was closed: {epitaph}, reason: {}",
        .reason.as_ref().map(String::as_str).unwrap_or("not given")
    )]
    ClientChannelClosed {
        /// The epitaph or `Epitaph::PeerClosed`.
        #[source]
        epitaph: Epitaph,
        /// The name of the protocol at the other end of the channel.
        protocol_name: &'static str,
        /// Further details on why exactly the channel closed.
        reason: Option<String>,
    },

    #[cfg(target_os = "fuchsia")]
    #[error("A FIDL client's channel to the protocol {protocol_name} was closed: {epitaph}")]
    ClientChannelClosed {
        /// The epitaph or `Epitaph::PeerClosed`.
        #[source]
        epitaph: Epitaph,
        /// The name of the protocol at the other end of the channel.
        protocol_name: &'static str,
    },

    #[error("There was an error attaching a FIDL channel to the async executor: {0}")]
    AsyncChannel(#[source] Status),

    #[cfg(target_os = "fuchsia")]
    #[cfg(test)]
    #[error("Test Status: {0}")]
    TestIo(#[source] Status),
}

impl Error {
    /// Returns `true` if the error was sourced by a closed channel.
    pub fn is_closed(&self) -> bool {
        matches!(self, Error::ClientChannelClosed { .. })
    }
}
