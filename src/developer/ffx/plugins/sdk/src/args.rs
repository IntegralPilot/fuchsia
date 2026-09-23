// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use camino::Utf8PathBuf;
use ffx_core::ffx_command;
use std::path::PathBuf;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "sdk", description = "Modify or query the installed SDKs")]
pub struct SdkCommand {
    #[argh(subcommand)]
    pub sub: SubCommand,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand)]
pub enum SubCommand {
    Version(VersionCommand),
    Set(SetCommand),
    Run(RunCommand),
    PopulatePath(PopulatePathCommand),
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "version", description = "Retrieve the version of the current SDK")]
pub struct VersionCommand {}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "run", description = "Run a host tool from the active sdk")]
pub struct RunCommand {
    #[argh(positional)]
    /// the name of the host-tool to run from the active sdk
    pub name: String,
    #[argh(positional, greedy)]
    /// the host tool command to run, starting with the
    pub args: Vec<String>,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "set", description = "Set sdk-related configuration options")]
pub struct SetCommand {
    #[argh(subcommand)]
    pub sub: SetSubCommand,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand)]
pub enum SetSubCommand {
    Root(SetRootCommand),
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "root", description = "Sets the path to the root of the preferred SDK")]
pub struct SetRootCommand {
    #[argh(positional)]
    /// path to the sdk root
    pub path: PathBuf,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "populate-path",
    description = "Populates the given path with symlinks to the `fuchsha-sdk-run` tool to run project-specific sdk tools"
)]
pub struct PopulatePathCommand {
    #[argh(positional)]
    /// path to the location to install the `fuchsia-sdk-run` links
    pub path: Utf8PathBuf,
}
