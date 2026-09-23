// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[macro_use]
pub mod macros;
mod context;
mod error;

pub use context::FfxContext;
pub use error::{Error, NonFatalError, Result};

#[doc(hidden)]
pub mod macro_deps {
    pub use anyhow;
}

#[cfg(test)]
pub mod tests {
    pub const FFX_STR: &str = "I am an ffx error";
    pub const ERR_STR: &str = "I am not an ffx error";
}
