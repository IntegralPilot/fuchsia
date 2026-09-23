// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use pbms::AuthFlowChoice;

/// List all available products for a specific SDK version.
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "list",
    example = "\
    Sample invocations:

    // List all product names based on current ffx version. Note it will list
    // the latest version of the date section.
    ffx product list

    // List all product names with version 19.20240302.2.1
    ffx product list --version 19.20240302.2.1

    // List all product names for latest version of f18
    ffx product list --branch f18

    Auth flow choices for --auth include:

      `--auth no-auth` do not use auth.
      `--auth pkce` to use PKCE auth flow (requires GUI browser).
      `--auth device-experimental` to use device flow.
      `--auth <path/to/exe>` run tool at given path which will print an access
        token to stdout and exit 0.
      `--auth default` let the tool decide which auth flow to use.
    "
)]
pub struct ListCommand {
    /// use specific auth flow for oauth2 (see examples; default: pkce).
    #[argh(option, default = "AuthFlowChoice::Default")]
    pub auth: AuthFlowChoice,

    /// location to look for product bundles manifest inside GCS.
    #[argh(option)]
    pub base_url: Option<String>,

    /// filter on products of <version>. The version number is in the format of
    /// `a.b.c.d`. e.g. 19.20240302.2.1. If this value is not passed in, the
    /// version will be defaulted to version of ffx tool itself.
    #[argh(option)]
    pub version: Option<String>,

    /// filter on products of <branch>. The branch is either in the form of f<N>
    /// (e.g. f18) or `LATEST`. This option is exclusive with version option.
    #[argh(option)]
    pub branch: Option<String>,
}
