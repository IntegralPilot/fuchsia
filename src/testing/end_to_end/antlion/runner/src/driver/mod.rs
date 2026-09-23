// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub(crate) mod infra;
pub(crate) mod local;

use crate::config::Config;

use std::path::Path;

use anyhow::Result;

/// Driver provide insight into the information surrounding running an antlion
/// test.
pub(crate) trait Driver {
    /// Path to output directory for test artifacts.
    fn output_path(&self) -> &Path;
    /// Antlion config for use during test.
    fn config(&self) -> Config;
    /// Additional logic to run before all tests start
    fn setup(&self) -> Result<()>;
    /// Additional logic to run after all tests run, regardless of tests passing
    /// or failing.
    fn teardown(&self) -> Result<()>;
}
