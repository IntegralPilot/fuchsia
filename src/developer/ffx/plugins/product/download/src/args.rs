// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use pbms::AuthFlowChoice;
use std::path::PathBuf;

/// Download Product Bundle from GCS.
#[derive(ArgsInfo, FromArgs, Debug, PartialEq, Clone)]
#[argh(
    subcommand,
    name = "download",
    example = "\
    Sample invocations:

    // Download core.vim3 based on current ffx version. Note it will download
    // the latest version of the date section.
    ffx product download core.vim3 ~/local_pb

    // Download core.vim3 with version 19.20240302.2.1
    ffx product download core.vim3 ~/local_pb --version 19.20240302.2.1

    // Download core.vim3 for latest version of f18
    ffx product download core.vim3 ~/local_pb --branch f18 --force

    Auth flow choices for --auth include:

      `--auth no-auth` do not use auth.
      `--auth pkce` to use PKCE auth flow (requires GUI browser).
      `--auth device-experimental` to use device flow.
      `--auth <path/to/exe>` run tool at given path which will print an access
        token to stdout and exit 0.
      `--auth default` let the tool decide which auth flow to use.
    "
)]
pub struct DownloadCommand {
    /// get the data again, even if it's already present locally.
    #[argh(switch)]
    pub force: bool,

    /// use specific auth flow for oauth2 (see examples; default: pkce).
    #[argh(option, default = "AuthFlowChoice::Default")]
    pub auth: AuthFlowChoice,

    /// url to the transfer manifest of the product bundle to download, or the
    /// product name to fetch.
    #[argh(positional)]
    pub manifest_url: String,

    /// path to the local directory to download the product bundle.
    #[argh(positional)]
    pub product_dir: PathBuf,

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
