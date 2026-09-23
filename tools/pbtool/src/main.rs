// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![deny(missing_docs)]

//! Manage product bundles, prepare them for OTAs, and convert them to build
//! archives.

mod build_archive;
mod product_description;
mod transfer_manifest;

use crate::product_description::GenerateProductDescription;
use crate::transfer_manifest::GenerateTransferManifest;
use anyhow::Result;
use argh::FromArgs;
use build_archive::GenerateBuildArchive;

/// Tool for managing product bundles that is specific to fuchsia infrastructure.
#[derive(FromArgs, PartialEq, Debug)]
struct Command {
    /// the nested subcommands.
    #[argh(subcommand)]
    sub: Subcommand,
}

/// Subcommands for pbtool.
#[derive(FromArgs, PartialEq, Debug)]
#[argh(subcommand)]
enum Subcommand {
    /// Generate a build archive.
    GenerateBuildArchive(GenerateBuildArchive),

    /// Generate a description of the product bundle with a link to the transfer
    /// manifest json file.
    GenerateProductDescription(GenerateProductDescription),

    /// Generate the transfer manifest which lists how to upload/download the
    /// product bundle.
    GenerateTransferManifest(GenerateTransferManifest),
}

#[fuchsia::main]
async fn main() -> Result<()> {
    let command = argh::from_env::<Command>();
    match command.sub {
        Subcommand::GenerateBuildArchive(cmd) => cmd.generate(),
        Subcommand::GenerateProductDescription(cmd) => cmd.generate(),
        Subcommand::GenerateTransferManifest(cmd) => cmd.generate().await,
    }
}
