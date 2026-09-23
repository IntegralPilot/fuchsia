// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::Resolution;
use crate::target_connector::{
    BUFFER_SIZE, ConnectionStreamError, FDomainConnection, OvernetConnection, TargetConnection,
    TargetConnectionError, TargetConnector,
};
use anyhow::Result;
use async_channel::Sender;
use ffx_command_error::FfxContext as _;
use ffx_config::{EnvironmentContext, TryFromEnvContext};
use ffx_ssh::ssh::{SshError, build_ssh_command_with_env};
use fuchsia_async::Task;
use futures::future::LocalBoxFuture;
use netext::ScopedSocketAddr;
use nix::sys::signal::Signal::SIGKILL;
use nix::sys::signal::kill;
use nix::sys::wait::waitpid;
use nix::unistd::Pid;
use std::fmt::Debug;
use std::net::SocketAddr;
use std::process::Stdio;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader, ErrorKind};
use tokio::process::{Child, ChildStderr};

impl From<SshError> for TargetConnectionError {
    fn from(ssh_err: SshError) -> Self {
        use SshError::*;
        match &ssh_err {
            // These errors are considered potentially recoverable, as they can often surface when
            // a device is actively rebooting while trying to reconnect to it.
            Unknown(_)
            | Timeout
            | ConnectionRefused
            | UnknownNameOrService
            | NoRouteToHost
            | NetworkUnreachable
            | ConnectionClosedByRemoteHost => TargetConnectionError::NonFatal(ssh_err.into()),
            // Note: this error is encountered as a side-effect of trying to `ssh` into a device
            // that is actively rebooting, and a user is invoking `ffx target wait`. The issue here
            // is that the scope ID of the network interface for the device, if it is IPv6
            // link-local, is deemed an invalid argument, because `ssh` thinks it cannot exist
            // (since there is no interface available during reboot). Since this is working from a
            // cached address, this causes this kind of error.
            //
            // This could be potentially hazardous, however, as it is not clear if all cases in
            // which this error surfaces are the same. It should be made clear to the user _why_
            // this continues to attempt connecting ot the device. We can presume we're going to
            // reasonably not encounter this error since we have an array of tests for `ssh`
            // connections, but this does not guarantee a lack of regression later on. That being
            // said, we would like to move away from `ssh` as a transport layer altogether, so
            // so hopefully this won't present itself as an issue.
            InvalidArgument => TargetConnectionError::NonFatal(ssh_err.into()),
            // These errors are unrecoverable, as they are fundamental errors in an existing
            // configuration.
            PermissionDenied | KeyVerificationFailure | TargetIncompatible => {
                TargetConnectionError::Fatal(ssh_err.into())
            }
        }
    }
}

enum FDomainConnectionError {
    ConnectionError(TargetConnectionError),
    NotSupported,
}

pub struct SshConnector {
    overnet_cmd: Option<Child>,
    fdomain_cmd: Option<Child>,
    target: ScopedSocketAddr,
    env_context: EnvironmentContext,
}

impl Debug for SshConnector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshConnector")
            .field("target", &self.target)
            .field("overnet_cmd", &self.overnet_cmd)
            .field("fdomain_cmd", &self.overnet_cmd)
            .finish()
    }
}

impl SshConnector {
    pub fn new(target: ScopedSocketAddr, env_context: &EnvironmentContext) -> Result<Self> {
        Ok(Self { overnet_cmd: None, fdomain_cmd: None, target, env_context: env_context.clone() })
    }

    /// This is mainly for diagnostics/reporting info to the user. This takes the usual command
    /// with which fdomain is started and converts it into a readable string.
    pub async fn fdomain_command(&self) -> std::result::Result<String, crate::FfxTargetCrateError> {
        let cmd = make_fdomain_ssh_command(self.target.clone(), &self.env_context).await?;
        let envs = cmd
            .as_std()
            .get_envs()
            .filter_map(|(k, v)| v.map(|v_unwrapped| (k, v_unwrapped)))
            .map(|(k, v)| format!("{}={}", k.to_string_lossy(), v.to_string_lossy()))
            .collect::<Vec<_>>()
            .join(" ");
        let cmd_main = cmd.as_std().get_program().to_string_lossy();
        let args_str =
            cmd.as_std().get_args().map(|arg| arg.to_string_lossy()).collect::<Vec<_>>().join(" ");
        Ok(format!("{} {} {}", envs, cmd_main, args_str))
    }
}

