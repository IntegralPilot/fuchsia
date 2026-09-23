// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_component_sandbox as fsandbox;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum UtilsError {
    #[error("FIDL error: {0}")]
    Fidl(#[from] fidl::Error),
    #[error("Unexpected capability type. Wanted: {0}, got: {1}")]
    UnexpectedCapabilityType(String, String),
    #[error("Unexpected routed type")]
    UnexpectedRoutedType,
    #[error("Sandbox error: {0}")]
    SandboxError(String),
    #[error("zx error: {0}")]
    ZxStatus(zx::Status),
}

impl From<fsandbox::CapabilityStoreError> for UtilsError {
    fn from(err: fsandbox::CapabilityStoreError) -> Self {
        UtilsError::SandboxError(format!("CapabilityStoreError: {:?}", err))
    }
}

impl From<fsandbox::RouterError> for UtilsError {
    fn from(err: fsandbox::RouterError) -> Self {
        UtilsError::SandboxError(format!("RouterError: {:?}", err))
    }
}

impl From<i32> for UtilsError {
    fn from(err: i32) -> Self {
        UtilsError::ZxStatus(zx::Status::err_from_raw(err))
    }
}
