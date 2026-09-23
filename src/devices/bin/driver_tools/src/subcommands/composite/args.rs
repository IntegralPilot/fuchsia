// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::subcommands::list::args::ListCompositeCommand;
use super::subcommands::show::args::ShowCompositeCommand;
use argh::{ArgsInfo, FromArgs};

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "composite",
    description = "Commands to interact with composite node specs."
)]
pub struct CompositeCommand {
    #[argh(subcommand)]
    pub subcommand: CompositeSubcommand,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand)]
pub enum CompositeSubcommand {
    List(ListCompositeCommand),
    Show(ShowCompositeCommand),
}
