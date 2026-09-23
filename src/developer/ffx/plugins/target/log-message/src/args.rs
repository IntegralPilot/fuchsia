// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use diagnostics_log_types::Severity;
use ffx_core::ffx_command;
use std::str::FromStr;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq, Clone)]
#[argh(
    subcommand,
    name = "log-message",
    description = r#"Writes a message to the device's log."#,
    example = "To write a message:

    $ ffx target log-message \"this is a log message\"
"
)]

pub struct LogMessageCommand {
    #[argh(
        option,
        short = 't',
        description = "the log tag to use",
        default = "String::from(\"ffx-cli\")"
    )]
    pub tag: String,
    #[argh(
        option,
        short = 's',
        description = "trace, debug, info, warn, error, fatal",
        default = "Severity::Warn",
        from_str_fn(severity_from_str)
    )]
    pub severity: Severity,
    #[argh(positional)]
    pub message: String,
}

fn severity_from_str(value: &str) -> Result<Severity, String> {
    Severity::from_str(value).map_err(|e| e.to_string())
}
