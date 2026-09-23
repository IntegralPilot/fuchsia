// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "collection",
    description = "Manages collections in the component topology"
)]
pub struct CollectionCommand {
    #[argh(subcommand)]
    pub subcommand: SubCommandEnum,
}

#[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
#[argh(subcommand)]
pub enum SubCommandEnum {
    List(ListArgs),
    Show(ShowArgs),
}

#[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
#[argh(subcommand, name = "list", description = "List all collections in the component topology")]
pub struct ListArgs {}

#[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
#[argh(
    subcommand,
    name = "show",
    description = "Shows detailed information about a collection in the component topology"
)]
pub struct ShowArgs {
    #[argh(positional)]
    /// name of collection. Partial matches allowed.
    pub query: String,
}