type StderrLineReader = tokio::io::Lines<BufReader<ChildStderr>>;

async fn read_stderr(
    mut stderr: StderrLineReader,
    error_sender: Sender<ConnectionStreamError>,
    ctx: &EnvironmentContext,
) {
    let log_file = ffx_ssh::parse::ssh_log_file(ctx);
    while let Ok(Some(line)) = stderr.next_line().await {
        if let Some(file) = log_file {
            ffx_ssh::parse::write_ssh_log("E", &line, file);
        }
        // This abandons the structured error as this output is intended to provide debugging after
        // an ssh connection has failed. The error sender is only here to show the verbatim error
        // so that it can be drained in the event of the SshConnector disconnecting.
        if ffx_ssh::parse::ssh_stderr_to_pipe_error(&line).is_some() {
            match error_sender
                .send(ConnectionStreamError::Forwarded(anyhow::anyhow!("SSH stderr: {line}")))
                .await
            {
                Err(_e) => break,
                Ok(_) => {}
            }
        }
    }
}

impl SshConnector {
    async fn attempt_connect_overnet(
        &mut self,
        use_log_id: bool,
    ) -> Result<Result<OvernetConnection, ffx_ssh::parse::PipeError>, TargetConnectionError> {
        self.overnet_cmd = Some(
            start_overnet_ssh_command(self.target.clone(), &self.env_context, use_log_id).await?,
        );
        let cmd = self.overnet_cmd.as_mut().unwrap();
        let mut stdout = BufReader::with_capacity(
            BUFFER_SIZE,
            cmd.stdout.take().expect("process should have stdout"),
        );
        let mut stderr = BufReader::with_capacity(
            BUFFER_SIZE,
            cmd.stderr.take().expect("process should have stderr"),
        );
        match ffx_ssh::parse::parse_ssh_output(&mut stdout, &mut stderr, &self.env_context).await {
            Ok((_addr, _overnet_id)) => {
                let stdin = cmd.stdin.take().expect("process should have stdin");
                let stderr = stderr.lines();
                let (error_sender, errors_receiver) = async_channel::unbounded();
                let stderr_ctx = self.env_context.clone();
                let stderr_reader =
                    async move { read_stderr(stderr, error_sender, &stderr_ctx).await };
                let main_task = Some(Task::local(stderr_reader));
                Ok(Ok(OvernetConnection {
                    output: Box::new(stdout),
                    input: Box::new(stdin),
                    errors: errors_receiver,
                    main_task,
                }))
            }
            Err(e) => Ok(Err(e)),
        }
    }

    async fn connect_overnet(&mut self) -> Result<OvernetConnection, TargetConnectionError> {
        let mut use_log_id = true;
        loop {
            match self.attempt_connect_overnet(use_log_id).await? {
                Ok(conn) => return Ok(conn),
                Err(ffx_ssh::parse::PipeError::NoLogIdSupport) if use_log_id => {
                    log::info!("Target does not support log-id, retrying without it");
                    try_ssh_cmd_cleanup(
                        self.overnet_cmd.take().expect("ssh command must have started"),
                    )
                    .await?;
                    use_log_id = false;
                    // continue loop
                }
                Err(e) => {
                    log::warn!("SSH pipe error encountered {e:?}");
                    try_ssh_cmd_cleanup(
                        self.overnet_cmd.take().expect("ssh command must have started"),
                    )
                    .await?;
                    return Err(ffx_ssh::ssh::SshError::from(e.to_string()).into());
                }
            }
        }
    }

