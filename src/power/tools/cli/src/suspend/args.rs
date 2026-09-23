// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use arg_parsing::parse_duration;
use argh::{ArgsInfo, FromArgs};

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "suspend", description = "Control system suspend behavior")]
pub struct SuspendCommand {
    #[argh(subcommand)]
    pub subcommand: SuspendSubcommand,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand)]
pub enum SuspendSubcommand {
    Prevent(PreventCommand),
    Allow(AllowCommand),
}

#[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
/// Prevent system from suspending.
#[argh(subcommand, name = "prevent")]
pub struct PreventCommand {
    #[argh(switch)]
    /// drop existing lease and re-take it after wait_time.
    pub restart: bool,

    #[argh(option, default = "parse_duration(\"100ms\").unwrap()", from_str_fn(parse_duration))]
    /// the duration the system waits before starting application activity again (e.g. 100ms, 5s).
    /// The system is not guaranteed to start again after this time, but on the next wakeup
    /// this command will take a lease on application activity.
    /// Defaults to 100ms.
    pub wait_time: std::time::Duration,
}

#[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
/// Allow system to suspend.
#[argh(subcommand, name = "allow")]
pub struct AllowCommand {}
