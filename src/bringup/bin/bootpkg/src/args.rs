// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};

#[derive(FromArgs, Debug, PartialEq)]
/// discover information about installed bootfs packages
pub struct Args {
    #[argh(subcommand)]
    pub command: SubCommand,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand)]
pub enum SubCommand {
    List(ListCommand),
    Show(ShowCommand),
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "list")]
/// list set of bootfs package available
pub struct ListCommand {}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "show")]
/// display bootfs package information
pub struct ShowCommand {
    #[argh(positional)]
    /// package name to show information about
    pub package_name: String,
}
