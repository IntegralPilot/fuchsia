// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "list",
    description = "List driver hosts and drivers loaded within them",
    example = "To list all driver hosts:

    $ driver host list",
    error_code(1, "Failed to connect to the driver development service")
)]
pub struct ListCommand {}
