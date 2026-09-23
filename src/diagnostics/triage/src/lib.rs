// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod app; // Runs the config binary
pub mod config; // Read the config file(s) for metric and action specs.
pub mod file_io;

#[cfg(test)]
mod test;

use crate::config::OutputFormat;
use clap::Parser;

#[derive(Parser, Debug, Clone)]
pub struct Options {
    /// Config files
    #[arg(long = "config")]
    config_files: Vec<String>,

    /// Directory to read diagnostic (snapshot) files from.
    #[arg(long = "data")]
    data_directory: String,

    /// How to print the results.
    #[arg(long = "output-format", default_value = "text")]
    output_format: OutputFormat,

    /// Which tags we should include
    #[arg(long = "tag")]
    tags: Vec<String>,

    /// Which tags we should exclude
    #[arg(long = "exclude-tag")]
    exclude_tags: Vec<String>,
}
