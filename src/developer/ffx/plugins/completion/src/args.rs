// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "completion",
    description = "Generate shell completions for ffx",
    example = "To load completions for zsh, run:
  $ source <(ffx completion zsh)

To load completions for fish, run:
  $ ffx completion fish | source -

To load completions for bash, run:
  $ source <(ffx completion bash)"
)]
pub struct CompletionCommand {
    #[argh(positional)]
    /// the shell to generate completions for: "bash", "zsh", or "fish"
    pub shell: String,
}