    pub async fn connect_via_fdomain(
        &mut self,
    ) -> Result<FDomainConnection, TargetConnectionError> {
        self.connect_fdomain().await.map_err(|e| match e {
            // TODO(b/421013405): This could likely be much more informative.
            // Why isn't it supported? Version skew? What can we do if this is the case?
            FDomainConnectionError::NotSupported => {
                TargetConnectionError::Fatal(anyhow::anyhow!("FDomain not supported"))
            }
            FDomainConnectionError::ConnectionError(other) => other,
        })
    }

    async fn connect_fdomain(&mut self) -> Result<FDomainConnection, FDomainConnectionError> {
        self.fdomain_cmd = Some(
            start_fdomain_ssh_command(self.target.clone(), &self.env_context)
                .await
                .map_err(|x| FDomainConnectionError::ConnectionError(x.into()))?,
        );
        let cmd = self.fdomain_cmd.as_mut().unwrap();
        let mut stdout = BufReader::with_capacity(
            BUFFER_SIZE,
            cmd.stdout.take().expect("process should have stdout"),
        );
        let mut stderr = BufReader::with_capacity(
            BUFFER_SIZE,
            cmd.stderr.take().expect("process should have stderr"),
        );
        let mut ack = [0u8; 3];
        match stdout.read_exact(&mut ack).await {
            Ok(_) => (),
            Err(e) => {
                let mut stderr_content = String::new();
                if e.kind() == ErrorKind::UnexpectedEof {
                    let _ = stderr.read_to_string(&mut stderr_content).await;
                }
                if let Some(cmd) = self.fdomain_cmd.take() {
                    let _ = try_ssh_cmd_cleanup(cmd).await;
                }

                if stderr_content.is_empty() {
                    return Err(FDomainConnectionError::ConnectionError(
                        TargetConnectionError::NonFatal(e.into()),
                    ));
                }

                let ssh_err = ffx_ssh::ssh::SshError::from(stderr_content);
                match ssh_err {
                    SshError::Unknown(_) => {
                        // If it is unknown, we assume FDomain is not supported or broken,
                        // and fallback to Overnet.
                        return Err(FDomainConnectionError::NotSupported);
                    }
                    _ => {
                        return Err(FDomainConnectionError::ConnectionError(
                            TargetConnectionError::from(ssh_err),
                        ));
                    }
                }
            }
        }

        if ack != *b"OK\n" {
            if let Some(cmd) = self.fdomain_cmd.take() {
                let _ = try_ssh_cmd_cleanup(cmd).await;
            }
            return Err(FDomainConnectionError::ConnectionError(
                ffx_ssh::ssh::SshError::Unknown(format!("Unknown Ack string {ack:?}")).into(),
            ));
        }
        let stdin = cmd.stdin.take().expect("process should have stdin");
        let stderr = stderr.lines();
        let (error_sender, errors_receiver) = async_channel::unbounded();
        let stderr_ctx = self.env_context.clone();
        let stderr_reader = async move { read_stderr(stderr, error_sender, &stderr_ctx).await };
        let main_task = Some(Task::local(stderr_reader));
        Ok(FDomainConnection {
            output: Box::new(stdout),
            input: Box::new(stdin),
            errors: errors_receiver,
            main_task,
        })
    }
}

impl TryFromEnvContext for SshConnector {
    fn try_from_env_context<'a>(
        env: &'a EnvironmentContext,
    ) -> LocalBoxFuture<'a, ffx_command_error::Result<Self>> {
        Box::pin(async {
            let resolution = Resolution::try_from_env_context(env).await?;
            let res = resolution.addr().map_err(|_| {
                ffx_command_error::user_error!(
                    "query did not resolve an IP address. Resolved the following: {:?}",
                    resolution,
                )
            })?;
            let target = ScopedSocketAddr::from_socket_addr(res)
                .user_message(format!("Failed to verify IP '{res}'"))?;
            SshConnector::new(target, env).bug().map_err(Into::into)
        })
    }
}

