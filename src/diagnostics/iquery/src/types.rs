// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::str::FromStr;
use thiserror::Error;

#[cfg(feature = "fdomain")]
use component_debug_fdomain as component_debug;
#[cfg(target_os = "fuchsia")]
use diagnostics_reader as reader;

use flex_fuchsia_developer_remotecontrol::ConnectCapabilityError;

#[derive(Error, Debug)]
pub enum Error {
    #[cfg(target_os = "fuchsia")]
    #[error("Error while fetching data: {0}")]
    Fetch(reader::Error),

    #[error("Invalid format: {0}")]
    InvalidFormat(String),

    #[error("Invalid arguments: {0}")]
    InvalidArguments(String),

    #[error("Failed formatting the command response: {0}")]
    InvalidCommandResponse(serde_json::Error),

    #[error("Failed parsing selector {0}: {1}")]
    ParseSelector(String, anyhow::Error),

    #[error("Failed to list archive accessors on {0} {1}")]
    ListAccessors(String, anyhow::Error),

    #[error("Error while communicating with {0}: {1}")]
    CommunicatingWith(String, #[source] anyhow::Error),

    #[error("Failed to connect to {0}: {1}")]
    ConnectToProtocol(String, #[source] anyhow::Error),

    #[error("IO error. Failed to {0}: {1}")]
    IOError(String, #[source] anyhow::Error),

    #[error("No running component was found whose URL contains the given string: {0}")]
    ManifestNotFound(String),

    #[error("No running components were found matching {0}")]
    SearchParameterNotFound(String),

    #[error("Invalid selector: {0}")]
    InvalidSelector(String),

    #[error("Invalid accessor: {0}")]
    InvalidAccessor(String),

    #[error(transparent)]
    GetManifestError(#[from] component_debug::realm::GetDeclarationError),

    #[error(transparent)]
    GetAllInstancesError(#[from] component_debug::realm::GetAllInstancesError),

    #[error(transparent)]
    SocketConversionError(#[from] std::io::Error),

    #[error(transparent)]
    FidlError(#[from] fidl::Error),

    #[error("Not enough dots in selector")]
    NotEnoughDots,

    #[error("Must be an exact moniker. Wildcards are not supported.")]
    MustBeExactMoniker,

    #[error("Must use a property selector to specify the protocol.")]
    MustUsePropertySelector,

    #[error("Failed to connect to capability {0:?}")]
    FailedToConnectToCapability(ConnectCapabilityError),

    #[error("Must be exact protocol (protocol cannot contain wildcards)")]
    MustBeExactProtocol,

    #[error(transparent)]
    FuzzyMatchRealmQuery(anyhow::Error),

    #[error("Fuzzy matching failed due to too many matches, please re-try with one of these:\n{0}")]
    FuzzyMatchTooManyMatches(FuzzyMatchErrorWrapper),

    #[error(
        "hint: positional selectors paired with --data must not include tree/node selector segment: {0}"
    )]
    PartialSelectorHint(#[source] selectors::Error),

    #[error(
        "When using --data, there must be exactly one component query as a positional argument"
    )]
    WrongNumberOfSearchQueriesForDataFlag,
}

#[derive(Debug, PartialEq, Eq)]
pub struct FuzzyMatchErrorWrapper(pub Vec<String>);

impl std::iter::FromIterator<String> for FuzzyMatchErrorWrapper {
    fn from_iter<I: IntoIterator<Item = String>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl std::fmt::Display for FuzzyMatchErrorWrapper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        for component in &self.0 {
            writeln!(f, "{component}")?;
        }

        Ok(())
    }
}

impl From<ConnectCapabilityError> for Error {
    fn from(value: ConnectCapabilityError) -> Self {
        Self::FailedToConnectToCapability(value)
    }
}

impl Error {
    pub fn invalid_accessor(name: impl Into<String>) -> Error {
        Error::InvalidAccessor(name.into())
    }

    pub fn invalid_format(format: impl Into<String>) -> Error {
        Error::InvalidFormat(format.into())
    }

    pub fn invalid_arguments(msg: impl Into<String>) -> Error {
        Error::InvalidArguments(msg.into())
    }

    pub fn io_error(msg: impl Into<String>, e: anyhow::Error) -> Error {
        Error::IOError(msg.into(), e)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Format {
    Text,
    Json,
}

impl FromStr for Format {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_ref() {
            "json" => Ok(Format::Json),
            "text" => Ok(Format::Text),
            f => Err(Error::invalid_format(f)),
        }
    }
}
