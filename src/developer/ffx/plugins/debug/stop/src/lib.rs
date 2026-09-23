// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use async_trait::async_trait;
use ffx_debug_stop_args::StopCommand;
use fho::{FfxMain, FfxTool, SimpleWriter, moniker};
use fidl_fuchsia_debugger::DebugAgentProxy;
use zx_status::Status;

#[derive(FfxTool)]
pub struct StopTool {
    #[command]
    cmd: StopCommand,

    #[with(moniker("/core/debugger"))]
    debugger_proxy: fidl_fuchsia_debugger::DebugAgentProxy,
}

fho::embedded_plugin!(StopTool);

#[async_trait(?Send)]
impl FfxMain for StopTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    async fn main(self, mut _writer: Self::Writer) -> fho::Result<()> {
        stop_tool_impl(self.cmd, self.debugger_proxy).await?;
        Ok(())
    }
}

async fn stop_tool_impl(_cmd: StopCommand, debugger_proxy: DebugAgentProxy) -> Result<()> {
    debugger_proxy.shutdown().await?.map_err(Status::err_from_raw)?;
    Ok(())
}
