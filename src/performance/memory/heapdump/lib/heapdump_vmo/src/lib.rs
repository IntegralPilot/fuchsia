// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use thiserror::Error;

pub mod allocations_table_v1;
pub mod resources_table_v1;
pub mod stack_trace_compression;

#[derive(Debug, Eq, Error, PartialEq)]
pub enum Error {
    #[error("The provided memory region is too small")]
    BufferTooSmall,
    #[error("The provided memory region is too big")]
    BufferTooBig,
    #[error("Operation failed due to lack of space")]
    OutOfSpace,
    #[error("Operation failed because the provided input is not valid")]
    InvalidInput,
    #[error("System call failed: {}", .0)]
    SyscallFailed(#[from] zx::Status),
}

impl From<memory_mapped_vmo::Error> for Error {
    fn from(error: memory_mapped_vmo::Error) -> Self {
        match error {
            memory_mapped_vmo::Error::InvalidInput => Error::InvalidInput,
        }
    }
}
