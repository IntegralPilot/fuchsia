// Copyright 2025 The Fuchsia Authors. All rights reserved.
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

mod config;
mod templates;

/// Generate Rust bindings from FIDL IR
#[derive(FromArgs)]
pub struct Fidlgen {
    /// source JSON IR file path
    #[argh(option)]
    json: PathBuf,
    /// source config file path
    #[argh(option)]
    config: PathBuf,
    /// output file path
    #[argh(option)]
    output_filename: PathBuf,
    /// rustfmt binary path
    #[argh(option)]
    rustfmt: PathBuf,
    /// rustfmt configuration file path
    #[argh(option)]
    rustfmt_config: PathBuf,
    /// name of the common crate, which contains the POD parts of this library.
    #[argh(option)]
    common_lib: Option<String>,
}

fn main() {
    let args = argh::from_env::<Fidlgen>();

    let file = File::open(&args.json).expect("failed to open source JSON IR file");
    let library = serde_json::from_reader::<_, Library>(BufReader::new(file))
        .expect("failed to parse source JSON IR");

    let file = File::open(&args.config).expect("failed to open source JSON config file");
    let mut config = serde_json::from_reader::<_, self::config::Config>(BufReader::new(file))
        .expect("failed to parse source JSON IR");
    config.common_lib = args.common_lib;

    if config.is_common && config.common_lib.is_some() {
        panic!("Common crate cannot have a common crate");
    }

    let context = templates::Context::new(library, config);
    let result =
        templates::LibraryTemplate::new(&context).render().expect("failed to emit FIDL bindings");
    let result = trim_trailing_whitespace(&result);

    std::fs::write(&args.output_filename, result).expect("failed to write to output file");

    // TODO(540526942): rustfmt is not idempotent right now. As a workaround,
    // just format the file twice.
    const FORMAT_PASSES: usize = 2;
    for _ in 0..FORMAT_PASSES {
        Command::new(&args.rustfmt)
            .arg("--config-path")
            .arg(&args.rustfmt_config)
            .arg(&args.output_filename)
            .status()
            .expect("failed to format output file");
    }
}
