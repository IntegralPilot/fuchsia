// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::FromArgs;
use gn_graph::{Graph, Target};

/// display a list of found targets
#[derive(Debug, FromArgs)]
#[argh(subcommand, name = "list")]
pub struct ListArgs {}

impl ListArgs {
    /// Display the label of the target.
    pub fn perform(&self, target: &Target, _graph: &Graph) -> Result<(), anyhow::Error> {
        println!("{}", target.label);
        Ok(())
    }
}
