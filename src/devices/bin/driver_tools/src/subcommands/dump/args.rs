// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "dump",
    description = "Dump device tree",
    example = "To dump the device tree:

    $ driver dump\n",
    example = "To dump the subtree of the device tree under a node:

    $ driver dump my-node-name\n",
    error_code(1, "Failed to connect to the driver development service"),
    example = "To graph device tree:

    $ driver dump --graph | dot -Tpng | display"
)]
pub struct DumpCommand {
    /// the device name to dump. All devices with this name will have their subtree printed. If
    /// this is not supplied then the entire device tree will be dumped.
    #[argh(positional)]
    pub device: Option<String>,

    /// output device graph in dot language so that it may be viewed
    #[argh(switch, short = 'g', long = "graph")]
    pub graph: bool,

    /// if this exists, the user will be prompted for a component to select.
    #[argh(switch, short = 's', long = "select")]
    pub select: bool,
}
