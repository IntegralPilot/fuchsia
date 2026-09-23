// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "default",
    description = "View the default target",
    example = "For one-off overrides for the default use `--target` option:
    $ ffx --target <target name> <subcommand>

Set the default target in-tree with `fx set-device`:
    $ fx set-device <target name>

Set the default target out-of-tree with the `$FUCHSIA_NODENAME` environment variable:
    $ export FUCHSIA_NODENAME=<target name>

Unset the default target in-tree with `fx unset-device`:
    $ fx unset-device

Unset the default target out-of-tree with the `$FUCHSIA_NODENAME` environment variable:
    $ unset FUCHSIA_NODENAME

Verify the correct default target setting with `ffx target default get`:
    $ ffx target default get",
    note = "Views the default configured target used for all operations.
In `ffx target list` the default target is designated by a `*` next to the name."
)]
pub struct TargetDefaultCommand {
    #[argh(subcommand)]
    pub subcommand: SubCommand,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand)]
pub enum SubCommand {
    Get(TargetDefaultGetCommand),
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "get",
    description = "Get the default configured target",
    note = "Returns the effective default configured target from configuration,
$FUCHSIA_NODENAME, and $FUCHSIA_DEVICE_ADDR.
Returns an empty string if no default is configured."
)]
pub struct TargetDefaultGetCommand {}
