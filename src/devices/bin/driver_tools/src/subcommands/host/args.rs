// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::subcommands::list::args::ListCommand;
use super::subcommands::show::args::ShowCommand;
use argh::{ArgsInfo, FromArgs};

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "host",
    description = "Commands to interact with driver framework driver hosts."
)]
pub struct HostCommand {
    #[argh(subcommand)]
    pub subcommand: HostSubcommand,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand)]
pub enum HostSubcommand {
    List(ListCommand),
    Show(ShowCommand),
}
