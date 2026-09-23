// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::FromArgs;

mod bind_generator;
mod board_compiler;
mod cml_generator;
mod cpp_generator;
mod driver_compiler;
mod parser;
mod rust_generator;
mod workarounds;

#[derive(FromArgs)]
/// DML Compiler
struct Args {
    #[argh(subcommand)]
    subcommand: Subcommand,
}

#[derive(FromArgs)]
#[argh(subcommand)]
enum Subcommand {
    CompileBoard(CompileBoardArgs),
    CompileDriver(CompileDriverArgs),
}

#[derive(FromArgs, Clone)]
#[argh(subcommand, name = "compile-board")]
/// Compile board configuration DML.
pub struct CompileBoardArgs {
    #[argh(positional)]
    /// input board DML file
    pub input_file: String,

    #[argh(option)]
    /// output directory for generated board config and manifest/bind files
    pub out_dir: Option<String>,

    #[argh(option)]
    /// path to output board config FIDL file
    pub fidl_output: Option<String>,

    #[argh(option)]
    /// path to output CML file
    pub cml_output: Option<String>,

    #[argh(option)]
    /// path to output bind file
    pub bind_output: Option<String>,

    #[argh(option)]
    /// driver DML files to load metadata schemas from
    pub driver_dml: Vec<String>,
}

#[derive(FromArgs, Clone)]
#[argh(subcommand, name = "compile-driver")]
/// Compile driver DML (generates bind rules, component manifests, and metadata C++ parser).
pub struct CompileDriverArgs {
    #[argh(positional)]
    /// input driver DML file
    pub input_file: String,

    #[argh(option)]
    /// path to output C++ parser header file
    pub h_output: Option<String>,

    #[argh(option)]
    /// path to output C++ parser source file
    pub cc_output: Option<String>,

    #[argh(option)]
    /// path to output Rust parser source file
    pub rs_output: Option<String>,

    #[argh(option)]
    /// path to output CML file
    pub cml_output: Option<String>,

    #[argh(option)]
    /// path to output bind file
    pub bind_output: Option<String>,

    #[argh(option)]
    /// namespace for the generated C++ parser
    pub namespace: Option<String>,
}

fn get_current_year() -> String {
    use chrono::Datelike;
    chrono::Utc::now().year().to_string()
}

fn main() -> Result<(), anyhow::Error> {
    let args: Args = argh::from_env();
    let year = get_current_year();
    match args.subcommand {
        Subcommand::CompileBoard(board_args) => board_compiler::compile_board(&board_args, &year),
        Subcommand::CompileDriver(driver_args) => {
            driver_compiler::compile_driver(&driver_args, &year)
        }
    }
}
