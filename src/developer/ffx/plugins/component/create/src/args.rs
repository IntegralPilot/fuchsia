// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use component_debug_fdomain::config::RawConfigEntry;
use ffx_core::ffx_command;
use fuchsia_url::fuchsia_pkg::AbsoluteComponentUrl;
use moniker::Moniker;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "create",
    description = "Creates a dynamic component instance, adding it to the collection designated by <moniker>",
    example = "To create a component instance designated by the moniker `/core/ffx-laboratory:foo`:

    $ ffx component create /core/ffx-laboratory:foo fuchsia-pkg://fuchsia.com/hello-world-rust#meta/hello-world-rust.cm",
    note = "To learn more about running components, see https://fuchsia.dev/go/components/run"
)]

pub struct CreateComponentCommand {
    #[argh(positional)]
    /// moniker of a component instance in an existing collection. See https://fuchsia.dev/fuchsia-src/reference/components/moniker
    /// The component instance will be added to the collection if the command
    /// succeeds.
    pub moniker: Moniker,

    #[argh(positional)]
    /// url of the component to create.
    pub url: AbsoluteComponentUrl,

    #[argh(option)]
    /// provide a configuration override to the component being run. Requires
    /// `mutability: [ "parent" ]` on the configuration field. Specified in the format
    /// `KEY=VALUE` where `VALUE` is a JSON string which can be resolved as the correct type of
    /// configuration value.
    pub config: Vec<RawConfigEntry>,
}
