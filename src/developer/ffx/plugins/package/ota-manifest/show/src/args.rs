// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use camino::Utf8PathBuf;
use ffx_core::ffx_command;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, Eq, PartialEq)]
#[argh(subcommand, name = "show", description = "Show the contents of a signed ota-manifest.")]
pub struct ShowCommand {
    /// path to the manifest file
    #[argh(positional)]
    pub manifest: Utf8PathBuf,

    /// optional public key file for verifying the manifest signature
    #[argh(option)]
    pub public_key: Option<Utf8PathBuf>,

    /// whether to print the full list of blobs (default is false outside of machine mode)
    #[argh(switch)]
    pub print_blobs: bool,
}
