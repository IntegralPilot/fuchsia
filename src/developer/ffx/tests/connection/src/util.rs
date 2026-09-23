// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Result, anyhow};
use async_lock::Mutex;
use fdomain_client::fidl::{DiscoverableProtocolMarker, Proxy};
use fdomain_fuchsia_developer_remotecontrol::RemoteControlProxy;
use fdomain_fuchsia_sys2 as fsys;
use fdomain_test_proxy_stress::{StressorMarker, StressorProxy};
use fuchsia_async as fasync;
use std::sync::Arc;
const STRESSOR_URL: &str =
    "fuchsia-pkg://fuchsia.com/ffx_connection_test_components#meta/proxy_stressor.cm";

/// A reference to a launched component on the target device. Used to tear down the component
/// when the test completes.
struct LaunchedComponent {
    moniker: String,
    target_identifier: String,
}

/// Helper for creating proxies to a launched component on the target device.
pub struct LaunchedComponentConnector {
    target_addr: String,
    moniker: String,
    rcs_proxy: RemoteControlProxy,
    connections: Mutex<Vec<Arc<ffx_target::Connection>>>,
    env_context: ffx_config::EnvironmentContext,
}

impl LaunchedComponent {
    async fn destroy(self, ffx: &ffx_isolate::Isolate) -> Result<()> {
        ffx.ffx(&["-t", &self.target_identifier, "component", "destroy", &self.moniker]).await?;
        Ok(())
    }
}

impl LaunchedComponentConnector {
    async fn connect_with_rcs_proxy(
        rcs_proxy: &RemoteControlProxy,
        moniker: &str,
    ) -> Result<StressorProxy> {
        loop {
            let client = rcs_proxy.domain();
            let (proxy, server) = client.create_proxy::<StressorMarker>();
            if let Ok(Ok(_)) = rcs_proxy
                .connect_capability(
                    &moniker,
                    fsys::OpenDirType::ExposedDir,
                    StressorMarker::PROTOCOL_NAME,
                    server.into_channel(),
                )
                .await
            {
                return Ok(proxy);
            }
        }
    }

    /// Creates a new connection to the component running on the target.
    pub async fn connect(&self) -> Result<StressorProxy> {
        Self::connect_with_rcs_proxy(&self.rcs_proxy, &self.moniker).await
    }

    /// Creates a new connection to the target and uses it to connect to the component
    /// running on target.
    pub async fn connect_via_new_target_connection(&self) -> Result<StressorProxy> {
        let (rcs_proxy, conn) = connect_to_rcs(&self.target_addr, &self.env_context).await?;
        self.connections.lock().await.push(conn);
        Self::connect_with_rcs_proxy(&rcs_proxy, &self.moniker).await
    }
}

/// Launch an instance of the stressor component on target.
async fn launch(
    name: &str,
    target_identifier: &str,
    target_addr: &str,
    isolate: &ffx_isolate::Isolate,
) -> Result<(LaunchedComponent, LaunchedComponentConnector)> {
    let moniker = format!("/core/ffx-laboratory:{}", name);

    // Best-effort cleanup: destroy any stale instance left over from a previous aborted run.
    // Log errors as info for debugging purposes.
    if let Err(e) = isolate.ffx(&["-t", target_identifier, "component", "destroy", &moniker]).await
    {
        log::info!("Stale component cleanup failed (this is normal if it didn't exist): {:?}", e);
    }

    let env_context = isolate.env_context().clone();
    let create_output = isolate
        .ffx(&["-t", target_identifier, "component", "create", &moniker, STRESSOR_URL])
        .await?;
    if !create_output.status.success() {
        return Err(anyhow!("Failed to create component: {:?}", create_output));
    }

    let component = LaunchedComponent {
        moniker: moniker.clone(),
        target_identifier: target_identifier.to_string(),
    };

    let start_and_launch_result = async move {
        let output =
            isolate.ffx(&["-t", target_identifier, "component", "start", &moniker]).await?;
        if !output.status.success() {
            Err(anyhow!("Failed to start component: {:?}", output))
        } else {
            let (rcs_proxy, conn) = connect_to_rcs(target_addr, &env_context).await?;
            Ok(LaunchedComponentConnector {
                target_addr: target_addr.to_string(),
                moniker,
                rcs_proxy,
                connections: Mutex::new(vec![conn]),
                env_context,
            })
        }
    }
    .await;

    match start_and_launch_result {
        Ok(component_connector) => Ok((component, component_connector)),
        Err(e) => {
            // In case resolve or start fails, destroy the component to cleanup resources.
            let _ = component.destroy(isolate).await;
            Err(e)
        }
    }
}

