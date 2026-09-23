// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs, SubCommand};
use ffx_core::ffx_command;
use std::sync::LazyLock;

#[ffx_command()]
#[derive(Clone, Debug, PartialEq)]
pub struct DevicesCommand(pub ffx_list_args::ListCommand);

// Implement FromArgs manually to delegate to ListCommand.
// We intercept EarlyExit (like --help) to replace "target list" with "devices".
impl FromArgs for DevicesCommand {
    fn from_args(command_name: &[&str], args: &[&str]) -> Result<Self, argh::EarlyExit> {
        match ffx_list_args::ListCommand::from_args(command_name, args) {
            Ok(cmd) => Ok(DevicesCommand(cmd)),
            Err(mut early_exit) => {
                early_exit.output = early_exit.output.replace("target list", "devices");
                Err(early_exit)
            }
        }
    }

    fn redact_arg_values(
        command_name: &[&str],
        args: &[&str],
    ) -> Result<Vec<String>, argh::EarlyExit> {
        match ffx_list_args::ListCommand::redact_arg_values(command_name, args) {
            Ok(redacted) => Ok(redacted),
            Err(mut early_exit) => {
                early_exit.output = early_exit.output.replace("target list", "devices");
                Err(early_exit)
            }
        }
    }
}

// Implement SubCommand manually to define the command name as "devices".
impl SubCommand for DevicesCommand {
    const COMMAND: &'static argh::CommandInfo = &argh::CommandInfo {
        name: "devices",
        short: &'\0',
        description: "List all targets (alias for ffx target list)",
    };
}

static DEVICES_INFO: LazyLock<argh::CommandInfoWithArgs> = LazyLock::new(|| {
    let mut info = <ffx_list_args::ListCommand as ArgsInfo>::get_args_info();
    info.name = "devices";
    info.description = "List all targets (alias for ffx target list)";

    let mut examples = Vec::new();
    for ex in info.examples {
        let replaced = ex.replace("target list", "devices");
        examples.push(Box::leak(replaced.into_boxed_str()) as &'static str);
    }
    info.examples = Box::leak(examples.into_boxed_slice());

    let mut notes = Vec::new();
    for note in info.notes {
        let replaced = note.replace("target list", "devices");
        notes.push(Box::leak(replaced.into_boxed_str()) as &'static str);
    }
    info.notes = Box::leak(notes.into_boxed_slice());

    info
});

// Implement ArgsInfo manually to expose the modified CommandInfoWithArgs.
impl ArgsInfo for DevicesCommand {
    fn get_args_info() -> argh::CommandInfoWithArgs {
        DEVICES_INFO.clone()
    }
}

// Implement From to allow easy conversion to ListCommand.
impl From<DevicesCommand> for ffx_list_args::ListCommand {
    fn from(dev: DevicesCommand) -> Self {
        dev.0
    }
}
