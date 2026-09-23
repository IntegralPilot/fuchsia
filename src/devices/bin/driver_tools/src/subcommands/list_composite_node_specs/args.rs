// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "list-composite-node-specs",
    description = "List composite node specs.",
    example = "To list all composite node specs with properties:

    $ driver list-composite-node-specs -v

To show a specific composite nod spec, specify a `--name` or `-n` for short:

    $ driver list-composite-node-specs -n example_group",
    error_code(1, "Failed to connect to the driver development service")
)]
pub struct ListCompositeNodeSpecsCommand {
    /// list all driver properties.
    #[argh(switch, short = 'v', long = "verbose")]
    pub verbose: bool,

    /// only show the spec with this name.
    #[argh(option, short = 'n', long = "name")]
    pub name: Option<String>,

    /// if this exists, the user will be prompted for a component to select.
    #[argh(switch, short = 's', long = "select")]
    pub select: bool,
}
