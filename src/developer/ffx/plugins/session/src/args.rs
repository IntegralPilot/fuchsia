// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;
use ffx_session_sub_command::SubCommand;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "session",
    description = "Control the session component.",
    note = "See https://fuchsia.dev/fuchsia-src/glossary#session-component."
)]
pub struct SessionCommand {
    #[argh(subcommand)]
    pub subcommand: SubCommand,
}
