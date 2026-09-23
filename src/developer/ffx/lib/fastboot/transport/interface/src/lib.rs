// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod tcp;
pub mod udp;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum FastbootTransportError {
    #[error("Timed out waiting for reply")]
    Timeout,

    #[error("Could not parse response packet")]
    ParseError,

    #[error("Sending error: {0}")]
    SendError(std::io::Error),

    #[error("Recv error: {0}")]
    RecvError(std::io::Error),

    #[error("Invalid response to handshake")]
    InvalidHandshake,

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}
