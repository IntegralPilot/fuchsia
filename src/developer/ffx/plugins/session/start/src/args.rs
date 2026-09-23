// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "start",
    description = "Start the default session component.",
    note = "The default session component is specified by the URL `session_url`\
in `session_manager`'s structured configuration."
)]
pub struct SessionStartCommand {}
