// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub use ffx_package_archive_remove_args::PackageArchiveRemoveCommand;
use ffx_writer::VerifiedMachineWriter;
use fho::{FfxContext, FfxMain, FfxTool};
use package_tool::cmd_package_archive_remove;

#[derive(FfxTool)]
#[target(None)]
pub struct ArchiveRemoveTool {
    #[command]
    pub cmd: PackageArchiveRemoveCommand,
}

fho::embedded_plugin!(ArchiveRemoveTool);

#[async_trait::async_trait(?Send)]
impl FfxMain for ArchiveRemoveTool {
    type Writer = VerifiedMachineWriter<()>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: <Self as fho::FfxMain>::Writer) -> fho::Result<()> {
        cmd_package_archive_remove(self.cmd)
            .await
            .with_user_message(|| "failed to remove from archive")?;
        writer.machine(&()).bug()?;
        Ok(())
    }
}
