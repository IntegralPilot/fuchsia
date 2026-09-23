// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};

#[derive(ArgsInfo, FromArgs, Debug, PartialEq, Clone)]
#[argh(
    subcommand,
    name = "local-name",
    description = "Interact with the active Bluetooth controller's local name.",
    example = "ffx bluetooth controller local-name"
)]
pub struct LocalNameCommand {
    #[argh(subcommand)]
    pub subcommand: LocalNameSubCommand,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq, Clone)]
#[argh(subcommand)]
pub enum LocalNameSubCommand {
    Set(SetCommand),
    Get(GetCommand),
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq, Clone)]
#[argh(
    subcommand,
    name = "set",
    description = "Set the active Bluetooth controller's local name.",
    example = "ffx bluetooth controller local-name set <name>"
)]
pub struct SetCommand {
    #[argh(positional)]
    pub name: String,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq, Clone)]
#[argh(
    subcommand,
    name = "get",
    description = "Get the active Bluetooth controller's local name.",
    example = "ffx bluetooth controller local-name get <name>"
)]
pub struct GetCommand {}
