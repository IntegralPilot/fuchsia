// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub use ffx_package_archive_extract_args::PackageArchiveExtractCommand;
use ffx_writer::VerifiedMachineWriter;
use fho::{FfxContext, FfxMain, FfxTool};
use package_tool::cmd_package_archive_extract;

#[derive(FfxTool)]
#[target(None)]
pub struct ArchiveExtractTool {
    #[command]
    pub cmd: PackageArchiveExtractCommand,
}

fho::embedded_plugin!(ArchiveExtractTool);

#[async_trait::async_trait(?Send)]
impl FfxMain for ArchiveExtractTool {
    type Writer = VerifiedMachineWriter<()>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: <Self as fho::FfxMain>::Writer) -> fho::Result<()> {
        cmd_package_archive_extract(self.cmd)
            .await
            .with_user_message(|| "failed to extract archive")?;
        writer.machine(&()).bug()?;
        Ok(())
    }
}
