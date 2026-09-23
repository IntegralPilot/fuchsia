// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use camino::Utf8PathBuf;
use product_bundle::Type;

/// Get the paths of a group of artifacts inside a Product Bundle.
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "get-artifacts")]
pub struct GetArtifactsCommand {
    /// path to product bundle directory. Defaults to the configured
    /// path in `product.path`.
    #[argh(positional)]
    pub product_bundle: Option<Utf8PathBuf>,

    /// select what group of artifacts to list.
    /// One of flash, emu, update, bootloader, all
    #[argh(option, short = 'g')]
    pub artifacts_group: Type,

    /// return relative path or not
    #[argh(switch, short = 'r')]
    pub relative_path: bool,
}
