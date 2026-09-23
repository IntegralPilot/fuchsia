// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "doctor", description = "Diagnose driver binding issues")]
pub struct DoctorCommand {
    /// URL or a substring of the URL of the driver to diagnose. The command will fail if multiple drivers match.
    #[argh(option)]
    pub driver: Option<String>,

    /// moniker of the node to diagnose.
    #[argh(option)]
    pub node: Option<String>,

    /// if this exists, the user will be prompted for a component to select.
    #[argh(switch, short = 's', long = "select")]
    pub select: bool,

    /// name of the composite node spec to diagnose.
    #[argh(option)]
    pub composite_node_spec: Option<String>,
}
