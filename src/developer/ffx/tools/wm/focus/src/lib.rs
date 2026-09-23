// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Result, format_err};
use async_trait::async_trait;
use fdomain_fuchsia_session_window::ManagerProxy;
use ffx_wm_focus_args::WMFocusCommand;
use ffx_writer::SimpleWriter;
use fho::{FfxMain, FfxTool};
use target_holders::moniker;

#[derive(FfxTool)]
pub struct FocusTool {
    #[command]
    cmd: WMFocusCommand,
    #[with(moniker("core/session-manager"))]
    manager_proxy: ManagerProxy,
}

fho::embedded_plugin!(FocusTool);

#[async_trait(?Send)]
impl FfxMain for FocusTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        focus_impl(self.manager_proxy, self.cmd, &mut writer).await?;
        Ok(())
    }
}

pub async fn focus_impl<W: std::io::Write>(
    manager_proxy: ManagerProxy,
    cmd: WMFocusCommand,
    writer: &mut W,
) -> Result<()> {
    writeln!(writer, "Focus on window previously at position: {}", cmd.position)?;
    manager_proxy.focus(cmd.position).await.map_err(|err| format_err!("{:?}", err))
}

#[cfg(test)]
mod test {
    use super::*;
    use fdomain_fuchsia_session_window::ManagerRequest;
    use target_holders::fake_proxy;

    #[fuchsia::test]
    async fn test_focus() {
        const POSITION: u64 = 1;

        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, |req| match req {
            ManagerRequest::Cycle { .. } => unreachable!(),
            ManagerRequest::Focus { position, responder } => {
                assert_eq!(position, POSITION);
                let _ = responder.send();
            }
            ManagerRequest::List { .. } => unreachable!(),
            ManagerRequest::SetOrder { .. } => unreachable!(),
            _ => unreachable!(),
        });
        let focus_cmd = WMFocusCommand { position: POSITION };
        let mut writer = Vec::new();
        let result = focus_impl(proxy, focus_cmd, &mut writer).await;
        assert!(result.is_ok());
        let output = String::from_utf8(writer).unwrap();
        assert_eq!(output, format!("Focus on window previously at position: {}\n", POSITION));
    }
}
