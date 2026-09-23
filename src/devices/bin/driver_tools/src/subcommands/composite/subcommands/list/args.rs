// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "list",
    description = "List composite node specs",
    example = "To list all composite node specs:

    $ driver composite list

To list only bound composite node specs:

    $ driver composite list --only bound",
    error_code(1, "Failed to connect to the driver development service")
)]
pub struct ListCompositeCommand {
    /// shows the composite's state and bound driver url if one exists.
    #[argh(switch, short = 'v', long = "verbose")]
    pub verbose: bool,

    #[argh(option, long = "only", short = 'o')]
    /// filter the list by a criteria: bound, unbound, incomplete
    pub filter: Option<CompositeFilter>,

    #[argh(positional)]
    /// optional name filter. Partial matches allowed.
    pub name: Option<String>,
}

#[derive(Debug, PartialEq, Clone)]
pub enum CompositeFilter {
    Bound,
    Unbound,
    Incomplete,
}

impl std::str::FromStr for CompositeFilter {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "bound" => Ok(CompositeFilter::Bound),
            "unbound" => Ok(CompositeFilter::Unbound),
            "incomplete" => Ok(CompositeFilter::Incomplete),
            _ => Err("Invalid filter. Must be 'bound', 'unbound', or 'incomplete'"),
        }
    }
}
