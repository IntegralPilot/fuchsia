// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use camino::Utf8PathBuf;

/// Get the info of repository inside a Product Bundle.
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "get-repository")]
pub struct GetRepositoryCommand {
    /// path to product bundle directory.
    #[argh(positional)]
    pub product_bundle: Utf8PathBuf,
}