async fn make_ssh_command(
    target: ScopedSocketAddr,
    env_context: &EnvironmentContext,
    args: Vec<&str>,
) -> Result<tokio::process::Command> {
    let ssh_path: String = env_context.get("ssh.path").unwrap_or_else(|_| "ssh".to_string());
    let ssh = tokio::process::Command::from(
        build_ssh_command_with_env(&ssh_path, target, env_context, args).await?,
    );
    Ok(ssh)
}

async fn spawn_ssh_command(mut ssh: tokio::process::Command, label: &str) -> Result<Child> {
    log::debug!("SshConnector starting {label} invoking: {ssh:?}");
    let ssh_cmd = ssh.stdout(Stdio::piped()).stdin(Stdio::piped()).stderr(Stdio::piped());
    Ok(ssh_cmd.spawn().bug_context("spawning ssh command")?)
}

async fn make_fdomain_ssh_command(
    target: ScopedSocketAddr,
    env_context: &EnvironmentContext,
) -> Result<tokio::process::Command> {
    let log_id = format!("{:0>20}", *ffx_config::logging::LOGGING_ID);
    let args = vec!["fdomain_runner", "--log-id", &log_id];
    make_ssh_command(target, env_context, args).await
}

async fn start_fdomain_ssh_command(
    target: ScopedSocketAddr,
    env_context: &EnvironmentContext,
) -> Result<Child> {
    let ssh = make_fdomain_ssh_command(target, env_context).await?;
    spawn_ssh_command(ssh, "start_fdomain_ssh").await
}

async fn start_overnet_ssh_command(
    target: ScopedSocketAddr,
    env_context: &EnvironmentContext,
    use_log_id: bool,
) -> Result<Child> {
    // Converting milliseconds since unix epoch should have enough bits for u64. As of writing
    // it takes up 43 of the 128 bits to represent the number.
    let circuit_id =
        SystemTime::now().duration_since(UNIX_EPOCH).expect("system time").as_millis() as u64;
    let circuit_id_str = format!("{}", circuit_id);
    let log_id = format!("{:0>20}", *ffx_config::logging::LOGGING_ID);
    let rev: u64 =
        version_history_data::HISTORY.get_misleading_version_for_ffx().abi_revision.as_u64();
    let abi_revision = format!("{rev}");
    let mut args = vec![
        "remote_control_runner",
        "--circuit",
        &circuit_id_str,
        "--abi-revision",
        &abi_revision,
    ];
    if use_log_id {
        args.push("--log-id");
        args.push(&log_id);
    }
    let ssh = make_ssh_command(target, env_context, args).await?;
    spawn_ssh_command(ssh, "overnet").await
}

async fn try_ssh_cmd_cleanup(mut cmd: Child) -> Result<()> {
    cmd.kill().await?;
    if let Some(status) = cmd.try_wait()? {
        match status.code() {
            // Possible to catch more error codes here, hence the use of a match.
            Some(255) => {
                log::warn!("SSH ret code: 255. Unexpected session termination.")
            }
            _ => log::error!("SSH exited with error code: {status}. "),
        }
    } else {
        log::error!("ssh child has not ended, trying one more time then ignoring it.");
        fuchsia_async::Timer::new(std::time::Duration::from_secs(2)).await;
        log::error!("ssh child status is {:?}", cmd.try_wait());
    }
    Ok(())
}

impl TargetConnector for SshConnector {
    const CONNECTION_TYPE: &'static str = "ssh";

    async fn connect(&mut self) -> Result<TargetConnection, TargetConnectionError> {
        match self.connect_fdomain().await {
            Ok(fdomain) => Ok(TargetConnection::FDomain(fdomain)),
            Err(FDomainConnectionError::NotSupported) => {
                self.connect_overnet().await.map(TargetConnection::Overnet)
            }
            Err(FDomainConnectionError::ConnectionError(other)) => Err(other),
        }
    }

    fn device_address(&self) -> Option<SocketAddr> {
        Some(*self.target.addr())
    }
}

