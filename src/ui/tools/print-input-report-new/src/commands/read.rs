// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use argh::FromArgs;

#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "read")]
/// Read input reports.
pub struct ReadArgs {}

pub async fn run(_args: ReadArgs) -> Result<()> {
    println!("read subcommand called");
    Ok(())
}
