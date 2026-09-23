// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "remove",
    description = "Remove an element from the current session.

Persistent elements will be removed permanently. Any persistent storage used by
elements will *not* be removed."
)]
pub struct SessionRemoveCommand {
    /// name of the element to remove
    #[argh(positional)]
    pub name: String,
}
