// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow as _;
use argh::{ArgsInfo, FromArgs};
use async_trait::async_trait;
use ffx_writer::SimpleWriter;
use fho::{Error, FfxMain, FfxTool, Result};

mod generate;

#[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
#[argh(subcommand)]
pub enum AuthSubCommand {
    Generate(generate::GenerateCommand),
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "auth", description = "Generate Fuchsia authorization credentials.")]
pub struct AuthCommand {
    #[argh(subcommand)]
    pub subcommand: AuthSubCommand,
}

#[derive(FfxTool)]
pub struct AuthTool {
    #[command]
    pub cmd: AuthCommand,
}

#[async_trait(?Send)]
impl FfxMain for AuthTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    async fn main(self, writer: Self::Writer) -> Result<()> {
        match &self.cmd.subcommand {
            AuthSubCommand::Generate(command) => {
                generate::generate(&command, writer).await.map_err(|e| Error::User(e.into()))
            }
        }
    }
}