impl Drop for SshConnector {
    fn drop(&mut self) {
        for (name, mut cmd) in self
            .overnet_cmd
            .take()
            .into_iter()
            .map(|x| ("Overnet", x))
            .chain(self.fdomain_cmd.take().into_iter().map(|x| ("FDomain", x)))
        {
            let pid = Pid::from_raw(cmd.id().unwrap() as i32);
            match cmd.try_wait() {
                Ok(Some(result)) => {
                    log::info!("{name} FidlPipe exited with {}", result);
                }
                Ok(None) => {
                    let _ = kill(pid, SIGKILL)
                        .map_err(|e| log::warn!("failed to kill {name} FidlPipe command: {:?}", e));
                    let _ = waitpid(pid, None).map_err(|e| {
                        log::warn!("failed to clean up {name} FidlPipe command: {:?}", e)
                    });
                }
                Err(e) => {
                    log::warn!("failed to soft-wait FidlPipe command: {:?}", e);
                    let _ = kill(pid, SIGKILL)
                        .map_err(|e| log::warn!("failed to kill {name} FidlPipe command: {:?}", e));
                    let _ = waitpid(pid, None).map_err(|e| {
                        log::warn!("failed to clean up {name} FidlPipe command: {:?}", e)
                    });
                }
            };
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use ffx_config::environment::TestEnvBuilder;
    use std::fs::File;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    #[test]
    fn test_ssh_error_conversion() {
        use SshError::*;
        let err = Unknown("foobar".to_string());
        assert!(matches!(TargetConnectionError::from(err), TargetConnectionError::NonFatal(_)));
        let err = PermissionDenied;
        assert!(matches!(TargetConnectionError::from(err), TargetConnectionError::Fatal(_)));
        let err = ConnectionRefused;
        assert!(matches!(TargetConnectionError::from(err), TargetConnectionError::NonFatal(_)));
        let err = UnknownNameOrService;
        assert!(matches!(TargetConnectionError::from(err), TargetConnectionError::NonFatal(_)));
        let err = KeyVerificationFailure;
        assert!(matches!(TargetConnectionError::from(err), TargetConnectionError::Fatal(_)));
        let err = NoRouteToHost;
        assert!(matches!(TargetConnectionError::from(err), TargetConnectionError::NonFatal(_)));
        let err = NetworkUnreachable;
        assert!(matches!(TargetConnectionError::from(err), TargetConnectionError::NonFatal(_)));
        let err = InvalidArgument;
        assert!(matches!(TargetConnectionError::from(err), TargetConnectionError::NonFatal(_)));
        let err = TargetIncompatible;
        assert!(matches!(TargetConnectionError::from(err), TargetConnectionError::Fatal(_)));
        let err = Timeout;
        assert!(matches!(TargetConnectionError::from(err), TargetConnectionError::NonFatal(_)));
        let err = ConnectionClosedByRemoteHost;
        assert!(matches!(TargetConnectionError::from(err), TargetConnectionError::NonFatal(_)));
    }

    fn write_mock_ssh(dir: &tempfile::TempDir, content: &str) -> PathBuf {
        let path = dir.path().join("ssh");
        let mut file = File::create(&path).unwrap();
        file.write_all(content.as_bytes()).unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        path
    }

    #[fuchsia::test]
    async fn test_connect_via_fdomain_fallback_and_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let mock_ssh_script = r#"#!/bin/bash
PORT=""
args=("$@")
for ((i=0; i<${#args[@]}; i++)); do
  if [[ "${args[i]}" == "-p" ]]; then
    PORT="${args[i+1]}"
    break
  fi
done

case "$PORT" in
  "2201")
    printf "OK\n"
    sleep 3600
    ;;
  "2202")
    echo "fdomain_runner: not found" >&2
    exit 127
    ;;
  "2203")
    echo "bash: fdomain_runner: command not found" >&2
    exit 127
    ;;
  "2204")
    echo "ssh: connect to host 127.0.0.1 port 2204: Connection refused" >&2
    exit 255
    ;;
  "2205")
    echo "Permission denied (publickey)." >&2
    exit 255
    ;;
  *)
    echo "Unknown port $PORT" >&2
    exit 1
    ;;
