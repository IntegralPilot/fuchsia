// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use {
    anyhow::Error,
    fidl_ir_lib::fidl::FidlIr,
    std::{
        fs::{write, File},
        path::PathBuf,
    },
    clap::Parser,
};

#[derive(Parser, Debug)]
struct Flags {
    #[arg(long)]
    input: PathBuf,

    #[arg(long)]
    stamp: PathBuf,
}

fn main() -> Result<(), Error> {
    let flags = Flags::parse();
    let _result: FidlIr = serde_json::from_reader(File::open(flags.input)?)?;
    write(flags.stamp, "Done!")?;
    Ok(())
}
