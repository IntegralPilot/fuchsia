// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
#[argh(
    subcommand,
    name = "symbol-index",
    description = "manage symbol sources used by other debug commands",
    note = "symbol-index is a global configuration used by debugging tools to locate
symbol files."
)]
pub struct SymbolIndexCommand {
    #[argh(subcommand)]
    pub sub_command: SymbolIndexSubCommand,
}

#[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
#[argh(subcommand)]
pub enum SymbolIndexSubCommand {
    List(ListCommand),
    Add(AddCommand),
    Remove(RemoveCommand),
    Clean(CleanCommand),
}

#[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
#[argh(subcommand, name = "list", description = "show the content in symbol index")]
pub struct ListCommand {
    /// show the aggregated symbol index
    #[argh(switch, short = 'a')]
    pub aggregated: bool,
}

#[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
#[argh(
    subcommand,
    name = "add",
    description = "add a path or url to the symbol index",
    note = "Add a path or a debuginfod server to the symbol index, e.g.,
  - A build-id directory, with an optional build directory.
  - An ids.txt file, with an optional build directory.
  - A file that ends with .symbol-index.json.
  - https://debuginfod.debian.net

Duplicated adding of the same path or url is a no-op, regardless of the optional
build directory."
)]
pub struct AddCommand {
    #[argh(option)]
    /// optional build directory used by zxdb to locate the source code
    pub build_dir: Option<String>,

    #[argh(positional)]
    /// the source to add
    pub source: String,
}

#[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
#[argh(
    subcommand,
    name = "remove",
    description = "remove a path from the symbol index",
    note = "Remove a path or a debuginfod server from the symbol index, e.g.,
  - A build-id directory.
  - An ids.txt file.
  - A file that ends with .symbol-index.json.
  - https://debuginfod.debian.net"
)]
pub struct RemoveCommand {
    #[argh(positional)]
    /// the source to remove
    pub source: String,
}

#[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
#[argh(
    subcommand,
    name = "clean",
    description = "remove all non-existent paths",
    note = "Remove all non-existent paths from the symbol index, useful as a garbage
collection."
)]
pub struct CleanCommand {}