esac
"#;
        let ssh_path = write_mock_ssh(&tmp, mock_ssh_script);
        let priv_key_path = tmp.path().join("fake_key");
        File::create(&priv_key_path).unwrap();

        let test_env = TestEnvBuilder::default()
            .user_config("ssh.path", ssh_path.to_str().unwrap())
            .user_config("ssh.priv", priv_key_path.to_str().unwrap())
            .user_config("ssh.controlmaster.mode", "none")
            .build()
            .unwrap();

        // 1. Test Success (Port 2201)
        {
            let target_addr: SocketAddr = "127.0.0.1:2201".parse().unwrap();
            let scoped_addr = ScopedSocketAddr::from_socket_addr(target_addr).unwrap();
            let mut connector = SshConnector::new(scoped_addr, &test_env.context).unwrap();
            let res = connector.connect_via_fdomain().await;
            assert!(res.is_ok(), "Expected Ok for success case, got {:?}", res);
        }

        // 2. Test Not Supported (Port 2202) -> Fatal(FDomain not supported)
        {
            let target_addr: SocketAddr = "127.0.0.1:2202".parse().unwrap();
            let scoped_addr = ScopedSocketAddr::from_socket_addr(target_addr).unwrap();
            let mut connector = SshConnector::new(scoped_addr, &test_env.context).unwrap();
            let res = connector.connect_via_fdomain().await;
            assert!(res.is_err(), "Expected Err for not_supported case");
            let err = res.unwrap_err();
            assert!(
                matches!(err, TargetConnectionError::Fatal(_)),
                "Expected Fatal for not_supported case, got {:?}",
                err
            );
            assert!(
                format!("{:?}", err).contains("FDomain not supported"),
                "Expected 'FDomain not supported' error message, got: '{:?}'",
                err
            );
        }

        // 3. Test Command Not Found (Port 2203) -> Fatal(FDomain not supported)
        {
            let target_addr: SocketAddr = "127.0.0.1:2203".parse().unwrap();
            let scoped_addr = ScopedSocketAddr::from_socket_addr(target_addr).unwrap();
            let mut connector = SshConnector::new(scoped_addr, &test_env.context).unwrap();
            let res = connector.connect_via_fdomain().await;
            assert!(res.is_err(), "Expected Err for command_not_found case");
            let err = res.unwrap_err();
            assert!(
                matches!(err, TargetConnectionError::Fatal(_)),
                "Expected Fatal for command_not_found case, got {:?}",
                err
            );
            assert!(
                format!("{:?}", err).contains("FDomain not supported"),
                "Expected 'FDomain not supported' error message, got: '{:?}'",
                err
            );
        }

        // 4. Test Connection Refused (Port 2204) -> NonFatal
        {
            let target_addr: SocketAddr = "127.0.0.1:2204".parse().unwrap();
            let scoped_addr = ScopedSocketAddr::from_socket_addr(target_addr).unwrap();
            let mut connector = SshConnector::new(scoped_addr, &test_env.context).unwrap();
            let res = connector.connect_via_fdomain().await;
            assert!(res.is_err(), "Expected Err for connection_refused case");
            let err = res.unwrap_err();
            assert!(
                matches!(err, TargetConnectionError::NonFatal(_)),
                "Expected NonFatal for connection_refused case, got {:?}",
                err
            );
        }

        // 5. Test Permission Denied (Port 2205) -> Fatal
        {
            let target_addr: SocketAddr = "127.0.0.1:2205".parse().unwrap();
            let scoped_addr = ScopedSocketAddr::from_socket_addr(target_addr).unwrap();
            let mut connector = SshConnector::new(scoped_addr, &test_env.context).unwrap();
            let res = connector.connect_via_fdomain().await;
            assert!(res.is_err(), "Expected Err for permission_denied case");
            let err = res.unwrap_err();
            assert!(
                matches!(err, TargetConnectionError::Fatal(_)),
                "Expected Fatal for permission_denied case, got {:?}",
                err
            );
            assert!(
                !format!("{:?}", err).contains("FDomain not supported"),
                "Expected NOT 'FDomain not supported' error message, got: '{:?}'",
                err
            );
        }
    }

    #[fuchsia::test]
    async fn test_connect_overnet_fallback_log_id() {
        let tmp = tempfile::tempdir().unwrap();
        let mock_ssh_script = r#"#!/bin/bash
# Check args
HAS_LOG_ID=false
IS_FDOMAIN=false
IS_OVERNET=false

for arg in "$@"; do
  if [[ "$arg" == "fdomain_runner" ]]; then
    IS_FDOMAIN=true
  fi
  if [[ "$arg" == "remote_control_runner" ]]; then
    IS_OVERNET=true
  fi
  if [[ "$arg" == "--log-id" ]]; then
    HAS_LOG_ID=true
  fi
done

if $IS_FDOMAIN; then
  echo "fdomain_runner: not found" >&2
  exit 127
fi

if $IS_OVERNET; then
  if $HAS_LOG_ID; then
    echo "Unrecognized argument: --log-id" >&2
    exit 1
  else
    printf "++ 127.0.0.1 1234 127.0.0.1 22 ++\n"
    sleep 3600
  fi
fi
"#;
        let ssh_path = write_mock_ssh(&tmp, mock_ssh_script);
        let priv_key_path = tmp.path().join("fake_key");
        File::create(&priv_key_path).unwrap();

        let test_env = TestEnvBuilder::default()
            .user_config("ssh.path", ssh_path.to_str().unwrap())
            .user_config("ssh.priv", priv_key_path.to_str().unwrap())
            .user_config("ssh.controlmaster.mode", "none")
            .build()
            .unwrap();

        let target_addr: SocketAddr = "127.0.0.1:2201".parse().unwrap();
        let scoped_addr = ScopedSocketAddr::from_socket_addr(target_addr).unwrap();
        let mut connector = SshConnector::new(scoped_addr, &test_env.context).unwrap();

        let res = connector.connect().await;
        assert!(res.is_ok(), "Expected Ok for fallback success, got {:?}", res);
        let conn = res.unwrap();
        assert!(
            matches!(conn, TargetConnection::Overnet(_)),
            "Expected Overnet connection, got {:?}",
            conn
        );
    }

    #[fuchsia::test]
    async fn test_connect_fdomain_success_does_not_connect_overnet() {
        let tmp = tempfile::tempdir().unwrap();
        let mock_ssh_script = r#"#!/bin/bash
for arg in "$@"; do
  if [[ "$arg" == "fdomain_runner" ]]; then
    printf "OK\n"
    sleep 3600
  fi
  if [[ "$arg" == "remote_control_runner" ]]; then
    echo "Overnet should not be called!" >&2
    exit 1
  fi
done
"#;
        let ssh_path = write_mock_ssh(&tmp, mock_ssh_script);
        let priv_key_path = tmp.path().join("fake_key");
        File::create(&priv_key_path).unwrap();

        let test_env = TestEnvBuilder::default()
            .user_config("ssh.path", ssh_path.to_str().unwrap())
            .user_config("ssh.priv", priv_key_path.to_str().unwrap())
            .user_config("ssh.controlmaster.mode", "none")
            .build()
            .unwrap();

        let target_addr: SocketAddr = "127.0.0.1:2201".parse().unwrap();
        let scoped_addr = ScopedSocketAddr::from_socket_addr(target_addr).unwrap();
        let mut connector = SshConnector::new(scoped_addr, &test_env.context).unwrap();

        let res = connector.connect().await;
        assert!(res.is_ok(), "Expected Ok for fdomain success, got {:?}", res);
        let conn = res.unwrap();
        assert!(
            matches!(conn, TargetConnection::FDomain(_)),
            "Expected FDomain connection, got {:?}",
            conn
        );
    }
}
