// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::append;

use thiserror::Error;

#[derive(Error, Debug, PartialEq, Eq)]
pub enum FrameWriteError {
    #[error("Buffer is too small")]
    BufferTooSmall,
    #[error("Attempted to write an invalid frame: {0}")]
    InvalidData(String),
    #[error("Write failed: {0}")]
    BadWrite(String),
}

impl From<append::BufferTooSmall> for FrameWriteError {
    fn from(_error: append::BufferTooSmall) -> Self {
        FrameWriteError::BufferTooSmall
    }
}

impl From<zx::Status> for FrameWriteError {
    fn from(status: zx::Status) -> Self {
        FrameWriteError::BadWrite(status.to_string())
    }
}
#[derive(Error, Debug, PartialEq, Eq)]
#[error("Error parsing frame: {0}")]
pub struct FrameParseError(pub(crate) String);

pub type FrameParseResult<T> = Result<T, FrameParseError>;
