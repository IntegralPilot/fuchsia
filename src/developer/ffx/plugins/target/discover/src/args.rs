// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LoopMode {
    Foreground,
    Background,
}

fn loop_mode(lm: &str) -> Result<LoopMode, String> {
    match lm {
        "f" | "fg" | "foreground" => Ok(LoopMode::Foreground),
        "b" | "bg" | "background" => Ok(LoopMode::Background),
        _ => Err(format!(
            "loop mode must be either \"f(oreground)\" or \"b(ackground)\" (got {lm:?})"
        )),
    }
}

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq, Clone)]
#[argh(
    subcommand,
    name = "discover",
    description = "Discovers targets",
    short = 'd',
    note = "Discovers targets, storing them in the discovery cache. By default, runs as a background process."
)]
pub struct DiscoverCommand {
    #[argh(
        option,
        short = 'l',
        long = "loop",
        description = "run in a loop, in the specified mode (fg or bg)",
        from_str_fn(loop_mode)
    )]
    pub loop_mode: Option<LoopMode>,

    #[argh(switch, short = 'q', description = "do not write to stdout")]
    pub quiet: bool,

    #[argh(
        option,
        short = 't',
        description = "time in seconds between updates (default: 60). 0 to run once (only valid in foreground)"
    )]
    pub time: Option<u64>,

    #[argh(
        switch,
        description = "stop the discover process (whether in foreground or background)"
    )]
    pub stop: bool,

    #[argh(subcommand)]
    pub subcommand: Option<DiscoverSubCommand>,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq, Clone)]
#[argh(subcommand)]
pub enum DiscoverSubCommand {
    Clear(ClearCommand),
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq, Clone)]
#[argh(subcommand, name = "clear", description = "clear the discovery cache")]
pub struct ClearCommand {}
