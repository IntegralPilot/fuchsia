// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// NOTE: The doc comments on `Flags` and its fields appear as the helptext of
/// `fx testgen`. Please run that command to make sure the output looks correct before
/// submitting changes.
use argh::FromArgs;
use diagnostics_log::{set_minimum_severity, Severity};

/// testgen generates a Fuchsia test.
#[derive(FromArgs, Debug)]
pub(crate) struct Flags {
    #[argh(subcommand)]
    pub subcommand: Subcommand,

    /// if true, all logs are printed. Otherwise only errors are shown.
    #[argh(option, short = 'v', default = "false")]
    pub verbose_logging: bool,

    /// if set, override the year of generated files to this value
    #[argh(option)]
    pub year_override: Option<String>,
}

#[derive(FromArgs, PartialEq, Debug)]
#[argh(subcommand)]
pub(crate) enum Subcommand {
    IntegrationTest(crate::cmd_integration_test::IntegrationTestCmd),
}

impl Flags {
    pub fn setup_logging(&self) {
        set_minimum_severity(match self.verbose_logging {
            true => Severity::Info,
            false => Severity::Error,
        });
    }
}
