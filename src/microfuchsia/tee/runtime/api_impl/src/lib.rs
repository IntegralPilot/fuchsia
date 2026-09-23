// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use tee_internal::Error;

pub mod binding_stubs;
pub mod context;
pub mod crypto;
pub mod mem;
pub mod props;
pub mod storage;
pub mod time;

/// Packages an error alongside an optional, context-dependent size. In
/// particular, if `error` is `Error::ShortBuffer`, then `size`` is the
/// required size of the output buffer for the operation in question.
pub struct ErrorWithSize {
    pub error: Error,
    pub size: usize,
}

impl ErrorWithSize {
    const fn new(error: Error) -> Self {
        Self { error, size: 0 }
    }

    const fn short_buffer(size: usize) -> Self {
        Self { error: Error::ShortBuffer, size }
    }
}

pub fn panic(code: u32) {
    std::process::exit(code as i32)
}
