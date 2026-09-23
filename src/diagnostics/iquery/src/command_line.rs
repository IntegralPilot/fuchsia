// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::commands::*;
use crate::types::*;
use argh::FromArgs;
use serde::Serialize;

#[derive(FromArgs, PartialEq, Debug)]
#[argh(subcommand)]
pub enum SubCommand {
    List(ListCommand),
    ListAccessors(ListAccessorsCommand),
    Selectors(SelectorsCommand),
    Show(ShowCommand),
}

#[derive(FromArgs, PartialEq, Debug)]
/// Top-level command.
pub struct CommandLine {
    #[argh(option, default = "Format::Text", short = 'f')]
    /// the format to be used to display the results (json, text).
    pub format: Format,

    #[argh(subcommand)]
    pub command: SubCommand,

    #[argh(option)]
    /// optional tag to print to the console before and after the normal output
    /// of this program.
    /// This instructs the Archivist to suspend printing serial logs from other
    /// sources until this program has finished.
    pub serial_tag: Option<String>,
}

fn serialize<T: Serialize + ToString>(format: &Format, input: T) -> Result<String, Error> {
    match format {
        Format::Json => serde_json::to_string_pretty(&input).map_err(Error::InvalidCommandResponse),
        Format::Text => Ok(input.to_string()),
    }
}

impl Command for CommandLine {
    type Result = String;

    async fn execute<P: DiagnosticsProvider>(self, provider: &P) -> Result<Self::Result, Error> {
        match self.command {
            SubCommand::List(c) => serialize(&self.format, c.execute(provider).await?),
            SubCommand::ListAccessors(c) => serialize(&self.format, c.execute(provider).await?),
            SubCommand::Selectors(c) => serialize(&self.format, c.execute(provider).await?),
            SubCommand::Show(c) => serialize(&self.format, c.execute(provider).await?),
        }
    }
}
