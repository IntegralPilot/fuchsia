// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub use ffx_package_build_args::PackageBuildCommand;
use ffx_writer::VerifiedMachineWriter;
use fho::{FfxContext, FfxMain, FfxTool, Result};
use package_tool::cmd_package_build;

#[derive(FfxTool)]
#[target(None)]
pub struct PackageBuildTool {
    #[command]
    pub cmd: PackageBuildCommand,
}

fho::embedded_plugin!(PackageBuildTool);

#[async_trait::async_trait(?Send)]
impl FfxMain for PackageBuildTool {
    type Writer = VerifiedMachineWriter<()>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: <Self as fho::FfxMain>::Writer) -> fho::Result<()> {
        cmd_package_build(self.cmd).await.with_user_message(|| "failed to build package")?;
        writer.machine(&()).bug()?;
        Ok(())
    }
}
