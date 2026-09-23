// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::transactional_symbolizer::ReadError;
use fdomain_fuchsia_developer_remotecontrol::{ConnectCapabilityError, IdentifyHostError};
use ffx_config::api::ConfigError;
use log_command_fdomain::FormatterError;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum LogError {
    #[error(
        "Failed to identify host: {0:?}. Please ensure the device is connected and powered on. Run 'ffx target list' to see available targets."
    )]
    IdentifyHostError(IdentifyHostError),
    #[error(transparent)]
    UnknownError(#[from] anyhow::Error),
    #[error(transparent)]
    FormatterError(#[from] FormatterError),
    #[error(transparent)]
    ConfigError(#[from] ConfigError),
    #[error(transparent)]
    FidlError(#[from] fidl::Error),
    #[error("No boot timestamp")]
    NoBootTimestamp,
    #[error(
        "SDK not available, please use --symbolize off to disable symbolization. Reason: {msg}"
    )]
    SdkNotAvailable { msg: &'static str },
    #[error("failed to connect: {0:?}")]
    ConnectCapabilityError(ConnectCapabilityError),
    #[error(transparent)]
    IOError(#[from] std::io::Error),
    #[error("Cannot use dump with --since now")]
    DumpWithSinceNow,
    #[error(
        "No symbolizer configuration provided. You can provide one via config, or run 'ffx log --symbolize off' to disable symbolization."
    )]
    NoSymbolizerConfig,
    #[error(transparent)]
    SymbolizerError(#[from] ReadError),
    #[error(
        "Daemon connection was lost and retries are disabled. Run 'ffx doctor' to diagnose and restore the daemon connection."
    )]
    DaemonRetriesDisabled,
    #[error("AI agent timed out waiting for device connection in dump mode")]
    AIAgentTimedOut,
    #[error(transparent)]
    LogCommand(fho::Error),
    #[error("--dump not supported, use dump instead")]
    DumpNotSupported,
    #[error(transparent)]
    Internal(#[from] fho::Error),
}

impl From<log_command_fdomain::LogError> for LogError {
    fn from(value: log_command_fdomain::LogError) -> Self {
        use log_command_fdomain::LogError::*;
        let err: fho::Error = match value {
            UnknownError(err) => err.into(),
            FuzzyMatchTooManyMatches { .. }
            | SearchParameterNotFound(_)
            | DumpWithSinceNow
            | NoBootTimestamp
            | NoSymbolizerConfig
            | RegexError(_)
            | DeprecatedFlag { .. } => fho::Error::User(value.into()),
            IOError(err) => fho::Error::Unexpected(err.into()),
            FfxError(err) => err.into(),
            Utf8Error(err) => fho::Error::Unexpected(err.into()),
            FidlError(err) => fho::Error::Unexpected(err.into()),
            FormatterError(err) => fho::Error::Unexpected(err.into()),
        };
        Self::LogCommand(err)
    }
}

impl From<LogError> for fho::Error {
    fn from(value: LogError) -> Self {
        use LogError::*;
        match value {
            // anyhow errors may carry ffx user errors, so let the normal translation deal with that.
            UnknownError(err) => err.into(),
            // these errors have useful, actionable errors for users
            DumpWithSinceNow
            | NoBootTimestamp
            | NoSymbolizerConfig
            | IdentifyHostError { .. }
            | DumpNotSupported
            | SdkNotAvailable { .. }
            | ConnectCapabilityError { .. }
            | AIAgentTimedOut => fho::Error::User(value.into()),
            // these errors are probably an unexpected problem with no actionable error output.
            FidlError(err) => fho::Error::Unexpected(err.into()),
            IOError(err) => fho::Error::Unexpected(err.into()),
            ConfigError(err) => fho::Error::Unexpected(err.into()),
            SymbolizerError(err) => fho::Error::Unexpected(err.into()),
            DaemonRetriesDisabled => fho::Error::Unexpected(value.into()),
            FormatterError(error) => fho::Error::Unexpected(error.into()),
            Internal(error) | LogCommand(error) => error,
        }
    }
}

impl From<ConnectCapabilityError> for LogError {
    fn from(error: ConnectCapabilityError) -> Self {
        Self::ConnectCapabilityError(error)
    }
}

impl From<IdentifyHostError> for LogError {
    fn from(error: IdentifyHostError) -> Self {
        LogError::IdentifyHostError(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identify_host_error_message() {
        let err = LogError::IdentifyHostError(IdentifyHostError::ProxyConnectionFailed);
        let msg = format!("{}", err);
        assert!(msg.contains("Please ensure the device is connected and powered on. Run 'ffx target list' to see available targets."));
    }

    #[test]
    fn test_daemon_retries_disabled_message() {
        let err = LogError::DaemonRetriesDisabled;
        let msg = format!("{}", err);
        assert_eq!(
            msg,
            "Daemon connection was lost and retries are disabled. Run 'ffx doctor' to diagnose and restore the daemon connection."
        );
    }
}
