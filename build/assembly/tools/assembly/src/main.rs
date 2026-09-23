// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod create_system;
mod product;

use anyhow::Result;
use argh::FromArgs;
use assembly_cli_args::{CreateSystemArgs, ProductArgs};

/// Command-line arguments for executing assembly.
#[derive(FromArgs)]
struct Args {
    #[argh(subcommand)]
    command: Subcommand,
}

#[derive(FromArgs)]
#[argh(subcommand)]
#[allow(clippy::large_enum_variant)]
enum Subcommand {
    Product(ProductArgs),
    CreateSystem(CreateSystemArgs),
}

#[fuchsia::main(logging_minimum_severity = "warn")]
async fn main() -> Result<()> {
    let args: Args = argh::from_env();

    match args.command {
        Subcommand::Product(args) => product::assemble(args),
        Subcommand::CreateSystem(args) => create_system::create_system(args).await,
    }
}
