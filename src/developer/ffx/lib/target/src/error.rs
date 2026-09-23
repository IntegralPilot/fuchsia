// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use errors::FfxError;
use std::net::SocketAddr;
use target_errors::FfxTargetError;

#[derive(Debug, thiserror::Error, Clone, PartialEq)]
pub enum TargetResolutionError {
    #[error("Target {node_name:?} did not have a product address")]
    MissingProductAddress { node_name: Option<String> },

    #[error("Target does not connect via networking")]
    NonNetworkTarget,

    #[error("Network connections are disabled in configuration")]
    NetworkDisabled,

    #[error("USB connections are disabled in configuration")]
    UsbDisabled,

    #[error("VSOCK connections are disabled in configuration")]
    VsockDisabled,

    #[error("Timeout after {timeout:?} identifying manual target {addr}")]
    ManualTargetTimeout { addr: SocketAddr, timeout: std::time::Duration },

    #[error("Connection to target was terminated")]
    ConnectionTerminated,
}

#[derive(Debug, thiserror::Error)]
pub enum FfxTargetCrateError {
    #[error("Cache error: {0}")]
    Cache(#[from] crate::cache::CacheError),

    #[error("Connection error: {0}")]
    Connection(#[from] crate::connection::ConnectionError),

    #[error("Knock error: {0}")]
    Knock(#[from] crate::KnockError),

    #[error("Ffx configuration error: {0}")]
    Config(#[from] ffx_config::api::ConfigError),

    #[error("Socket address translation failed: {0}")]
    SocketAddr(#[from] std::net::AddrParseError),

    #[error("Discovery error: {0}")]
    Discovery(#[from] discovery::error::Error),

    #[error("Target error: {0}")]
    Target(#[from] FfxError),

    #[error("Target parsing error: {0}")]
    JsonParse(#[from] serde_json::Error),

    #[error("Target resolution error: {0}")]
    Resolution(#[from] TargetResolutionError),

    #[error("Identify host failed with error: {0:?}")]
    IdentifyHost(fdomain_fuchsia_developer_remotecontrol::IdentifyHostError),

    #[error("USB Driver error: {0}")]
    UsbDriver(#[from] usb_driver_api::ProtocolError),

    #[error("Invalid network interface ID: {0}")]
    InvalidInterfaceId(#[from] netext::InvalidInterfaceIdError),

    #[error("FIDL error")]
    Fidl(#[from] fidl::Error),

    #[error(transparent)]
    Fallback(anyhow::Error),
}

impl From<FfxTargetError> for FfxTargetCrateError {
    fn from(err: FfxTargetError) -> Self {
        Self::Target(err.into())
    }
}

impl From<anyhow::Error> for FfxTargetCrateError {
    fn from(err: anyhow::Error) -> Self {
        match err.downcast::<FfxError>() {
            Ok(ffx_err) => Self::Target(ffx_err),
            Err(err) => match err.downcast::<FfxTargetError>() {
                Ok(target_err) => Self::Target(target_err.into()),
                Err(err) => Self::Fallback(err),
            },
        }
    }
}

impl FfxTargetCrateError {
    /// Converts this error into a user-facing command error (`Error::User`) if the underlying
    /// error represents a target resolution or connection failure that is actionable by the
    /// user (such as target not found or ambiguous target query). Otherwise, wraps it as an
    /// unexpected error (`Error::Unexpected`).
    pub fn into_command_error(self) -> ffx_command_error::Error {
        match self {
            Self::Target(err) => ffx_command_error::Error::User(anyhow::Error::new(err)),
            Self::Fallback(err) => ffx_command_error::Error::Unexpected(err),
            other => ffx_command_error::Error::Unexpected(anyhow::Error::new(other)),
        }
    }
}

impl Into<ffx_command_error::Error> for FfxTargetCrateError {
    fn into(self) -> ffx_command_error::Error {
        self.into_command_error()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl_fuchsia_developer_ffx as ffx;

    #[test]
    fn test_into_command_error_from_ffx_target_error() {
        let target_err = FfxTargetError::OpenTargetError {
            err: ffx::OpenTargetError::TargetNotFound,
            target: Some("test-target".to_string()),
            targets: vec![],
            target_source: None,
        };
        let crate_err: FfxTargetCrateError = target_err.into();
        let cmd_err = crate_err.into_command_error();
        match cmd_err {
            ffx_command_error::Error::User(user_err) => {
                let ffx_err = user_err.downcast_ref::<FfxError>().expect("expected FfxError");
                assert!(matches!(ffx_err, FfxError::OpenTargetError { .. }));
                let inner = user_err
                    .source()
                    .and_then(|s| s.downcast_ref::<FfxTargetError>())
                    .expect("expected inner FfxTargetError");
                assert!(matches!(
                    inner,
                    FfxTargetError::OpenTargetError {
                        err: ffx::OpenTargetError::TargetNotFound,
                        ..
                    }
                ));
            }
            other => panic!("expected Error::User, got {other:?}"),
        }
    }

    #[test]
    fn test_into_command_error_from_ffx_error() {
        let ffx_err = errors::ffx_error!("some ffx error");
        let crate_err: FfxTargetCrateError = ffx_err.into();
        let cmd_err = crate_err.into_command_error();
        match cmd_err {
            ffx_command_error::Error::User(user_err) => {
                let inner = user_err.downcast_ref::<FfxError>().expect("expected FfxError");
                assert_eq!(inner.to_string(), "some ffx error");
            }
            other => panic!("expected Error::User, got {other:?}"),
        }
    }

    #[test]
    fn test_into_command_error_from_anyhow_wrapping_ffx_target_error() {
        let target_err = FfxTargetError::OpenTargetError {
            err: ffx::OpenTargetError::QueryAmbiguous,
            target: Some("ambiguous".to_string()),
            targets: vec!["t1".to_string(), "t2".to_string()],
            target_source: None,
        };
        let anyhow_err = anyhow::Error::new(target_err);
        let crate_err: FfxTargetCrateError = anyhow_err.into();
        let cmd_err = crate_err.into_command_error();
        match cmd_err {
            ffx_command_error::Error::User(user_err) => {
                let ffx_err = user_err.downcast_ref::<FfxError>().expect("expected FfxError");
                assert!(matches!(ffx_err, FfxError::OpenTargetError { .. }));
            }
            other => panic!("expected Error::User, got {other:?}"),
        }
    }

    #[test]
    fn test_into_command_error_from_anyhow_wrapping_ffx_error() {
        let ffx_err = errors::ffx_error!("wrapped error");
        let anyhow_err = anyhow::Error::new(ffx_err);
        let crate_err: FfxTargetCrateError = anyhow_err.into();
        let cmd_err = crate_err.into_command_error();
        match cmd_err {
            ffx_command_error::Error::User(user_err) => {
                let inner = user_err.downcast_ref::<FfxError>().expect("expected FfxError");
                assert_eq!(inner.to_string(), "wrapped error");
            }
            other => panic!("expected Error::User, got {other:?}"),
        }
    }

    #[test]
    fn test_into_command_error_from_unexpected_anyhow() {
        let anyhow_err = anyhow::anyhow!("something unexpected broke");
        let crate_err: FfxTargetCrateError = anyhow_err.into();
        let cmd_err = crate_err.into_command_error();
        match cmd_err {
            ffx_command_error::Error::Unexpected(err) => {
                assert_eq!(err.to_string(), "something unexpected broke");
            }
            other => panic!("expected Error::Unexpected, got {other:?}"),
        }
    }

    #[test]
    fn test_into_command_error_from_other_variants() {
        let crate_err = FfxTargetCrateError::Resolution(TargetResolutionError::NonNetworkTarget);
        let cmd_err = crate_err.into_command_error();
        assert!(matches!(cmd_err, ffx_command_error::Error::Unexpected(_)));
    }
}
