// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use thiserror::Error;
use tpm2_tss_sys as tss_sys;

#[derive(Debug, Error)]
pub enum TpmError {
    #[error("Provided Buffer Was Too Small : Expected {:?} Found {:?}", .expected, .found)]
    BufferTooSmall { expected: usize, found: usize },
    #[error("Provided Buffer Was Too Large: Expected {:?} Found {:?}", .maximum, .found)]
    BufferTooLarge { maximum: usize, found: usize },
    #[error("Unexpected Property Count: Expected {:?} Found {:?}", .expected, .found)]
    UnexpectedPropertyCount { expected: u32, found: u32 },
    #[error("TSS2_RC Error: {:?}", .0)]
    TssReturnCode(tss_sys::TSS2_RC),
}
