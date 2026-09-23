// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "collaborative-reboot",
    description = "Interact with collaborative reboot"
)]
/// Top-level command for "power collaborative-reboot".
pub struct CollaborativeRebootCommand {
    #[argh(subcommand)]
    pub subcommand: SubCommand,
}

#[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
#[argh(subcommand)]
pub enum SubCommand {
    PerformPendingReboot(PerformPendingRebootCommand),
}

#[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
/// Perform a pending Collaborative Reboot, if any.
#[argh(subcommand, name = "perform-pending-reboot")]
pub struct PerformPendingRebootCommand {}
