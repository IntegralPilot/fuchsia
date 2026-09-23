// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;

/// Options for "ffx debug symbolize".
#[ffx_command()]
#[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
#[argh(
    subcommand,
    name = "symbolize",
    description = "symbolize backtraces in markup format",
    short = 's'
)]
pub struct SymbolizeCommand {
    /// start the authentication process.
    #[argh(switch)]
    pub auth: bool,

    /// do not prettify the backtraces.
    #[argh(switch)]
    pub no_prettify: bool,

    /// extra arguments passed to the symbolizer. Any arguments starting with "-" must be after a "--" separator.
    #[argh(positional)]
    pub symbolizer_args: Vec<String>,
}
