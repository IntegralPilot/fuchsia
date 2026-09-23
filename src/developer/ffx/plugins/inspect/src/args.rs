// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use iquery_fdomain::commands::{ListAccessorsCommand, ListCommand, SelectorsCommand, ShowCommand};
use std::path::PathBuf;

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "inspect",
    description = "Query data exposed by components via the Inspect API.",
    example = "\
    If you wish to see the JSON format of Inspect, you must pass `--machine json` to the `ffx` \
    command. \n\
    For example to see the Inspect JSON of all components running in the system, run: \n\n\
    ```\n\
    ffx --machine json inspect show\n\
    ```"
)]
pub struct InspectCommand {
    #[argh(subcommand)]
    pub sub_command: InspectSubCommand,
}

#[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
#[argh(subcommand)]
pub enum InspectSubCommand {
    ApplySelectors(ApplySelectorsCommand),
    List(ListCommand),
    ListAccessors(ListAccessorsCommand),
    Selectors(SelectorsCommand),
    Show(ShowCommand),
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "apply-selectors",
    description = "Apply selectors from a file interactively."
)]
pub struct ApplySelectorsCommand {
    #[argh(positional)]
    /// path to the selector file to apply to the snapshot.
    pub selector_file: PathBuf,

    #[argh(option)]
    /// path to the inspect json file to read
    /// this file contains inspect.json data from snapshot.zip.
    /// If not provided, DiagnosticsProvider will be used to get inspect data.
    pub snapshot_file: Option<PathBuf>,

    #[argh(option)]
    /// moniker of the component to apply the command.
    pub moniker: Option<String>,

    #[argh(option)]
    /// the path from where to get the ArchiveAccessor connection. If the given path is a
    /// directory, the command will look for a `fuchsia.diagnostics.ArchiveAccessor` service file.
    /// If the given path is a service file, the command will attempt to connect to it as an
    /// ArchiveAccessor.
    pub accessor_path: Option<String>,
}
