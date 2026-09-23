// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use fdomain_fuchsia_hardware_power_statecontrol::{
    AdminProxy, ShutdownAction, ShutdownOptions, ShutdownReason,
};
use ffx_off_args::OffCommand;
use ffx_writer::SimpleWriter;
use fho::{FfxContext, FfxMain, FfxTool};
use target_holders::moniker;

#[derive(FfxTool)]
pub struct OffTool {
    #[command]
    cmd: OffCommand,
    #[with(moniker("/bootstrap/shutdown_shim"))]
    admin_proxy: AdminProxy,
}

fho::embedded_plugin!(OffTool);

#[async_trait(?Send)]
impl FfxMain for OffTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    async fn main(self, _writer: Self::Writer) -> fho::Result<()> {
        off(self.admin_proxy, self.cmd).await
    }
}

async fn off(admin_proxy: AdminProxy, _cmd: OffCommand) -> fho::Result<()> {
    let res = admin_proxy
        .shutdown(&ShutdownOptions {
            action: Some(ShutdownAction::Poweroff),
            reasons: Some(vec![ShutdownReason::DeveloperRequest]),
            ..Default::default()
        })
        .await;
    match res {
        Ok(_) => Ok(()),
        Err(ref e) => match e {
            fidl::Error::ClientChannelClosed { epitaph: fidl::Epitaph::PeerClosed, .. } => Ok(()),
            _ => res
                .bug()?
                .map_err(fidl::Status::err_from_raw)
                .user_message("Unexpected error from poweroff"),
        },
    }
}

////////////////////////////////////////////////////////////////////////////////
// tests

#[cfg(test)]
mod test {
    use super::*;
    use fdomain_fuchsia_hardware_power_statecontrol::AdminRequest;
    use target_holders::fake_proxy;

    fn setup_fake_admin_server() -> AdminProxy {
        let client = fdomain_local::local_client_empty();
        fake_proxy(client, |req| match req {
            AdminRequest::Shutdown { options, responder } => {
                assert_eq!(options.action, Some(ShutdownAction::Poweroff));
                assert_eq!(options.reasons, Some(vec![ShutdownReason::DeveloperRequest]));
                responder.send(Ok(())).unwrap();
            }
            _ => assert!(false),
        })
    }

    #[fuchsia::test]
    async fn test_off() {
        let admin_proxy = setup_fake_admin_server();
        let result = off(admin_proxy, OffCommand {}).await;
        assert!(result.is_ok());
    }
}