/// Connects directly to RCS on the target using target resolution.
async fn connect_to_rcs(
    target_addr: &str,
    context: &ffx_config::EnvironmentContext,
) -> Result<(RemoteControlProxy, Arc<ffx_target::Connection>)> {
    let query = ffx_target::TargetInfoQuery::try_from(target_addr.to_string())?;
    let resolution = ffx_target::resolve_target_address(&query, false, context).await?;
    let conn = resolution.get_connection(context).await?;
    let rcs_proxy = conn.rcs_proxy_fdomain().await?;
    Ok((rcs_proxy, conn))
}

/// Test fixture that handles launching and tearing down a test after execution.
pub async fn setup_and_teardown_fixture<F, Fut>(case_name: &str, test_fn: F)
where
    F: FnOnce(LaunchedComponentConnector) -> Fut + Send + 'static,
    Fut: futures::future::Future<Output = ()>,
{
    let ssh_path = std::env::var("FUCHSIA_SSH_KEY").unwrap().into();
    let test_env = ffx_config::test_init().expect("Setting up test environment");
    let isolate = ffx_isolate::Isolate::new_in_test(case_name, ssh_path, &test_env.context)
        .await
        .expect("create isolate");

    // Ensure that the address is formatted properly, and include port if it is available.
    // Without this formatting, the connection does not work when using a remote workflow.
    let raw_addr = std::env::var("FUCHSIA_DEVICE_ADDR").unwrap();
    let base_addr = raw_addr.trim_start_matches('[').trim_end_matches(']');
    let port = std::env::var("FUCHSIA_SSH_PORT").ok();
    let addr = if base_addr.contains(':') {
        format!("[{}]{}", base_addr, port.map(|v| format!(":{}", v)).unwrap_or_default())
    } else {
        format!("{}{}", base_addr, port.map(|v| format!(":{}", v)).unwrap_or_default())
    };
    let nodename = std::env::var("FUCHSIA_NODENAME").unwrap();

    isolate.ffx(&["target", "add", &addr]).await.expect("add target");
    let target_identifier = if nodename.is_empty() { &addr } else { &nodename };
    isolate.ffx(&["-t", target_identifier, "target", "wait"]).await.expect("wait for target");

    let (launched_component, component_connector) =
        launch(case_name, target_identifier, &addr, &isolate).await.expect("launch component");

    // Spawn a new thread so that we can catch panics from the test. We check completion of
    // the thread using an mpsc channel, so that futures on the original executor continue
    // to be polled while the test runs in a different thread (as opposed to joining using
    // join, which is blocking and prevents any other futures from polling).
    let (done_sender, done) = futures::channel::oneshot::channel();
    let join_handle = std::thread::spawn(move || {
        let mut test_executor = fasync::LocalExecutor::default();
        test_executor.run_singlethreaded(test_fn(component_connector));
        let _ = done_sender.send(());
    });
    let _ = done.await;
    // after the receiver completes we know the test is done, so we can do a blocking join
    // without issue.
    let test_result = join_handle.join();

    let destroy_result = launched_component.destroy(&isolate).await;

    // Test error is a dyn Any. The only way we can display it is by propagating the panic.
    match (test_result, destroy_result) {
        (Ok(()), Ok(())) => (),
        (Err(test_err), Ok(())) => std::panic::resume_unwind(test_err),
        (Ok(()), Err(destroy_err)) => panic!("{}", destroy_err),
        (Err(test_err), Err(destroy_err)) => {
            log::error!("Destroy failed: {}", destroy_err);
            std::panic::resume_unwind(test_err);
        }
    }
}
