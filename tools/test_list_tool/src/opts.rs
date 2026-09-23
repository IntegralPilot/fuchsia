// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, ensure};
use camino::Utf8PathBuf;
use clap::Parser;

#[derive(Parser, Debug)]
pub struct Opt {
    #[arg(short = 'i', long = "input")]
    // Path to the tests.json file.
    pub input: Utf8PathBuf,

    #[arg(long = "disabled-ctf-tests")]
    // Path to the disabled_tests.json file.
    pub disabled_ctf_tests: Option<Utf8PathBuf>,

    #[arg(short = 't', long = "test-components")]
    // Path to the test_components.json file.
    pub test_components_list: Utf8PathBuf,

    #[arg(short = 'o', long = "output")]
    // Path to output test-list.
    pub output: Utf8PathBuf,

    #[arg(short = 'b', long = "build-dir")]
    // Path to the build directory.
    pub build_dir: Utf8PathBuf,

    #[arg(short = 'd', long = "depfile")]
    // Path to output a depfile.
    pub depfile: Option<Utf8PathBuf>,

    #[arg(long = "experimental-test-config")]
    /// This is experimental as we are just prototyping
    /// TODO(b/294567466): Eventually this will replace test-list.json.
    pub test_config_output: Option<Utf8PathBuf>,

    #[arg(long = "ignore-device-test-errors")]
    /// If set, do not fail if there are errors with device tests.
    /// This may be desired for use from scripts that prefer to simply skip those tests.
    pub ignore_device_test_errors: bool,

    #[arg(long = "single-threaded")]
    /// If set, do not parallelize reads.
    /// This is useful for debugging.
    pub single_threaded: bool,
}

impl Opt {
    pub fn validate(&self) -> Result<(), Error> {
        ensure!(self.input.exists(), "input {:?} does not exist", self.input);
        ensure!(
            self.test_components_list.exists(),
            "test_components_list {:?} does not exist",
            self.test_components_list
        );
        ensure!(self.build_dir.exists(), "build-dir {:?} does not exist", self.build_dir);
        Ok(())
    }
}
