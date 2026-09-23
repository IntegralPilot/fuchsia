// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_paver as paver;
use thiserror::Error;
use zx::Status;

/// Error condition that may be returned by a boot manager client.
#[derive(Error, Debug)]
pub enum BootManagerError {
    #[error("BootManager returned non-ok status while calling {method_name:}")]
    Status {
        method_name: &'static str,
        #[source]
        status: Status,
    },

    #[error("fidl error while calling BootManager method {method_name:}")]
    Fidl {
        method_name: &'static str,
        #[source]
        error: fidl::Error,
    },

    #[error("the status field of QueryConfigurationStatusAndBootAttempts was not set")]
    StatusNotSet,
}

/// Error condition that may be returned by the PolicyEngine.
#[derive(Error, Debug)]
pub enum PolicyError {
    #[error("the policy engine failed to build")]
    Build(#[source] BootManagerError),

    #[error(
        "the current configuration ({_0:?}) is unbootable with reason {_1:?}. This should never happen."
    )]
    CurrentConfigurationUnbootable(paver::Configuration, Option<paver::UnbootableReason>),
}

/// Error condition that may be returned by HealthVerification.
#[derive(Error, Debug)]
pub enum HealthVerificationError {
    #[error("fidl error while querying health checks")]
    Fidl(#[source] fidl::Error),
    #[error("HealthVerification responded with unhealthy status")]
    Unhealthy(#[source] Status),
}

/// Error condition that may be returned by `put_metadata_in_happy_state`.
#[derive(Error, Debug)]
pub enum MetadataError {
    #[error("while doing health verification")]
    HealthVerification(#[source] HealthVerificationError),

    #[error("while signalling EventPair peer")]
    SignalPeer(#[source] Status),

    #[error("while sending the unblock")]
    Unblock,

    #[error("while doing commit")]
    Commit(#[source] BootManagerError),

    #[error("while interfacing with policy")]
    Policy(#[source] PolicyError),

    #[error("the commit timed out")]
    Timeout,
}

/// Helper to convert fidl's nested errors.
pub trait BootManagerResultExt {
    type T;

    fn into_boot_manager_result(
        self,
        method_name: &'static str,
    ) -> Result<Self::T, BootManagerError>;
}

impl BootManagerResultExt for Result<i32, fidl::Error> {
    type T = ();

    fn into_boot_manager_result(
        self: Result<i32, fidl::Error>,
        method_name: &'static str,
    ) -> Result<(), BootManagerError> {
        match self.map(Status::ok) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(status)) => Err(BootManagerError::Status { status, method_name }),
            Err(error) => Err(BootManagerError::Fidl { error, method_name }),
        }
    }
}

impl<T> BootManagerResultExt for Result<Result<T, i32>, fidl::Error> {
    type T = T;

    fn into_boot_manager_result(
        self: Result<Result<Self::T, i32>, fidl::Error>,
        method_name: &'static str,
    ) -> Result<Self::T, BootManagerError> {
        match self {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(raw)) => {
                Err(BootManagerError::Status { status: Status::err_from_raw(raw), method_name })
            }
            Err(error) => Err(BootManagerError::Fidl { error, method_name }),
        }
    }
}
