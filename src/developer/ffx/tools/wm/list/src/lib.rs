// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Result, format_err};
use async_trait::async_trait;
use fdomain_fuchsia_session_window::ManagerProxy;
use ffx_wm_list_args::WMListCommand;
use ffx_writer::SimpleWriter;
use fho::{FfxMain, FfxTool};
use target_holders::moniker;

#[derive(FfxTool)]
pub struct ListTool {
    #[command]
    cmd: WMListCommand,
    #[with(moniker("core/session-manager"))]
    manager_proxy: ManagerProxy,
}

fho::embedded_plugin!(ListTool);

#[async_trait(?Send)]
impl FfxMain for ListTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        list_impl(self.manager_proxy, self.cmd, &mut writer).await?;
        Ok(())
    }
}

pub async fn list_impl<W: std::io::Write>(
    manager_proxy: ManagerProxy,
    _cmd: WMListCommand,
    writer: &mut W,
) -> Result<()> {
    writeln!(writer, "Windows in the current session:")?;
    match manager_proxy.list().await {
        Ok(views) => {
            for view in views {
                let pos = view.position;
                let id = view.id;
                writeln!(writer, "{pos}: {id}")?;
            }
            Ok(())
        }
        Err(e) => Err(format_err!("{:?}", e)),
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use fdomain_fuchsia_session_window::{ListedView, ManagerRequest};
    use target_holders::fake_proxy;

    #[fuchsia::test]
    async fn test_list() {
        let listed_views = vec![
            ListedView { position: 0, id: "view0".to_string() },
            ListedView { position: 1, id: "view1".to_string() },
            ListedView { position: 2, id: "view2".to_string() },
            ListedView { position: 3, id: "view3".to_string() },
        ];
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, move |req| match req {
            ManagerRequest::Cycle { .. } => unreachable!(),
            ManagerRequest::Focus { .. } => unreachable!(),
            ManagerRequest::List { responder } => {
                let _ = responder.send(&listed_views);
            }
            ManagerRequest::SetOrder { .. } => unreachable!(),
            _ => unreachable!(),
        });
        let list_cmd = WMListCommand {};
        let mut writer = Vec::new();
        let result = list_impl(proxy, list_cmd, &mut writer).await;
        assert!(result.is_ok());
        let output = String::from_utf8(writer).unwrap();
        assert_eq!(
            output,
            "Windows in the current session:\n0: view0\n1: view1\n2: view2\n3: view3\n"
        );
    }
}
