// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;

#[ffx_command()]
#[derive(Debug, Default, ArgsInfo, FromArgs, PartialEq)]
#[argh(
    subcommand,
    name = "status",
    description = "Run status checks step by step to determine a device's state"
)]
pub struct TargetStatus {
    #[argh(positional, short = 't', default = "1.0")]
    /// the timeout in fractional seconds for connecting to the device ID component.
    pub proxy_connect_timeout: f64,
}
