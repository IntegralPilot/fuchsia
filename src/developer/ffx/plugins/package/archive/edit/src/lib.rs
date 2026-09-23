// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub use ffx_package_archive_edit_args::PackageArchiveEditCommand;
use ffx_writer::VerifiedMachineWriter;
use fho::{FfxContext, FfxMain, FfxTool};
use package_tool::cmd_package_archive_edit;

#[derive(FfxTool)]
#[target(None)]
pub struct ArchiveEditTool {
    #[command]
    pub cmd: PackageArchiveEditCommand,
}

fho::embedded_plugin!(ArchiveEditTool);

#[async_trait::async_trait(?Send)]
impl FfxMain for ArchiveEditTool {
    type Writer = VerifiedMachineWriter<()>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: <Self as fho::FfxMain>::Writer) -> fho::Result<()> {
        cmd_package_archive_edit(self.cmd).await.with_user_message(|| "failed to edit archive")?;
        writer.machine(&()).bug()?;
        Ok(())
    }
}
