// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use camino::Utf8PathBuf;
use std::path::PathBuf;
use vbmeta::VBMeta;

/// Command-line arguments for the vbmeta tool.
#[derive(argh::FromArgs)]
struct Args {
    /// path to a ZBI.
    #[argh(option)]
    zbi: PathBuf,

    /// path to a private key PEM.
    #[argh(option)]
    private_key_pem: PathBuf,

    /// path to a public key metadata file.
    #[argh(option)]
    public_key_metadata: PathBuf,

    /// path at which to output the corresponding VBMeta.
    #[argh(option)]
    output: PathBuf,
}

fn main() -> Result<()> {
    let args: Args = argh::from_env();

    let output_path =
        Utf8PathBuf::try_from(args.output).context("converting output path to UTF-8")?;
    let outdir = output_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("output path must have a parent directory"))?;
    let name = output_path
        .file_stem()
        .ok_or_else(|| anyhow::anyhow!("output path must have a file stem"))?;

    VBMeta::builder(name, Utf8PathBuf::try_from(args.private_key_pem)?)
        .key_metadata(Utf8PathBuf::try_from(args.public_key_metadata)?)
        .hash_descriptor("zircon", Utf8PathBuf::try_from(args.zbi)?)
        .construct(outdir)?;

    Ok(())
}
