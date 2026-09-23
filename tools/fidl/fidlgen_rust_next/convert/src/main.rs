// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::process::Command;

use argh::FromArgs;
use askama::Template as _;
use fidl_ir::Library;
use fidlgen::trim_trailing_whitespace;

mod context;
mod ident;
mod templates;

/// Generate conversion functions between old and new FIDL bindings.
#[derive(FromArgs)]
pub struct Args {
    /// source JSON IR file path
    #[argh(option)]
    json: PathBuf,
    /// output file path
    #[argh(option)]
    output_filename: PathBuf,
    /// rustfmt binary path
    #[argh(option)]
    rustfmt: PathBuf,
    /// rustfmt configuration file path
    #[argh(option)]
    rustfmt_config: PathBuf,
    /// name of the rust bindings crate
    #[argh(option)]
    rust_crate: String,
    /// name of the rust_next bindings crate
    #[argh(option)]
    rust_next_crate: String,
}

fn main() {
    let args = argh::from_env::<Args>();

    let file = File::open(&args.json).expect("failed to open JSON IR file");
    let library = serde_json::from_reader::<_, Library>(BufReader::new(file))
        .expect("failed to parse source JSON IR");

    let context = context::Context {
        library,
        rust_crate: args.rust_crate,
        rust_next_crate: args.rust_next_crate,
    };
    let result = templates::LibraryTemplate::new(&context)
        .render()
        .expect("failed to emit FIDL conversions");
    let result = trim_trailing_whitespace(&result);

    std::fs::write(&args.output_filename, result).expect("failed to write to output file");

    Command::new(&args.rustfmt)
        .arg("--config-path")
        .arg(&args.rustfmt_config)
        .arg(&args.output_filename)
        .status()
        .expect("failed to format output file");
}
