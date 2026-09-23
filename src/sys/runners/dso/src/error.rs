// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use elf_runner::error::{ProgramError, StartComponentError};
use fidl_fuchsia_component as fcomponent;
use runner::{StartInfoError, StartInfoProgramError};
use thiserror::Error;

#[derive(Debug, Error)]
pub(super) enum StartError {
    #[error("internal error")]
    Internal,

    #[error("invalid args")]
    InvalidArgs,

    #[error(transparent)]
    Program(#[from] ProgramError),

    #[error(transparent)]
    StartInfoProgram(#[from] StartInfoProgramError),

    #[error(transparent)]
    Start(#[from] StartComponentError),

    #[error(transparent)]
    StartInfo(#[from] StartInfoError),

    #[error("invalid namespace")]
    InvalidNamespace,

    #[error("could not open /pkg/{path}: {err}")]
    OpenPackagePathFidl { path: String, err: anyhow::Error },

    #[error("could not open dso for {path}: {err}")]
    OpenDsoFidl { path: String, err: anyhow::Error },

    #[error("could not open dso for {path}: {err}")]
    OpenDso { path: String, err: fidl::Status },

    #[error("could not load DSO {name}: {err}")]
    LoadDso { name: String, err: fidl::Status },

    #[error("could not create dispatcher for {name}: {err}")]
    CreateDispatcher { name: String, err: zx::Status },

    #[error("execution failed: {err}")]
    Execute { err: zx::Status },
}

impl From<&StartError> for zx::Status {
    fn from(value: &StartError) -> Self {
        let err = match value {
            StartError::Internal => fcomponent::Error::Internal,
            StartError::InvalidArgs => fcomponent::Error::InvalidArguments,
            StartError::InvalidNamespace => fcomponent::Error::InvalidArguments,
            StartError::Program(p) => return p.as_zx_status(),
            StartError::StartInfoProgram(_) => fcomponent::Error::InvalidArguments,
            StartError::StartInfo(p) => return p.as_zx_status(),
            StartError::Start(p) => return p.as_zx_status(),
            StartError::OpenPackagePathFidl { .. } => fcomponent::Error::ResourceNotFound,
            StartError::OpenDso { .. } => fcomponent::Error::ResourceNotFound,
            StartError::OpenDsoFidl { .. } => fcomponent::Error::ResourceNotFound,
            StartError::LoadDso { .. } => fcomponent::Error::ResourceUnavailable,
            StartError::Execute { .. } => fcomponent::Error::InstanceCannotStart,
            StartError::CreateDispatcher { .. } => fcomponent::Error::InstanceCannotStart,
        };
        zx::Status::err_from_raw(err.into_primitive() as i32)
    }
}
