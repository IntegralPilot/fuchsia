// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use discovery::{FastbootTargetState, TargetHandle, TargetState};
use errors::{ffx_bail, ffx_error};
use fdomain_fuchsia_hardware_power_statecontrol::{
    AdminProxy, ShutdownAction, ShutdownOptions, ShutdownReason,
};
use ffx_config::EnvironmentContext;
use ffx_reboot_args::RebootCommand;
use ffx_writer::MachineWriter;
use fho::{Deferred, FfxContext, FfxMain, FfxTool};
use fidl_fuchsia_developer_ffx::TargetRebootState;
use target_holders::moniker;
use tokio::sync::mpsc::channel;

#[derive(FfxTool)]
pub struct RebootTool {
    context: EnvironmentContext,
    #[command]
    cmd: RebootCommand,
    // Admin proxy for shutdown shim
    #[with(fho::deferred(moniker("/bootstrap/shutdown_shim")))]
    admin_proxy: Deferred<AdminProxy>,
}

fho::embedded_plugin!(RebootTool);

#[async_trait(?Send)]
impl FfxMain for RebootTool {
    type Writer = MachineWriter<()>;

    type Error = ::fho::Error;

    async fn main(mut self, mut writer: Self::Writer) -> fho::Result<()> {
        let res = reboot_direct(&mut self.admin_proxy, self.cmd, &self.context).await;
        if res.is_ok() {
            writer.machine(&())?;
        }
        res
    }
}

async fn reboot_direct(
    admin_proxy: &mut Deferred<AdminProxy>,
    cmd: RebootCommand,
    context: &EnvironmentContext,
) -> Result<(), fho::Error> {
    let state = reboot_state(&cmd)?;
    let reason = cmd.reason.unwrap_or(ShutdownReason::DeveloperRequest);
    // Discover the device, because we may need to reach it directly if it's in fastboot mode
    let handle = ffx_target::discover_single_default_target(context)
        .await
        .map_err(|e| e.into_command_error())?;
    reboot_direct_with_handle(handle, admin_proxy, state, reason, context).await
}

async fn reboot_direct_with_handle(
    handle: TargetHandle,
    admin_proxy: &mut Deferred<AdminProxy>,
    state: TargetRebootState,
    reason: ShutdownReason,
    context: &EnvironmentContext,
) -> Result<(), fho::Error> {
    match handle.state {
        TargetState::Product { .. } => {
            reboot_direct_from_product(admin_proxy.await?, state, reason).await
        }
        TargetState::Fastboot(fastboot_state) => {
            reboot_direct_from_fastboot(handle.node_name, fastboot_state, context, state).await
        }
        s => ffx_bail!("Rebooting a target in state {s} is not supported in direct mode"),
    }
}

async fn reboot_direct_from_product(
    admin_proxy: AdminProxy,
    state: TargetRebootState,
    reason: ShutdownReason,
) -> Result<(), fho::Error> {
    let action = match state {
        TargetRebootState::Product => ShutdownAction::Reboot,
        TargetRebootState::Bootloader => ShutdownAction::RebootToBootloader,
        TargetRebootState::Recovery => ShutdownAction::RebootToRecovery,
    };
    let options =
        ShutdownOptions { action: Some(action), reasons: Some(vec![reason]), ..Default::default() };
    // There are two errors: the outer error, which represents a FIDL failure, and the inner error
    // which is the Shutdown() failure.  The daemon version ignores the shutdown failure, so so will
    // we.
    let _res = match admin_proxy.shutdown(&options).await {
        e @ Err(fidl::Error::ClientChannelClosed { protocol_name, .. }) => {
            // If the 'protocol_name' is 'fuchsia.hardware.power.statecontrol.Admin'
            // then we can be more confident that target reboot/shutdown has succeeded.
            if protocol_name == "fuchsia.hardware.power.statecontrol.Admin" {
                log::info!("Target reboot succeeded.");
            } else {
                log::info!(
                    "Assuming target reboot succeeded. Client received a PEER_CLOSED from '{protocol_name}'"
                );
            }
            log::debug!("{e:?}");
            return Ok(());
        }
        e @ Err(fidl::Error::ClientRead(_)) => {
            // If it is a client read error then we errored out reading
            // the response from the target. This happens when the reboot is
            // successful (the transport shut down).
            log::info!("Target reboot succeeded.");
            log::debug!("{e:?}");
            return Ok(());
        }
        Err(e) => return Err(e).bug_context("Shutting down target"),
        Ok(res) => res,
    };
    Ok(())
}

