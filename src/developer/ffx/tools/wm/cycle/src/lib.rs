// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Result, format_err};
use async_trait::async_trait;
use fdomain_fuchsia_session_window::ManagerProxy;
use ffx_wm_cycle_args::WMCycleCommand;
use ffx_writer::SimpleWriter;
use fho::{FfxMain, FfxTool};
use target_holders::moniker;

#[derive(FfxTool)]
pub struct CycleTool {
    #[command]
    cmd: WMCycleCommand,
    #[with(moniker("core/session-manager"))]
    manager_proxy: ManagerProxy,
}

fho::embedded_plugin!(CycleTool);

#[async_trait(?Send)]
impl FfxMain for CycleTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        cycle_impl(self.manager_proxy, self.cmd, &mut writer).await?;
        Ok(())
    }
}

pub async fn cycle_impl<W: std::io::Write>(
    manager_proxy: ManagerProxy,
    _cmd: WMCycleCommand,
    writer: &mut W,
) -> Result<()> {
    writeln!(writer, "Cycle windows in the current session")?;
    manager_proxy.cycle().await.map_err(|err| format_err!("{:?}", err))
}

#[cfg(test)]
mod test {
    use super::*;
    use fdomain_fuchsia_session_window::ManagerRequest;
    use target_holders::fake_proxy;

    #[fuchsia::test]
    async fn test_cycle() {
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, move |req| match req {
            ManagerRequest::Cycle { responder } => {
                let _ = responder.send();
            }
            ManagerRequest::Focus { .. } => unreachable!(),
            ManagerRequest::List { .. } => unreachable!(),
            ManagerRequest::SetOrder { .. } => unreachable!(),
            _ => unreachable!(),
        });

        let cycle_cmd = WMCycleCommand {};
        let mut writer = Vec::new();
        let result = cycle_impl(proxy, cycle_cmd, &mut writer).await;
        assert!(result.is_ok());
        let output = String::from_utf8(writer).unwrap();
        assert_eq!(output, "Cycle windows in the current session\n");
    }
}
