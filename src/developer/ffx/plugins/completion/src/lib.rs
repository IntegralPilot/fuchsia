// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{
    CommandInfoWithArgs, ErrorCodeInfo, FlagInfo, FlagInfoKind, Optionality, PositionalInfo,
    SubCommandInfo,
};
use argh_complete::Generator;
use async_trait::async_trait;
use ffx_command::CliArgsInfo;
use ffx_completion_args::CompletionCommand;
use fho::{FfxMain, FfxTool, Result, return_user_error};
use std::process::Command;

#[derive(FfxTool)]
pub struct CompletionTool {
    #[command]
    cmd: CompletionCommand,
}

fho::embedded_plugin!(CompletionTool);

#[async_trait(?Send)]
impl FfxMain for CompletionTool {
    type Writer = fho::null_writer::NullWriter;

    type Error = ::fho::Error;

    async fn main(self, _writer: Self::Writer) -> Result<()> {
        let exe = std::env::current_exe()
            .map_err(|e| fho::user_error!("Failed to get current executable: {}", e))?;

        // Execute `ffx --machine json help` to get the full JSON structure of all commands
        let output =
            Command::new(&exe).args(["--machine", "json", "help"]).output().map_err(|e| {
                fho::user_error!("Failed to execute ffx help to get completions schema: {}", e)
            })?;

        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            return_user_error!("ffx help failed with {}: {}", output.status, err);
        }

        let cli_args: CliArgsInfo = serde_json::from_slice(&output.stdout)
            .map_err(|e| fho::user_error!("Failed to parse JSON output from ffx help: {}", e))?;

        let command_info = convert_to_command_info(&cli_args);

        let completion_script = match self.cmd.shell.as_str() {
            "bash" => argh_complete::bash::Bash::generate("ffx", &command_info),
            "zsh" => argh_complete::zsh::Zsh::generate("ffx", &command_info),
            "fish" => argh_complete::fish::Fish::generate("ffx", &command_info),
            _ => return_user_error!(
                "Unsupported shell '{}'. Supported shells are: bash, zsh, fish",
                self.cmd.shell
            ),
        };

        println!("{}", completion_script);
        Ok(())
    }
}

/// Converts a `ffx_command::CliArgsInfo` into an `argh_shared::CommandInfoWithArgs<'static>`.
/// Because `argh_complete` requires `&'static str` for all strings inside its schema struct,
/// we simply leak the allocated strings. This is safe to do because the `completion` tool
/// halts immediately after generating and printing its completion script out.
fn convert_to_command_info(cli: &CliArgsInfo) -> CommandInfoWithArgs {
    let name = leak_string(&cli.name);
    let description = leak_string(&cli.description);
    let mut flags = Vec::new();
    for f in &cli.flags {
        let kind = match &f.kind {
            ffx_command::FlagKind::Switch => FlagInfoKind::Switch,
            ffx_command::FlagKind::Option { arg_name } => {
                FlagInfoKind::Option { arg_name: leak_string(arg_name) }
            }
        };

        let optionality = match f.optionality {
            ffx_command::Optionality::Required => Optionality::Required,
            ffx_command::Optionality::Optional => Optionality::Optional,
            ffx_command::Optionality::Repeating => Optionality::Repeating,
            ffx_command::Optionality::Greedy => Optionality::Greedy,
        };

        flags.push(FlagInfo {
            kind,
            optionality,
            long: leak_string(&f.long),
            short: f.short,
            description: leak_string(&f.description),
            hidden: f.hidden,
        });
    }

    let mut positionals = Vec::new();
    for p in &cli.positionals {
        let optionality = match p.optionality {
            ffx_command::Optionality::Required => Optionality::Required,
            ffx_command::Optionality::Optional => Optionality::Optional,
            ffx_command::Optionality::Repeating => Optionality::Repeating,
            ffx_command::Optionality::Greedy => Optionality::Greedy,
        };

        positionals.push(PositionalInfo {
            name: leak_string(&p.name),
            description: leak_string(&p.description),
            optionality,
            hidden: p.hidden,
        });
    }

    let mut commands = Vec::new();
    for curr in &cli.commands {
        commands.push(SubCommandInfo {
            name: leak_string(&curr.name),
            command: convert_to_command_info(&curr.command),
        });
    }

    let mut error_codes = Vec::new();
    for e in &cli.error_codes {
        error_codes.push(ErrorCodeInfo { code: e.code, description: leak_string(&e.description) });
    }

    CommandInfoWithArgs {
        name,
        short: &'\0', // ffx command args info does not parse short alias strings in this struct currently
        description,
        examples: &[],
        flags: Box::leak(flags.into_boxed_slice()),
        notes: &[],
        commands,
        positionals: Box::leak(positionals.into_boxed_slice()),
        error_codes: Box::leak(error_codes.into_boxed_slice()),
    }
}

fn leak_string(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}
