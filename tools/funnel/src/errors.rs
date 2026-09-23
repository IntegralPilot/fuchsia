// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::ssh::{CloseExistingTunnelError, TunnelError};
use crate::target::ChooseTargetError;
#[cfg(feature = "update")]
use crate::update::UpdateError;
use std::fmt::Debug;
use thiserror::Error;

pub(crate) trait IntoExitCode {
    fn exit_code(&self) -> i32;
}

#[derive(Error, Debug)]
pub enum FunnelError {
    #[error("{}", .0)]
    Error(#[from] anyhow::Error),

    #[error("{}", .0)]
    IoError(#[from] std::io::Error),

    #[error("{}", .0)]
    CloseExistingTunnelError(#[from] CloseExistingTunnelError),

    #[error("{}", .0)]
    TunnelError(#[from] TunnelError),

    #[cfg(feature = "update")]
    #[error("{}", .0)]
    UpdateError(#[from] UpdateError),

    #[error("{}", .0)]
    ChooseTargetError(#[from] ChooseTargetError),
}

impl IntoExitCode for FunnelError {
    fn exit_code(&self) -> i32 {
        match self {
            Self::Error(_) => 1,
            Self::IoError(e) => e.raw_os_error().unwrap_or(2),
            Self::CloseExistingTunnelError(e) => e.exit_code(),
            Self::TunnelError(e) => e.exit_code(),
            #[cfg(feature = "update")]
            Self::UpdateError(e) => e.exit_code(),
            Self::ChooseTargetError(e) => e.exit_code(),
        }
    }
}
