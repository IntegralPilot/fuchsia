// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use zx::Status;

/// Error type returned by drivers on startup.
///
/// Driver authors should not usually need to construct or convert errors to this type
/// manually. Instead, they can use the `?` operator to automatically propagate and convert
/// higher-level error types (like [`zx::Status`], [`anyhow::Error`], and [`fidl::Error`])
/// into `DriverError` at the `Driver::start` boundary.
#[derive(Debug)]
pub enum DriverError {
    /// A zx status error.
    Status(Status),
    /// An anyhow error.
    Anyhow(anyhow::Error),
    /// A FIDL error.
    Fidl(fidl::Error),
}

impl DriverError {
    /// Convert the error into a [`Status`], logging any internal or FIDL errors.
    pub fn log_to_status(self) -> Status {
        match self {
            DriverError::Status(status) => status,
            DriverError::Anyhow(err) => {
                if let Some(status) = err.root_cause().downcast_ref::<Status>() {
                    *status
                } else {
                    log::error!("Driver failed with internal error: {:?}", err);
                    Status::INTERNAL
                }
            }
            DriverError::Fidl(err) => {
                log::error!("Driver failed with FIDL error: {:?}", err);
                Status::INTERNAL
            }
        }
    }
}

impl From<Status> for DriverError {
    fn from(status: Status) -> Self {
        DriverError::Status(status)
    }
}

impl From<anyhow::Error> for DriverError {
    fn from(err: anyhow::Error) -> Self {
        DriverError::Anyhow(err)
    }
}

impl From<fidl::Error> for DriverError {
    fn from(err: fidl::Error) -> Self {
        DriverError::Fidl(err)
    }
}

impl std::fmt::Display for DriverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DriverError::Status(status) => write!(f, "Status: {status}"),
            DriverError::Anyhow(err) => write!(f, "Anyhow: {err}"),
            DriverError::Fidl(err) => write!(f, "FIDL: {err}"),
        }
    }
}

impl std::error::Error for DriverError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DriverError::Status(status) => Some(status),
            DriverError::Anyhow(err) => Some(err.as_ref()),
            DriverError::Fidl(err) => Some(err),
        }
    }
}

impl From<fidl_next::Error<Status>> for DriverError {
    fn from(err: fidl_next::Error<Status>) -> Self {
        match err {
            fidl_next::Error::Encode(encode_err) => {
                log::error!("FIDL encode error: {:?}", encode_err);
                DriverError::Status(Status::INTERNAL)
            }
            fidl_next::Error::Decode(decode_err) => {
                log::error!("FIDL decode error: {:?}", decode_err);
                DriverError::Status(Status::INTERNAL)
            }
            fidl_next::Error::Protocol(protocol_err) => match protocol_err {
                fidl_next::ProtocolError::TransportError(status) => DriverError::Status(status),
                fidl_next::ProtocolError::PeerClosed => DriverError::Status(Status::PEER_CLOSED),
                _ => {
                    log::error!("FIDL protocol error: {:?}", protocol_err);
                    DriverError::Status(Status::INTERNAL)
                }
            },
        }
    }
}
