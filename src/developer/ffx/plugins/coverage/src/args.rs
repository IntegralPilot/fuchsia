// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use std::path::PathBuf;

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "coverage", description = "Show coverage from test outputs")]

pub struct CoverageCommand {
    /// path to ffx test output directory
    #[argh(option)]
    pub test_output_dir: PathBuf,

    /// path to clang directory, llvm-profdata and llvm-cov are expected in clang_dir/bin
    #[argh(option)]
    pub clang_dir: PathBuf,

    /// path to symbol index json to load symbol index from
    #[argh(option)]
    pub symbol_index_json: Option<PathBuf>,

    /// directory to export HTML reports to
    #[argh(option)]
    pub export_html: Option<PathBuf>,

    /// path to export JSON file to
    #[argh(option)]
    pub export_json: Option<PathBuf>,

    /// path to export LCOV file to
    #[argh(option)]
    pub export_lcov: Option<PathBuf>,

    /// a "<from>,<to>" remapping of source file paths passed through to llvm-cov
    #[argh(option)]
    pub path_remappings: Vec<String>,

    /// path to the directory used as a base for relative coverage mapping paths, passed through to llvm-cov
    #[argh(option)]
    pub compilation_dir: Option<PathBuf>,

    /// regular expression pattern to only include matching source files
    #[argh(option)]
    pub include_filename_regex: Option<String>,

    /// regular expression pattern to skip matching source files
    #[argh(option)]
    pub ignore_filename_regex: Option<String>,

    /// paths to source files to show coverage for
    #[argh(positional)]
    pub src_files: Vec<PathBuf>,

    /// if set, print verbose information about each command being run, for debugging
    #[argh(switch, short = 'v')]
    pub verbose: bool,
}
