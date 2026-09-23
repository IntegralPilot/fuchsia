// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Error definitions for UART connection establishment, TTY configuration, and socket binding.

/// Errors encountered while opening or managing a UART driver connection.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, thiserror::Error)]
#[serde(tag = "type")]
pub enum ConnectionError {
    /// The specified serial device or socket path does not exist on the host filesystem.
    #[error("Target path {path} does not exist")]
    PathNotFound {
        /// Target device path that could not be found.
        path: String,
    },
    /// Permission denied when attempting to open the serial device path.
    #[error("Permission denied to open target path {path}: {error}")]
    PermissionDenied {
        /// Target path that was rejected.
        path: String,
        /// Underlying OS error message.
        error: String,
    },
    /// Failed to establish connection to an existing UNIX domain socket.
    #[error("Failed to connect to UNIX socket {path}: {error}")]
    UnixConnectFailed {
        /// UNIX socket path.
        path: String,
        /// Underlying connection error message.
        error: String,
    },
    /// Failed to configure termios baud rate, flow control, or raw mode on the TTY.
    #[error("Failed to configure terminal settings: {error}")]
    TtyConfigureFailed {
        /// Configuration error details.
        error: String,
    },
    /// Failed to bind the driver's UNIX domain control socket.
    #[error("Failed to bind driver control socket: {error}")]
    BindFailed {
        /// Socket binding error details.
        error: String,
    },
    /// Another driver process is already actively listening on the target socket path.
    #[error("Socket already in use: {socket_path}")]
    SocketInUse {
        /// Socket path currently held by another process.
        socket_path: String,
    },
    /// Uncategorized or unstructured error message.
    #[error("{message}")]
    Raw {
        /// Description of the error.
        message: String,
    },
}

impl ConnectionError {
    /// Constructs an unstructured raw connection error.
    pub fn raw(message: impl Into<String>) -> Self {
        Self::Raw { message: message.into() }
    }
}

impl From<String> for ConnectionError {
    fn from(message: String) -> Self {
        Self::Raw { message }
    }
}

impl From<&str> for ConnectionError {
    fn from(message: &str) -> Self {
        Self::Raw { message: message.to_string() }
    }
}
