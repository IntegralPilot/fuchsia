// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "config",
    description = "Manages configuration capability override values for components"
)]
pub struct ConfigComponentCommand {
    #[argh(subcommand)]
    pub subcommand: SubCommandEnum,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand)]
pub enum SubCommandEnum {
    Set(SetArgs),
    Unset(UnsetArgs),
    List(ListArgs),
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "set",
    description = "Sets configuration capability override values for the specified component",
    example = "To override the configuration fields `bar` and `baz` for the component `/core/ffx-laboratory:foo`:

    $ ffx component config set /core/ffx-laboratory:foo bar=true baz=42

    Use the reload flag to cause the component to reload the component with the override in effect:

    $ ffx component config set --reload /core/ffx-laboratory:foo bar=true

    To override a vector configuration field, use a comma separated list:

    $ ffx component config set /core/ffx-laboratory:foo \"some_list=1, 2, 3\"
    "
)]
pub struct SetArgs {
    #[argh(positional)]
    /// component URL, moniker or instance ID. Partial matches allowed.
    pub query: String,
    #[argh(positional)]
    /// key-value pairs of the configuration capability to be overridden. Takes
    /// the form 'key="value"'.
    pub key_values: Vec<String>,
    #[argh(switch, short = 'r')]
    /// if enabled, component instance will be immediately reloaded so overrides
    /// take effect.
    pub reload: bool,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "unset",
    description = "Unsets structured configuration override values for the specified component",
    example = "To unset overrides for the component `core/ffx-laboratory:foo`

    $ ffx component config unset `core/ffx-laboratory:foo`

    To unset overrides for all components with overrides:

    $ ffx component config unset
    "
)]
pub struct UnsetArgs {
    #[argh(positional)]
    /// component URL, moniker or instance ID. Partial matches allowed. If
    /// empty, unsets structured config overrides for all components.
    pub query: Option<String>,
    #[argh(switch, short = 'r')]
    /// if enabled, component instance will be immediately reloaded so overrides
    /// take effect.
    pub reload: bool,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "list",
    description = "Lists structured configuration values for the specified component"
)]
pub struct ListArgs {
    #[argh(positional)]
    /// component URL, moniker or instance ID. Partial matches allowed.
    pub query: String,
}
