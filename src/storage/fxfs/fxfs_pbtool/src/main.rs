// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use argh::FromArgs;
use fxfs_make_blob_image;
use std::path::PathBuf;

/// A tool to interact with Fxfs images within product bundles.
#[derive(FromArgs, Debug)]
struct FxfsToolArgs {
    #[argh(subcommand)]
    command: Command,
}

#[derive(FromArgs, Debug)]
#[argh(subcommand)]
enum Command {
    Extract(ExtractArgs),
}

/// Extracts blobs from an Fxfs image.
#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "extract")]
struct ExtractArgs {
    /// path to the source fxfs sparse image
    #[argh(option)]
    image: PathBuf,

    /// path to the destination directory where extracted blobs will be written
    #[argh(option)]
    out: PathBuf,
}

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    let args: FxfsToolArgs = argh::from_env();

    match args.command {
        Command::Extract(args) => fxfs_make_blob_image::extract_blobs(args.image, args.out).await,
    }
}
