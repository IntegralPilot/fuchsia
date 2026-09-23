// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;

/// Options for "ffx debug connect".
#[ffx_command()]
#[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
#[argh(
    subcommand,
    name = "connect",
    description = "start the debugger and connect to the target",
    short = 'c'
)]
pub struct ConnectCommand {
    /// start zxdb in another debugger. Currently, the only valid option is "lldb".
    #[argh(option)]
    pub debugger: Option<String>,

    /// only start the debug agent but not the zxdb. The path to the UNIX socket will be printed
    /// and can be connected via "connect -u" in zxdb shell.
    #[argh(switch)]
    pub agent_only: bool,

    /// attaches to given processes. The argument will be parsed in the same way as the "attach"
    /// command in the console.
    #[argh(option, short = 'a')]
    pub attach: Vec<String>,

    /// execute one zxdb command. Multiple commands will be executed sequentially.
    #[argh(option, short = 'e')]
    pub execute: Vec<String>,

    /// always spawn a new DebugAgent instance for this zxdb invocation.
    #[argh(switch)]
    pub new_agent: bool,

    /// extra arguments passed to zxdb. Any arguments starting with "-" must be after a "--" separator.
    #[argh(positional)]
    pub zxdb_args: Vec<String>,
}
