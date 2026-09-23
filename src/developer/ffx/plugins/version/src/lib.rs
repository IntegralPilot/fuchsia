// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use chrono::Local;
use ffx_config::EnvironmentContext;
use ffx_version_args::VersionCommand;
use ffx_writer::{MachineWriter, ToolIO};
use fho::{FfxMain, FfxTool, Result};

mod serialization;

use serialization::*;

#[derive(FfxTool)]
pub struct VersionTool {
    #[command]
    cmd: VersionCommand,
    context: EnvironmentContext,
}

fho::embedded_plugin!(VersionTool);

#[async_trait(?Send)]
impl FfxMain for VersionTool {
    type Writer = MachineWriter<Versions>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> Result<()> {
        let tool_version = self.context.build_info().into();
        let versions = Versions { tool_version, daemon_version: None };

        if writer.is_machine() {
            Ok(writer.machine(&versions)?)
        } else {
            format_versions(&versions, self.cmd.verbose, &mut writer, Local)
        }
    }
}
