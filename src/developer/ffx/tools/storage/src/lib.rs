// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use argh::{ArgsInfo, FromArgs};
use async_trait::async_trait;
use ffx_writer::SimpleWriter;
use fho::{FfxMain, FfxTool, Result, deferred};
use target_holders::moniker;

mod fxfs;

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand)]
pub enum StorageSubCommand {
    Fxfs(fxfs::FxfsCommand),
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "storage", description = "Manage Fuchsia Filesystems.")]
pub struct StorageCommand {
    #[argh(subcommand)]
    subcommand: StorageSubCommand,
}

#[derive(FfxTool)]
pub struct StorageTool {
    #[command]
    cmd: StorageCommand,

    #[with(deferred(moniker("/bootstrap/fshost/fxfs")))]
    fxfs_proxy: fho::Deferred<fdomain_fuchsia_fxfs::DebugProxy>,
}

#[async_trait(?Send)]
impl FfxMain for StorageTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    async fn main(self, writer: Self::Writer) -> Result<()> {
        match self.cmd.subcommand {
            StorageSubCommand::Fxfs(cmd) => {
                fxfs::handle_cmd(cmd, writer, self.fxfs_proxy.await?).await
            }
        }
    }
}
