// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use argh::FromArgs;

mod commands;
mod common;
mod descriptor_types;
#[expect(dead_code, reason = "Used in subsequent CLs and unit tests")]
mod indented_serializer;
mod input_report_types;

#[derive(FromArgs, Debug)]
/// A tool to dump input reports from input devices.
struct Args {
    #[argh(subcommand)]
    subcommand: Subcommands,
}

#[derive(FromArgs, Debug)]
#[argh(subcommand)]
enum Subcommands {
    List(commands::list::ListArgs),
    GetDescriptor(commands::get_descriptor::GetDescriptorArgs),
    Read(commands::read::ReadArgs),
}

#[fuchsia::main]
async fn main() -> Result<()> {
    let args: Args = argh::from_env();

    match args.subcommand {
        Subcommands::List(list_args) => commands::list::run(list_args).await,
        Subcommands::GetDescriptor(desc_args) => commands::get_descriptor::run(desc_args).await,
        Subcommands::Read(read_args) => commands::read::run(read_args).await,
    }
}
