// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::commands::types::*;
use crate::types::Error;
use argh::{ArgsInfo, FromArgs};
use serde::Serialize;
use std::fmt;

/// Lists all available ArchiveAccessor in the system and their selector for use in "accessor"
/// arguments in other sub-commands.
#[derive(Default, ArgsInfo, FromArgs, PartialEq, Debug)]
#[argh(subcommand, name = "list-accessors")]
pub struct ListAccessorsCommand {}

impl Command for ListAccessorsCommand {
    type Result = ListAccessorsResult;

    async fn execute<P: DiagnosticsProvider>(self, provider: &P) -> Result<Self::Result, Error> {
        let paths = provider.get_accessor_paths().await?;
        Ok(ListAccessorsResult(paths))
    }
}

#[derive(Serialize)]
pub struct ListAccessorsResult(Vec<String>);

impl fmt::Display for ListAccessorsResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for item in self.0.iter() {
            writeln!(f, "{item}")?;
        }
        Ok(())
    }
}
