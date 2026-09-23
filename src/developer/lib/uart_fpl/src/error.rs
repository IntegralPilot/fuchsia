// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Error types for the UART framing and protocol library.

use thiserror::Error;

/// Errors that can occur during Channel 0 protocol negotiation.
#[derive(Debug, Error, PartialEq, Eq, Clone, Copy)]
pub enum HandshakeError {
    /// Handshake payload was malformed, truncated, or contained invalid fields.
    #[error("Malformed or truncated handshake payload")]
    MalformedPayload,

    /// Selected protocol was not among the proposed protocols.
    #[error("Selected protocol was not proposed by host")]
    ProtocolNotProposed,

    /// Target rejected handshake: no mutually supported protocol was found.
    #[error("Target rejected handshake: no common protocol")]
    NoCommonProtocol,

    /// Unrecognized handshake status reported by peer.
    #[error("Target reported unsupported handshake status: {0}")]
    UnsupportedStatus(u8),
}

/// Errors that can occur during packet encoding or frame validation.
#[derive(Debug, Error, PartialEq, Eq, Clone, Copy)]
pub enum FrameError {
    /// Payload exceeds maximum permissible size.
    #[error("Payload length {0} exceeds MAX_PAYLOAD_SIZE")]
    PayloadTooLarge(usize),

    /// The sliding window is full; cannot enqueue further frames.
    #[error("Sliding window is full (window size {window_size}, in-flight {in_flight})")]
    WindowFull {
        /// Configured window size.
        window_size: u8,
        /// Frames currently in flight.
        in_flight: u8,
    },
}

/// Error emitted when consecutive Go-Back-N retransmission timeouts exceed the configured limit.
#[derive(Debug, Error, PartialEq, Eq, Clone, Copy)]
#[error("Too many retransmission failures, base={base}, next_seq={next_seq}, attempts={attempts}")]
pub struct RetransmissionLimitExceeded {
    /// Base sequence number of oldest unacknowledged frame.
    pub base: u8,
    /// Next sequence number to be assigned.
    pub next_seq: u8,
    /// Number of consecutive retransmission attempts made.
    pub attempts: usize,
}

/// Error emitted when an incoming sequence number does not match the expected sequence number.
#[derive(Debug, Error, PartialEq, Eq, Clone, Copy)]
#[error("Unexpected sequence number: expected {expected}, received {received}")]
pub struct UnexpectedSeqError {
    /// The sequence number expected by the receiver.
    pub expected: u8,
    /// The sequence number that was actually received.
    pub received: u8,
}
