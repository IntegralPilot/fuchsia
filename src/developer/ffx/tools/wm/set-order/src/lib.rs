// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Result, format_err};
use async_trait::async_trait;
use fdomain_fuchsia_session_window::ManagerProxy;
use ffx_wm_setorder_args::WMSetOrderCommand;
use ffx_writer::SimpleWriter;
use fho::{FfxMain, FfxTool};
use target_holders::moniker;

#[derive(FfxTool)]
pub struct SetOrderTool {
    #[command]
    cmd: WMSetOrderCommand,
    #[with(moniker("core/session-manager"))]
    manager_proxy: ManagerProxy,
}

fho::embedded_plugin!(SetOrderTool);

#[async_trait(?Send)]
impl FfxMain for SetOrderTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        set_order_impl(self.manager_proxy, self.cmd, &mut writer).await?;
        Ok(())
    }
}

pub async fn set_order_impl<W: std::io::Write>(
    manager_proxy: ManagerProxy,
    cmd: WMSetOrderCommand,
    writer: &mut W,
) -> Result<()> {
    writeln!(
        writer,
        "Move window at position {} to position {} in the current session",
        cmd.old_position, cmd.new_position
    )?;
    manager_proxy
        .set_order(cmd.old_position, cmd.new_position)
        .await
        .map_err(|err| format_err!("{:?}", err))
}

#[cfg(test)]
mod test {
    use super::*;
    use fdomain_fuchsia_session_window::ManagerRequest;
    use target_holders::fake_proxy;

    #[fuchsia::test]
    async fn test_set_order() {
        const OLD_POSITION: u64 = 0;
        const NEW_POSITION: u64 = 1;

        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, |req| match req {
            ManagerRequest::Cycle { .. } => unreachable!(),
            ManagerRequest::Focus { .. } => unreachable!(),
            ManagerRequest::List { .. } => unreachable!(),
            ManagerRequest::SetOrder { old_position, new_position, responder, .. } => {
                assert_eq!(old_position, OLD_POSITION);
                assert_eq!(new_position, NEW_POSITION);
                let _ = responder.send();
            }
            _ => unreachable!(),
        });

        let set_order_cmd =
            WMSetOrderCommand { old_position: OLD_POSITION, new_position: NEW_POSITION };
        let mut writer = Vec::new();
        let result = set_order_impl(proxy, set_order_cmd, &mut writer).await;
        assert!(result.is_ok());
        let output = String::from_utf8(writer).unwrap();
        assert_eq!(
            output,
            format!(
                "Move window at position {} to position {} in the current session\n",
                OLD_POSITION, NEW_POSITION
            )
        );
    }
}
