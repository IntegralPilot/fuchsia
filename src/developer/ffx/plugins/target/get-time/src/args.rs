// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq, Clone)]
#[argh(
    subcommand,
    name = "get-time",
    description = "Returns the current time in the system monotonic clock. \
This is the number of nanoseconds since the system was powered on. \
It does not always reset on reboot and does not adjust during sleep, \
and thus should not be used as a reliable source of uptime. \
\
See https://fuchsia.dev/reference/syscalls/clock_get_monotonic",
    example = "To get the target time:

    $ ffx target get-time
"
)]

pub struct GetTimeCommand {
    #[argh(switch, short = 'b', description = "if true, return boot time")]
    pub boot: bool,
}
