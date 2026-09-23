// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgValue, FromArgs};

#[derive(PartialEq, Debug)]
pub enum PidOrName {
    Pid(u64),
    Name(String),
}

impl FromArgValue for PidOrName {
    fn from_arg_value(value: &str) -> Result<Self, String> {
        Ok(match value.parse::<u64>() {
            Ok(pid) => PidOrName::Pid(pid),
            _ => PidOrName::Name(value.to_string()),
        })
    }
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "show",
    description = "Show details about a specific drivers host",
    example = "To show information about a specific driver host:

    $ driver host show <PID>",
    error_code(1, "Failed to connect to the driver development service")
)]
pub struct ShowCommand {
    /// the koid or name of the driver host to show.
    #[argh(positional)]
    pub pid_or_name: PidOrName,

    /// if this exists, the user will be prompted for a component to select.
    #[argh(switch, short = 's', long = "select")]
    pub select: bool,

    /// whether to print driver runtime info for the driver host.
    #[argh(switch, short = 'r', long = "runtime")]
    pub runtime: bool,

    /// whether to print stack traces for the driver host.
    #[argh(switch, short = 't', long = "stack-trace")]
    pub stack_trace: bool,
}
