// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[derive(Debug, thiserror::Error)]
pub enum DoctorUtilsError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Pgrep/Pkill execution status code: {0}")]
    ProcessStatusCode(i32),

    #[error("Pgrep/Pkill execution error")]
    ProcessStatusError,

    #[error("Zip archive error: {0}")]
    Zip(#[from] zip::result::ZipError),
}
