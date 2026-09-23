// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::env::VarError;
use std::ffi::OsStr;

/// Inspection of the process's environment.
pub(crate) trait Environment {
    /// Fetches the environment variable `key` from the current process.
    ///
    /// See [std::env::var] for details.
    ///
    /// [std::env::var]: https://doc.rust-lang.org/std/env/fn.var.html
    fn var<K: AsRef<OsStr>>(&self, key: K) -> Result<String, VarError>;
}

/// Query the local process's environment.
pub(crate) struct LocalEnvironment;

impl Environment for LocalEnvironment {
    fn var<K: AsRef<OsStr>>(&self, key: K) -> Result<String, VarError> {
        std::env::var(key)
    }
}
