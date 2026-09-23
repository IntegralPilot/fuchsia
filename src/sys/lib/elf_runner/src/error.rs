// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use clonable_error::ClonableError;

use thiserror::Error;

/// Errors produced when starting a component.
#[derive(Debug, Clone, Error)]
pub enum StartComponentError {
    #[error("failed to register as exception handler: {_0}")]
    ExceptionRegistrationFailed(#[source] zx::Status),
    #[error("failed to get process koid: {_0}")]
    ProcessGetKoidFailed(#[source] zx::Status),
    #[error("failed to get process info: {_0}")]
    ProcessInfoFailed(#[source] zx::Status),
    #[error("failed to mark main process as critical: {_0}")]
    ProcessMarkCriticalFailed(#[source] zx::Status),
    #[error("could not create job: {_0}")]
    JobError(#[from] JobError),
    #[error("failed to duplicate job: {_0}")]
    JobDuplicateFailed(#[source] zx::Status),
    #[error("failed to get job koid: {_0}")]
    JobGetKoidFailed(#[source] zx::Status),
    #[error("failed to get vDSO: {_0}")]
    VdsoError(#[from] VdsoError),
    #[error("error connecting to fuchsia.process.Launcher protocol: {_0}")]
    ProcessLauncherConnectError(#[source] ClonableError),
    #[error("fidl error in fuchsia.process.Launcher protocol: {_0}")]
    ProcessLauncherFidlError(#[source] fidl::Error),
    #[error("fuchsia.process.Launcher failed to create process: {_0}")]
    CreateProcessFailed(#[source] zx::Status),
    #[error("failed to duplicate UTC clock: {_0}")]
    UtcClockDuplicateFailed(#[source] zx::Status),
    #[error("failed to process the component's config data: {_0}")]
    ConfigDataError(#[from] runner::ConfigDataError),
    #[error("could not create component namespace, {_0}")]
    NamespaceError(#[from] namespace::NamespaceError),
    #[error("error configuring process launcher: {_0}")]
    LaunchError(#[from] runner::component::LaunchError),
    #[error("invalid start info: {_0}")]
    StartInfoError(#[from] StartInfoError),
    #[error("failed to create boot clock: {_0}")]
    BootClockCreateFailed(#[source] zx::Status),
}

impl StartComponentError {
    /// Convert this error into its approximate `zx::Status` equivalent.
    pub fn as_zx_status(&self) -> zx::Status {
        match self {
            StartComponentError::NamespaceError(_) => zx::Status::INVALID_ARGS,
            StartComponentError::LaunchError(_) => zx::Status::UNAVAILABLE,
            StartComponentError::StartInfoError(err) => err.as_zx_status(),
            _ => zx::Status::INTERNAL,
        }
    }
}

/// Errors from creating and initializing a component's job.
#[derive(Debug, Clone, Error)]
pub enum JobError {
    #[error("failed to set job policy: {_0}")]
    SetPolicy(#[source] zx::Status),
    #[error("failed to create child job: {_0}")]
    CreateChild(#[source] zx::Status),
}

/// Errors from parsing ComponentStartInfo.
#[derive(Debug, Clone, Error)]
pub enum StartInfoError {
    #[error(transparent)]
    StartInfoError(#[from] runner::StartInfoError),
    #[error("missing outgoing dir")]
    MissingOutgoingDir,
    #[error("missing runtime dir")]
    MissingRuntimeDir,
    #[error("missing component instance token")]
    MissingComponentInstanceToken,
    #[error("component resolved URL is malformed: {_0}")]
    BadResolvedUrl(String),
    #[error("program is invalid: {_0}")]
    ProgramError(#[from] ProgramError),
}

impl StartInfoError {
    /// Convert this error into its approximate `zx::Status` equivalent.
    pub fn as_zx_status(&self) -> zx::Status {
        match self {
            StartInfoError::StartInfoError(err) => err.as_zx_status(),
            StartInfoError::MissingOutgoingDir => zx::Status::INVALID_ARGS,
            StartInfoError::MissingRuntimeDir => zx::Status::INVALID_ARGS,
            StartInfoError::MissingComponentInstanceToken => zx::Status::INVALID_ARGS,
            StartInfoError::BadResolvedUrl(_) => zx::Status::INVALID_ARGS,
            StartInfoError::ProgramError(err) => err.as_zx_status(),
        }
    }
}

/// Errors from parsing the component `program` section to `ElfProgramConfig`.
#[derive(Debug, Clone, Error)]
pub enum ProgramError {
    #[error(
        "`is_shared_process` cannot be enabled without also enabling `job_policy_create_raw_processes`"
    )]
    SharedProcessRequiresJobPolicy,
    #[error("failed to parse: {_0}")]
    Parse(#[source] runner::StartInfoProgramError),
    #[error("configuration violates policy: {_0}")]
    Policy(#[source] routing::policy::PolicyError),
    #[error("policy disallows all sensitive features")]
    DenyPolicy,
}

impl ProgramError {
    /// Convert this error into its approximate `zx::Status` equivalent.
    pub fn as_zx_status(&self) -> zx::Status {
        match self {
            ProgramError::SharedProcessRequiresJobPolicy => zx::Status::INVALID_ARGS,
            ProgramError::Parse(_) => zx::Status::INVALID_ARGS,
            ProgramError::Policy(_) => zx::Status::ACCESS_DENIED,
            ProgramError::DenyPolicy => zx::Status::ACCESS_DENIED,
        }
    }
}

/// Errors from exception handling.
#[derive(Debug, Clone, Error)]
pub enum ExceptionError {
    #[error("failed to get thread koid: {_0}")]
    GetThreadKoid(#[source] zx::Status),
    #[error("failed to set exception state: {_0}")]
    SetState(#[source] zx::Status),
}

#[derive(Debug, Clone, Error)]
pub enum VdsoError {
    #[error("Could not duplicate VMO handle for vDSO with name \"{name}\": {status}")]
    CouldNotDuplicate {
        name: zx::Name,
        #[source]
        status: zx::Status,
    },
    #[error("No vDSO VMO found with name \"{_0}\"")]
    NotFound(zx::Name),
    #[error("failed to get vDSO name: {_0}")]
    GetName(#[source] zx::Status),
}
