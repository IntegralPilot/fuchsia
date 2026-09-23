// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::Resolution;
use crate::target_connector::{
    BUFFER_SIZE, FDomainConnection, OvernetConnection, TargetConnection, TargetConnectionError,
    TargetConnector,
};
use anyhow::Result;
use ffx_command_error::FfxContext as _;
use ffx_config::{EnvironmentContext, TryFromEnvContext};
use futures::future::LocalBoxFuture;
use std::fmt::Debug;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::io::BufReader;

const OVERNET_VSOCK_PORT: u32 = 202;
const FDOMAIN_VSOCK_PORT: u32 = 203;

const CONFIG_START_DRIVER: &str = "connectivity.usb_driver_autostart";

pub struct UsbConnector {
    driver: usb_driver_api::Driver,
    cid: u32,
}

impl Debug for UsbConnector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UsbConnector").field("cid", &self.cid).finish()
    }
}

impl UsbConnector {
    pub async fn new(
        cid: u32,
        env_context: &EnvironmentContext,
    ) -> std::result::Result<Self, crate::FfxTargetCrateError> {
        let socket_path: PathBuf = env_context.get(usb_driver_api::CONFIG_USB_SOCKET_PATH)?;

        try_daemon_autostart(&socket_path, env_context);

        let driver = usb_driver_api::Driver::init(socket_path).await?;
        Ok(Self { driver, cid })
    }
}

impl UsbConnector {
    async fn connect_overnet(&mut self) -> Result<OvernetConnection, TargetConnectionError> {
        let conn = self.driver.connect(self.cid, OVERNET_VSOCK_PORT).await.map_err(|e| {
            TargetConnectionError::Fatal(anyhow::anyhow!("Connection error: {e:?}"))
        })?;
        let (output, input) = conn.into_split();
        let output = BufReader::with_capacity(BUFFER_SIZE, output);
        let (_sender, errors) = async_channel::unbounded();
        Ok(OvernetConnection {
            output: Box::new(output),
            input: Box::new(input),
            errors,
            main_task: None,
        })
    }

    async fn connect_fdomain(&mut self) -> Result<FDomainConnection, TargetConnectionError> {
        let conn = self.driver.connect(self.cid, FDOMAIN_VSOCK_PORT).await.map_err(|e| {
            TargetConnectionError::Fatal(anyhow::anyhow!("Connection error: {e:?}"))
        })?;
        let (output, input) = conn.into_split();
        let output = BufReader::with_capacity(BUFFER_SIZE, output);
        let (_sender, errors) = async_channel::unbounded();
        Ok(FDomainConnection {
            output: Box::new(output),
            input: Box::new(input),
            errors,
            main_task: None,
        })
    }
}

impl TryFromEnvContext for UsbConnector {
    fn try_from_env_context<'a>(
        env: &'a EnvironmentContext,
    ) -> LocalBoxFuture<'a, ffx_command_error::Result<Self>> {
        Box::pin(async {
            let resolution = Resolution::try_from_env_context(env).await?;
            let cid = resolution.usb_cid().ok_or_else(|| {
                ffx_command_error::user_error!(
                    "query did not resolve a USB CID. Resolved the following: {:?}",
                    resolution,
                )
            })?;
            UsbConnector::new(cid, env).await.bug().map_err(Into::into)
        })
    }
}

impl TargetConnector for UsbConnector {
    const CONNECTION_TYPE: &'static str = "USB VSOCK";

    async fn connect(&mut self) -> Result<TargetConnection, TargetConnectionError> {
        match self.connect_fdomain().await {
            Ok(fdomain) => Ok(TargetConnection::FDomain(fdomain)),
            Err(e) => {
                // Eventually we should just return the error here, making
                // FDomain authoritative about whether the device is
                // connectable. For now we'll fall through because it's less
                // likely to cause breakages prior to migration.
                log::warn!("Connecting with FDomain encountered error {e:?}");
                self.connect_overnet().await.map(TargetConnection::Overnet)
            }
        }
    }
}

