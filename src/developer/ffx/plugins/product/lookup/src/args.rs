// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use pbms::AuthFlowChoice;

/// Retrieve image data.
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "lookup",
    example = "\
    Auth flow choices for --auth include:\
    \n  `--auth oob` to use Out-of-Band auth (deprecated).\
    \n  `--auth pkce` to use PKCE auth flow (requires GUI browser).\
    \n  `--auth <path/to/exe>` run tool at given path which will print an \
    access token to stdout and exit 0.
    "
)]
pub struct LookupCommand {
    /// use specific auth flow for oauth2 (see examples; default: pkce).
    #[argh(option, default = "AuthFlowChoice::Default")]
    pub auth: AuthFlowChoice,

    /// where to look for product bundles manifest.
    #[argh(option)]
    pub base_url: Option<String>,

    /// filter on products with <name>.
    #[argh(positional)]
    pub name: String,

    /// filter on products of <version>.
    #[argh(positional)]
    pub version: String,
}
