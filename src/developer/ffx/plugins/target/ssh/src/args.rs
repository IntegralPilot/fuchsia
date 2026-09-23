// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Default, Debug, Eq, PartialEq)]
#[argh(
    subcommand,
    name = "ssh",
    description = "SSH to a target device",
    example = "To ssh to a specific device:

    $ ffx -t fuchsia-EEEE-NNNN target ssh

To ssh and run a command:

    $ ffx target ssh 'echo $USER'
"
)]
pub struct SshCommand {
    // Custom ssh config file
    #[argh(option, description = "path to the custom ssh config file to use.")]
    pub sshconfig: Option<String>,

    #[argh(positional, description = "command to run on the target. If blank drops into a shell")]
    pub command: Vec<String>,
}
