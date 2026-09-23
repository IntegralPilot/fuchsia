// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod config_domain;
mod fuchsia_env;

pub use config_domain::{ConfigDomain, FileError};
pub use fuchsia_env::{ConfigMap, ConfigPath, FuchsiaEnv};

#[cfg(test)]
pub mod tests {
    use camino::Utf8Path;

    const TEST_DATA_PATH: &str = env!("TEST_DATA_PATH");

    pub fn test_data_path() -> &'static Utf8Path {
        Utf8Path::new(TEST_DATA_PATH)
    }
}
