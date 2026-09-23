// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::FromArgs;

#[derive(Debug, FromArgs)]
/// Read a GN's desc json output, and perform queries of its contents.
pub struct Args {
    /// the 'gn desc' json file to read
    #[argh(option)]
    pub file: String,

    /// verbose output
    #[argh(switch, short = 'v')]
    pub verbose: bool,

    #[argh(subcommand)]
    pub select: crate::commands::selectors::NodeSelector,
}