async fn reboot_direct_from_fastboot(
    node_name: Option<String>,
    fastboot_state: FastbootTargetState,
    context: &EnvironmentContext,
    target_state: TargetRebootState,
) -> Result<(), fho::Error> {
    // TODO(473553526): refactor this and the equivalent code in
    // daemon/protocols/target_collection/src/reboot.rs
    let mut fastboot_interface = ffx_fastboot_connection_factory::get_fastboot_interface(
        &fastboot_state,
        node_name,
        context,
        ffx_fastboot_connection_factory::RetryLimit::Default,
    )
    .await
    .map_err(|e| ffx_error!("Cannot get fastboot interface: {:?}", e))?;
    match target_state {
        TargetRebootState::Product => {
            fastboot_interface.reboot().await.map_err(|e| ffx_error!("Cannot reboot: {e:?}"))?
        }
        TargetRebootState::Bootloader => {
            let (reboot_client, mut reboot_server) = channel(1);
            let reboot_fut = fastboot_interface.reboot_bootloader(reboot_client);
            let drain_fut = async { while reboot_server.recv().await.is_some() {} };
            let (res, _) = futures::join!(reboot_fut, drain_fut);
            res.map_err(|e| ffx_error!("Cannot reboot to bootloader: {e:?}"))?
        }
        TargetRebootState::Recovery => ffx_bail!("Cannot reboot from fastboot to recovery"),
    }
    Ok(())
}

fn reboot_state(cmd: &RebootCommand) -> fho::Result<TargetRebootState> {
    match (cmd.bootloader, cmd.recovery) {
        (true, true) => {
            ffx_bail!("Cannot specify booth bootloader and recovery switches at the same time.")
        }
        (true, false) => Ok(TargetRebootState::Bootloader),
        (false, true) => Ok(TargetRebootState::Recovery),
        (false, false) => Ok(TargetRebootState::Product),
    }
}

////////////////////////////////////////////////////////////////////////////////
// tests
#[cfg(test)]
mod test {
    use super::*;
    use argh::FromArgs as _;
    use fdomain_fuchsia_hardware_power_statecontrol::AdminRequest;

    #[fuchsia::test]
    async fn test_reboot_direct_from_product() -> fho::Result<()> {
        let client = fdomain_local::local_client_empty();
        let admin_proxy = target_holders::fake_proxy(client, |req| match req {
            AdminRequest::Shutdown { options, responder } => {
                assert_eq!(options.action, Some(ShutdownAction::Reboot));
                assert_eq!(options.reasons, Some(vec![ShutdownReason::DeveloperRequest]));
                responder.send(Ok(())).unwrap();
            }
            r => panic!("unexpected request: {:?}", r),
        });
        reboot_direct_from_product(
            admin_proxy,
            TargetRebootState::Product,
            ShutdownReason::DeveloperRequest,
        )
        .await
    }

    #[fuchsia::test]
    async fn test_reboot_direct_from_product_with_reason() -> fho::Result<()> {
        let client = fdomain_local::local_client_empty();
        let admin_proxy = target_holders::fake_proxy(client, |req| match req {
            AdminRequest::Shutdown { options, responder } => {
                assert_eq!(options.action, Some(ShutdownAction::Reboot));
                assert_eq!(options.reasons, Some(vec![ShutdownReason::SystemUpdate]));
                responder.send(Ok(())).unwrap();
            }
            r => panic!("unexpected request: {:?}", r),
        });
        reboot_direct_from_product(
            admin_proxy,
            TargetRebootState::Product,
            ShutdownReason::SystemUpdate,
        )
        .await
    }

