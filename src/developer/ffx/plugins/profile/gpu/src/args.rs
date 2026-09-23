// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;
use ffx_profile_gpu_sub_command::SubCommand;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "gpu", description = "Access GPU usage information")]
/// Top-level command for "ffx profile gpu".
pub struct GpuCommand {
    #[argh(subcommand)]
    pub subcommand: SubCommand,
}
