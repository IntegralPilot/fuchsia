// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "static-checks",
    description = "Run static checks against a driver file."
)]
pub struct StaticChecksCommand {
    /// the path to the driver's FAR file
    #[argh(option)]
    pub driver: String,
}
