// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use component_debug_fdomain::cli::list::ListFilter;
use ffx_core::ffx_command;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "list",
    description = "Lists components in the component topology. Children of unresolved components are not included in this list.",
    short = 'l',
    example = "To list components in the topology:

    $ ffx component list

    To list all running components in the topology:

    $ ffx component list --only running

    To list all stopped components in the topology:

    $ ffx component list --only stopped

    To list the ancestors of a component named `foo`:

    $ ffx component list --only ancestor:foo

    To list the descendants of a component named `foo`:

    $ ffx component list --only descendant:foo

    To list both the ancestors and descendants of a component named `foo`:

    $ ffx component list --only relatives:foo"
)]

pub struct ComponentListCommand {
    #[argh(option, long = "only", short = 'o')]
    /// filter the instance list by a criteria: running, stopped, ancestors:<component_name>, descendants:<component_name>, or relatives:<component_name>
    pub filter: Option<ListFilter>,

    #[argh(switch, long = "verbose", short = 'v')]
    /// show detailed information about each instance
    pub verbose: bool,
}