    #[fuchsia::test]
    async fn test_reboot_direct_from_product_bootloader() -> fho::Result<()> {
        let client = fdomain_local::local_client_empty();
        let admin_proxy = target_holders::fake_proxy(client, |req| match req {
            AdminRequest::Shutdown { options, responder } => {
                assert_eq!(options.action, Some(ShutdownAction::RebootToBootloader));
                assert_eq!(options.reasons, Some(vec![ShutdownReason::DeveloperRequest]));
                responder.send(Ok(())).unwrap();
            }
            r => panic!("unexpected request: {:?}", r),
        });
        reboot_direct_from_product(
            admin_proxy,
            TargetRebootState::Bootloader,
            ShutdownReason::DeveloperRequest,
        )
        .await
    }

    #[fuchsia::test]
    async fn test_reboot_direct_from_product_recovery() -> fho::Result<()> {
        let client = fdomain_local::local_client_empty();
        let admin_proxy = target_holders::fake_proxy(client, |req| match req {
            AdminRequest::Shutdown { options, responder } => {
                assert_eq!(options.action, Some(ShutdownAction::RebootToRecovery));
                assert_eq!(options.reasons, Some(vec![ShutdownReason::DeveloperRequest]));
                responder.send(Ok(())).unwrap();
            }
            r => panic!("unexpected request: {:?}", r),
        });
        reboot_direct_from_product(
            admin_proxy,
            TargetRebootState::Recovery,
            ShutdownReason::DeveloperRequest,
        )
        .await
    }

    #[fuchsia::test]
    async fn test_reboot_direct_zedboot_error() {
        let env = ffx_config::test_init().unwrap();
        let handle = TargetHandle {
            node_name: Some("foo".to_string()),
            state: TargetState::Zedboot,
            manual: false,
        };
        // We can pass a dummy admin proxy since it won't be used
        let client = fdomain_local::local_client_empty();
        let mut admin_proxy =
            Deferred::from_output(Ok(target_holders::fake_proxy(client, |_req| {
                panic!("unexpected request")
            })));

        let res = reboot_direct_with_handle(
            handle,
            &mut admin_proxy,
            TargetRebootState::Product,
            ShutdownReason::DeveloperRequest,
            &env.context,
        )
        .await;

        assert!(res.is_err());
        assert!(
            res.unwrap_err()
                .to_string()
                .contains("Rebooting a target in state Zedboot is not supported")
        );
    }

    #[test]
    fn test_reboot_command_reason_parsing() {
        let cmd = RebootCommand::from_args(&["reboot"], &["--reason", "system update"]).unwrap();
        assert_eq!(cmd.reason, Some(ShutdownReason::SystemUpdate));

        let cmd = RebootCommand::from_args(&["reboot"], &["--reason", "system-update"]).unwrap();
        assert_eq!(cmd.reason, Some(ShutdownReason::SystemUpdate));

        let cmd =
            RebootCommand::from_args(&["reboot"], &["--reason", "DEVELOPER_REQUEST"]).unwrap();
        assert_eq!(cmd.reason, Some(ShutdownReason::DeveloperRequest));

        let cmd = RebootCommand::from_args(&["reboot"], &["--reason", "user-request"]).unwrap();
        assert_eq!(cmd.reason, Some(ShutdownReason::UserRequest));

        let cmd = RebootCommand::from_args(&["reboot"], &[]).unwrap();
        assert_eq!(cmd.reason, None);

        // Disallowed reasons or invalid strings should fail parsing.
        let err = RebootCommand::from_args(&["reboot"], &["--reason", "oom"]);
        assert!(err.is_err());

        let err = RebootCommand::from_args(&["reboot"], &["--reason", "high-temperature"]);
        assert!(err.is_err());

        let err = RebootCommand::from_args(&["reboot"], &["--reason", "invalid_reason"]);
        assert!(err.is_err());
    }
}
