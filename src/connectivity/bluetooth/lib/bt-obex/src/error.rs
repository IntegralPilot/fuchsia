// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::format_err;

use objects::ObexObjectError;
use thiserror::Error;

use crate::header::HeaderIdentifier;
use crate::operation::{OpCode, ResponseCode};

/// Errors that occur during the use of the OBEX library.
#[derive(Error, Debug)]
pub enum Error {
    #[error("Error parsing packet: {:?}", .0)]
    Packet(#[from] PacketError),
    #[error("Header {:?} already exists in HeaderSet", .0)]
    AlreadyExists(HeaderIdentifier),
    #[error("{:?} cannot be added with {:?}", .0, .1)]
    IncompatibleHeaders(HeaderIdentifier, HeaderIdentifier),
    #[error("Encountered an IO Error: {}", .0)]
    IOError(#[from] zx::Status),
    #[error("Invalid OBEX object: {:?}", .0)]
    Object(#[from] ObexObjectError),
    #[error("Operation is already in progress")]
    OperationInProgress,
    #[error("Internal error during {:?}: {:?}", .operation, .msg)]
    OperationError { operation: OpCode, msg: String },
    #[error("Single Response Mode is not supported")]
    SrmNotSupported,
    #[error("Peer disconnected")]
    PeerDisconnected,
    #[error("Peer rejected {:?} request with Error: {:?}", .operation, .response)]
    PeerRejected { operation: OpCode, response: ResponseCode },
    #[error("Invalid {:?} response from peer: {:?}", .operation, .msg)]
    PeerResponse { operation: OpCode, msg: String },
    #[error("{:?} is not implemented", .operation)]
    NotImplemented { operation: OpCode },
    #[error("Packet size ({}) exceeds the negotiated maximum ({})", .size, .max)]
    PacketTooLarge { size: usize, max: usize },
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl Error {
    pub fn peer_rejected(operation: OpCode, response: ResponseCode) -> Self {
        Self::PeerRejected { operation, response }
    }

    pub fn response(operation: OpCode, msg: impl Into<String>) -> Self {
        Self::PeerResponse { operation, msg: msg.into() }
    }

    pub fn operation(operation: OpCode, msg: impl Into<String>) -> Self {
        Self::OperationError { operation, msg: msg.into() }
    }

    pub fn not_implemented(operation: OpCode) -> Self {
        Self::NotImplemented { operation }
    }

    pub fn packet_too_large(size: usize, max: usize) -> Self {
        Self::PacketTooLarge { size, max }
    }

    pub fn other(msg: impl Into<String>) -> Self {
        Self::Other(format_err!(msg.into()).into())
    }
}

/// Errors that occur during the encoding & decoding of OBEX packets.
#[derive(Error, Debug)]
pub enum PacketError {
    #[error("Buffer is too small")]
    BufferTooSmall,
    #[error("Invalid data length")]
    DataLength,
    #[error("Invalid data: {}", .0)]
    Data(String),
    #[error("Invalid header identifier: {}", .0)]
    Identifier(u8),
    #[error("Invalid packet OpCode: {}", .0)]
    OpCode(u8),
    #[error("Invalid header encoding")]
    HeaderEncoding,
    #[error("Invalid response code: {}", .0)]
    ResponseCode(u8),
    #[error("Field is RFA.")]
    Reserved,
    /// An error from another source
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl PacketError {
    pub fn external(e: impl Into<anyhow::Error>) -> Self {
        Self::Other(e.into())
    }

    pub fn data(e: impl Into<String>) -> Self {
        Self::Data(e.into())
    }
}
