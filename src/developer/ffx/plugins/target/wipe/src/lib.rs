// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use errors::ffx_bail;
use fdomain_fuchsia_recovery::FactoryResetProxy;
use ffx_wipe_args::WipeCommand;
use ffx_writer::MachineWriter;
use fho::{Deferred, FfxContext, FfxMain, FfxTool};
use std::io::{Write, stdin};
use target_holders::moniker;
use zx_status::Status;

#[derive(FfxTool)]
pub struct WipeTool {
    #[command]
    cmd: WipeCommand,
    #[with(fho::deferred(moniker("/core/factory_reset")))]
    factory_reset_proxy: Deferred<FactoryResetProxy>,
}

fho::embedded_plugin!(WipeTool);

#[async_trait(?Send)]
impl FfxMain for WipeTool {
    type Writer = MachineWriter<()>;

    type Error = ::fho::Error;

    async fn main(mut self, mut writer: Self::Writer) -> fho::Result<()> {
        if !self.cmd.force {
            writeln!(
                writer,
                "{}",
                "WARNING: This will erase all user data on the target. Are you sure? [yn]"
            )
            .bug_context("failed to write")?;
            let answer = blocking::unblock(|| {
                use std::io::BufRead;
                let mut line = String::new();
                let stdin = stdin();
                let mut locked = stdin.lock();
                let _ = locked.read_line(&mut line);
                line
            })
            .await;
            if answer.trim() != "y" {
                ffx_bail!("User aborted");
            }
        }
        let factory_reset = self.factory_reset_proxy.await?;
        wipe_target(factory_reset).await?;
        writer.machine(&())?;
        Ok(())
    }
}

async fn wipe_target(factory_reset: FactoryResetProxy) -> fho::Result<()> {
    match factory_reset.reset().await.map(Status::ok) {
        Ok(Err(status)) => {
            ffx_bail!("Factory reset failed with status: {}", status);
        }
        Ok(Ok(())) => {
            log::info!("Factory reset succeeded.");
        }
        Err(fidl::Error::ClientChannelClosed { protocol_name, .. }) => {
            if protocol_name == "fuchsia.recovery.FactoryReset" {
                log::info!("Factory reset succeeded.");
            } else {
                log::info!(
                    "Assuming factory reset succeeded. Client received a PEER_CLOSED from '{protocol_name}'"
                );
            }
            return Ok(());
        }
        Err(fidl::Error::ClientRead(_)) => {
            log::info!("Factory reset succeeded.");
            return Ok(());
        }
        Err(e) => {
            ffx_bail!("FIDL error calling FactoryReset: {:?}", e);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fdomain_fuchsia_recovery::FactoryResetRequest;
    use target_holders::fake_proxy;

    #[fuchsia::test]
    async fn test_wipe_target() -> fho::Result<()> {
        let client = fdomain_local::local_client_empty();
        let factory_reset = fake_proxy(client, |req| match req {
            FactoryResetRequest::Reset { responder } => {
                responder.send(zx_status::sys::ZX_OK).unwrap();
            }
        });
        wipe_target(factory_reset).await
    }

    #[fuchsia::test]
    async fn test_wipe_target_error() {
        let client = fdomain_local::local_client_empty();
        let factory_reset = fake_proxy(client, |req| match req {
            FactoryResetRequest::Reset { responder } => {
                responder.send(Status::INTERNAL.into_raw()).unwrap();
            }
        });
        assert!(wipe_target(factory_reset).await.is_err());
    }
}