fn daemon_autostart_cmd(
    path: &PathBuf,
    context: &EnvironmentContext,
    cmd_path: Option<&str>,
) -> Result<Option<std::process::Command>, ffx_config::environment::ContextError> {
    if context.is_strict() || context.is_isolated() {
        return Ok(None);
    }

    if !context.get(CONFIG_START_DRIVER).unwrap_or(true) {
        return Ok(None);
    }

    let mut cmd = if let Some(cmd_path) = cmd_path {
        std::process::Command::new(cmd_path)
    } else {
        context.rerun_prefix()?
    };
    let socket_path_config = serde_json::to_string(&serde_json::json!({
        "connectivity": {
            "usb_socket_path": path,
        }
    }))?;

    cmd.args(["-c", socket_path_config.as_str(), "usb-driver", "--background"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    Ok(Some(cmd))
}

/// Try to auto-start the daemon if it is appropriate to do so.
pub fn try_daemon_autostart(path: &PathBuf, context: &EnvironmentContext) {
    if std::os::unix::net::UnixStream::connect(path).is_ok() {
        return;
    }

    let mut cmd = match daemon_autostart_cmd(path, context, None) {
        Ok(Some(cmd)) => cmd,
        Ok(None) => return,
        Err(error) => {
            log::warn!(error:?; "Could not get rerun prefix to spawn USB driver");
            return;
        }
    };

    let child = match cmd.spawn() {
        Ok(child) => child,
        Err(error) => {
            log::warn!(error:?; "Could not spawn USB driver process");
            return;
        }
    };

    let output = match child.wait_with_output() {
        Ok(output) => output,
        Err(error) => {
            log::warn!(error:?; "Error waiting for USB driver to start");
            return;
        }
    };

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        log::warn!(
            exit_status:? = output.status,
            stdout = stdout.as_str(),
            stderr = stderr.as_str();
            "USB driver exited with bad status");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[fuchsia::test]
    async fn test_daemon_autostart_cmd_no_injection() {
        let context = EnvironmentContext::no_context(
            ffx_config::environment::ExecutableKind::Test,
            ffx_config::ConfigMap::new(),
            None,
            true,
        )
        .unwrap();
        let malicious_path = PathBuf::from("/tmp/s,ffx.subtool-search-paths=/path/to/evil/dir");

        let cmd = daemon_autostart_cmd(&malicious_path, &context, Some("ffx"))
            .unwrap()
            .expect("should create command for non-isolated context");

        let args: Vec<String> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        let c_pos = args.iter().position(|a| a == "-c").expect("should contain -c arg");
        let config_val = &args[c_pos + 1];

        // Verify config parses as valid JSON object and does not inject ffx.subtool-search-paths
        let parsed: Value =
            serde_json::from_str(config_val).expect("config argument must be valid JSON");
        assert_eq!(
            parsed,
            serde_json::json!({
                "connectivity": {
                    "usb_socket_path": "/tmp/s,ffx.subtool-search-paths=/path/to/evil/dir"
                }
            })
        );
        assert!(parsed.get("ffx").is_none());

        // Verify ffx_config runtime parsing treats it safely
        let runtime_parsed = ffx_config::runtime::populate_runtime(&[config_val.clone()], None)
            .expect("runtime should parse config");
        assert_eq!(
            runtime_parsed.get("connectivity").and_then(|c| c.get("usb_socket_path")),
            Some(&Value::String("/tmp/s,ffx.subtool-search-paths=/path/to/evil/dir".to_string()))
        );
        assert!(runtime_parsed.get("ffx").is_none());
    }

    #[fuchsia::test]
    async fn test_daemon_autostart_disabled_by_config() {
        let test_env = ffx_config::test_env()
            .user_config(CONFIG_START_DRIVER, serde_json::json!(false))
            .build()
            .unwrap();

        let path = PathBuf::from("/tmp/usb.sock");
        let res = daemon_autostart_cmd(&path, &test_env.context, Some("ffx")).unwrap();
        assert!(res.is_none());
    }

    #[fuchsia::test]
    async fn test_try_daemon_autostart_socket_already_connected() {
        let temp_dir = tempfile::tempdir().unwrap();
        let socket_path = temp_dir.path().join("test_usb.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();

        let context = EnvironmentContext::no_context(
            ffx_config::environment::ExecutableKind::Test,
            ffx_config::ConfigMap::new(),
            None,
            false,
        )
        .unwrap();

        // If the socket is already open, try_daemon_autostart should return immediately without error
        try_daemon_autostart(&socket_path, &context);
    }
}
