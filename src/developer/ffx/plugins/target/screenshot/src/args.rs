// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Result, anyhow};
use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;
use std::str::FromStr;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq, Clone)]
#[argh(
    subcommand,
    name = "screenshot",
    description = "Takes a screenshot of the target",
    example = "Store the target's screenshot in a directory:

    $ ffx target screenshot -d .
    Exported ./screenshot",
    note = "This command connects to a running target to take its screenshot.
The `--dir` can be supplied to override the default
screenshot saving location `/tmp/screenshot/YYYYMMDD_HHMMSS/`.
The --format can be supplied to override the default format png.
Accepted format values: bgra, rgba, png.
The screenshot file name is `screenshot.<format>`."
)]
pub struct ScreenshotCommand {
    // valid directory where the screenshot will be stored
    #[argh(
        option,
        long = "dir",
        short = 'd',
        description = "override the default directory where the screenshot will be saved"
    )]
    pub output_directory: Option<String>,

    #[argh(
        option,
        default = "Format::PNG",
        long = "format",
        description = "screenshot format. If no value is provided bgra will be used.
        Accepted values: bgra, rgba, png"
    )]
    pub format: Format,
}

#[derive(Debug, PartialEq, Clone)]
pub enum Format {
    BGRA,
    RGBA,
    PNG,
}

impl FromStr for Format {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, anyhow::Error> {
        match s.to_lowercase().as_str() {
            "bgra" => Ok(Format::BGRA),
            "rgba" => Ok(Format::RGBA),
            "png" => Ok(Format::PNG),
            _ => Err(anyhow!("invalid format, {}. Expected one of: bgra, rgba, png", s)),
        }
    }
}
