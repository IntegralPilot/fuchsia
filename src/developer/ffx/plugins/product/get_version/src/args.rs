// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use camino::Utf8PathBuf;

/// Get the product version of a Product Bundle.
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "get-version")]
pub struct GetVersionCommand {
    /// when true, return version info for all included assembly artifacts.
    #[argh(switch)]
    pub include_dependencies: bool,

    /// path to product bundle or assembly artifact directory.
    #[argh(positional)]
    pub artifact: Utf8PathBuf,
}
