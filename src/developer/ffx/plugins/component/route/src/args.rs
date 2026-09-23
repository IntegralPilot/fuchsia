// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "route",
    description = "Perform capability routing on a component at runtime.",
    example = "To route capabilities from `font_provider`:

$ ffx component route /core/font_provider

This will perform routing on all the capabilities used or exposed by `font_provider`, and display
information for each route including its status (success or failure) and the identity of the source
component providing the capability.

$ ffx component route fonts.cm

This does the same thing, except with a fuzzy match (on the URL).

$ ffx component route /core/font_provider fuchsia.pkg.FontResolver

This will perform routing on capabilities used or exposed by `font_provider` that match
`fuchsia.pkg.FontResolver`.

$ ffx component route /core/font_provider use:fuchsia.pkg.FontResolver,expose:fuchsia.fonts.Provider

This will perform routing from `font_provider` on used capability `fuchsia.pkg.FontResolver` and
exposed capability `fuchsia.fonts.Provider`.

$ ffx component route /core/font_provider fuchsia.pkg

Fuzzy matching by capability is also supported. This will perform routing from `font_provider` on
any used or exposed capability matching *fuchsia.pkg*.
"
)]
pub struct RouteCommand {
    #[argh(positional)]
    /// component URL, moniker or instance ID of target component. Partial matches allowed.
    pub target: String,

    #[argh(positional)]
    /// optional filter of comma-separated capability names or <decl type>:<capability> pairs.
    /// If provided, <decl type> is one of `use` or `expose`, otherwise it will match both used
    /// and exposed capabilities. Partial matches on <capability> are allowed.
    ///
    /// Examples:
    /// - fuchsia.pkg.FontResolver
    /// - use:fuchsia.pkg.FontResolver
    /// - expose:fuchsia.fonts.Provider
    /// - fuchsia.pkg.FontResolver,expose:fuchsia.fonts.Provider
    /// - fuchsia.pkg
    pub filter: Option<String>,
}
