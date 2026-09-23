// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "restart",
    description = "Restart the current session component.",
    note = "Destroys the existing session component, if any, then creates and \
starts a new component with the same URL. Requires that a session component\
was previously launched."
)]
pub struct SessionRestartCommand {}
