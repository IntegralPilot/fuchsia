// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::config::SshConfig;
use ffx_config::EnvironmentContext;
use netext::ScopedSocketAddr;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::net::SocketAddr;
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use thiserror::Error;
use tokio::process::Command;

const SSH_PRIV: &str = "ssh.priv";
const SSH_CONTROLMASTER_MODE: &str = "ssh.controlmaster.mode";
const SSH_CONTROLMASTER_PATH: &str = "ssh.controlmaster.path";
const SSH_CONTROLMASTER_DIR: &str = "ssh.controlmaster.dir";
pub const KEEPALIVE_TIMEOUT_CONFIG: &str = "ssh.keepalive_timeout";
pub const CONNECT_TIMEOUT_CONFIG: &str = "ssh.connect_timeout";
pub const CONNECTION_ATTEMPTS_CONFIG: &str = "ssh.connection_attempts";
pub const IDENTITIES_ONLY_CONFIG: &str = "ssh.identities_only";

#[derive(Error, Debug)]
pub enum SshCommandError {
    #[error("Missing SSH command")]
    MissingSshCommand,
    #[error("SSH configuration error")]
    Config(#[from] crate::config::SshConfigError),
    #[error("FFX configuration error")]
    FfxConfig(#[from] ffx_config::ConfigError),
    #[error("ControlMaster management error")]
    ControlMaster(#[from] ManageSshControlMasterError),
}

#[derive(Error, Debug, Hash, Clone, PartialEq, Eq)]
pub enum SshError {
    #[error("unknown ssh error: {0}")]
    Unknown(String),
    #[error("permission denied")]
    PermissionDenied,
    #[error("connection refused")]
    ConnectionRefused,
    #[error("unknown name or service")]
    UnknownNameOrService,
    #[error("timeout")]
    Timeout,
    #[error("key verification failure")]
    KeyVerificationFailure,
    #[error("no route to host")]
    NoRouteToHost,
    #[error("network unreachable")]
    NetworkUnreachable,
    #[error("invalid argument")]
    InvalidArgument,
    #[error("target not compatible")]
    TargetIncompatible,
    #[error("connection closed by remote host")]
    ConnectionClosedByRemoteHost,
}

impl From<String> for SshError {
    fn from(s: String) -> Self {
        if s.contains("Permission denied") {
            return Self::PermissionDenied;
        }
        if s.contains("Connection refused") {
            return Self::ConnectionRefused;
        }
        if s.contains("Name or service not known") {
            return Self::UnknownNameOrService;
        }
        if s.contains("Connection timed out") {
            return Self::Timeout;
        }
        if s.contains("Host key verification failed") {
            return Self::KeyVerificationFailure;
        }
        if s.contains("No route to host") {
            return Self::NoRouteToHost;
        }
        if s.contains("Network is unreachable") {
            return Self::NetworkUnreachable;
        }
        if s.contains("Invalid argument") {
            return Self::InvalidArgument;
        }
        if s.contains("not compatible") {
            return Self::TargetIncompatible;
        }
        if s.contains("Connection closed by remote host") {
            return Self::ConnectionClosedByRemoteHost;
        }
        return Self::Unknown(s);
    }
}

impl From<&str> for SshError {
    fn from(s: &str) -> Self {
        Self::from(s.to_owned())
    }
}

pub fn get_ssh_key_paths_from_env(
    env: &EnvironmentContext,
) -> Result<Vec<String>, SshCommandError> {
    env.query(SSH_PRIV).build().get_file(env).map_err(SshCommandError::FfxConfig)
}

fn apply_auth_sock(cmd: &mut Command, context: &EnvironmentContext) {
    const SSH_AUTH_SOCK: &str = "ssh.auth-sock";
    if let Ok(path) = context.get::<String, _>(SSH_AUTH_SOCK) {
        log::debug!("SSH_AUTH_SOCK retrieved via config: {}", path);
        cmd.env("SSH_AUTH_SOCK", path.as_str());
        if !std::fs::exists(path.as_str()).unwrap() {
            log::warn!("SSH_AUTH_SOCK file does not exist at: {}", path);
        }
    }
}

fn apply_identities_only(
    config: &mut SshConfig,
    context: &EnvironmentContext,
) -> Result<(), SshCommandError> {
    const SSH_IDENTITIES_ONLY: &str = "ssh.identities-only";
    if let Ok(identities_only) = context.get::<bool, _>(SSH_IDENTITIES_ONLY) {
        log::debug!("ssh.identities-only retrieved via config: {}", identities_only);
        config.set("IdentitiesOnly", if identities_only { "yes" } else { "no" })?;
    }
    Ok(())
}

async fn build_ssh_command_with_ssh_path(
    ssh_path: &str,
    addr: ScopedSocketAddr,
    command: Vec<&str>,
    env: &EnvironmentContext,
) -> Result<Command, SshCommandError> {
    let mut config = SshConfig::new()?;
    build_ssh_command_with_ssh_config(ssh_path, addr, &mut config, command, env).await
}

pub async fn build_ssh_command_with_env(
    ssh_path: &str,
    addr: ScopedSocketAddr,
    env: &EnvironmentContext,
    command: Vec<&str>,
) -> Result<Command, SshCommandError> {
    let mut ssh_config = SshConfig::new()?;
    build_ssh_command_with_ssh_config_and_env(ssh_path, addr, &mut ssh_config, command, env).await
}

pub async fn build_ssh_command_with_ssh_config(
    ssh_path: &str,
    addr: ScopedSocketAddr,
    config: &mut SshConfig,
    command: Vec<&str>,
    env: &EnvironmentContext,
) -> Result<Command, SshCommandError> {
    build_ssh_command_with_ssh_config_and_env(ssh_path, addr, config, command, env).await
}

fn get_addr_port(addr: &ScopedSocketAddr) -> (String, String) {
    let mut addr_str = format!("{}", addr);
    let colon_port = addr_str.split_off(addr_str.rfind(':').expect("socket format includes port"));

    // Remove the enclosing [] used in IPv6 socketaddrs
    let addr_start = if addr_str.starts_with("[") { 1 } else { 0 };
    let addr_end = addr_str.len() - if addr_str.ends_with("]") { 1 } else { 0 };
    let addr_arg = addr_str[addr_start..addr_end].to_string();

    (addr_arg, colon_port[1..].to_string())
}

async fn spawn_controlmaster(
    ssh_path: &str,
    controlmaster_dir: PathBuf,
    ssh_keys: &[String],
    addr: &ScopedSocketAddr,
    config: &SshConfig,
) -> Result<PathBuf, SpawnControlMasterError> {
    // Okay we need to create the socket path, but unix sockets
    // have a limit of 109 characters, which an ipv6 address will eat the
    // vast majority of. Let's go ahead and hash it
    let mut hasher = DefaultHasher::new();
    addr.hash(&mut hasher);
    let hash_value = hasher.finish();
    log::info!("Checking ControlMaster for {}. Hash: {}", addr, hash_value);
    if !std::fs::exists(&controlmaster_dir)? {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700) // Read write execute for owner ownly.
            .create(&controlmaster_dir)?;
    }
    let socket_path = controlmaster_dir.join(hash_value.to_string());
    if socket_path.to_string_lossy().len() > MAX_SOCKET_LEN {
        return Err(SpawnControlMasterError::SocketPathTooLong {
            path: socket_path,
            max_allowed: MAX_SOCKET_LEN,
        });
    }

    if std::fs::exists(&socket_path)? {
        log::debug!("ControlMaster for {} already exists at: {}", addr, socket_path.display());
        return Ok(socket_path);
    }

    let mut c = Command::new(ssh_path);
    // Enable ControlMaster
    c.arg("-M");
    // ControlMaster path
    c.arg("-S");
    c.arg(&socket_path);
    // Forks SSH into the background and prevents it from executing a remote command.
    // This keeps the ControlMaster connection open without an interactive shell.
    c.arg("-fN");
    // Keep the connection alive for 5 minutes max
    c.args(["-o", "ControlPersist=5m"]);
    // No config file
    c.args(["-F", "none"]);
    // Use our custom Ssh Config
    c.args(config.to_args());

    // Add identity keys
    for key in ssh_keys {
        c.arg("-i").arg(key);
    }

    let (addr_arg, port_arg) = get_addr_port(addr);
    c.arg("-p").arg(port_arg);
    c.arg(addr_arg);

    log::debug!("Spawning ssh command for ControlMaster");
    c.stdout(std::process::Stdio::null());
    c.stderr(std::process::Stdio::piped());
    c.stdin(std::process::Stdio::null());
    let output = c.output().await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let ssherr = SshError::from(stderr);
        return Err(SpawnControlMasterError::ControlMasterStartError(ssherr));
    }

    Ok(socket_path)
}

#[derive(Debug)]
pub enum ControlMasterMode {
    None,
    Explicit,
    Managed,
}

#[derive(Error, Debug)]
pub enum ParseControlMasterModeError {
    #[error("Invalid ssh ControlMaster mode: {}", mode)]
    InvalidMode { mode: String },
}

#[derive(Error, Debug)]
pub enum ManageSshControlMasterError {
    #[error("ssh ControlMaster directory was not specified")]
    ControlMasterDirNotSpecified,
    #[error("ssh ControlMaster path was not specified")]
    ControlMasterPathNotSpecified,
    #[error("Error parsing ssh ControlMaster mode")]
    ParseError(#[from] ParseControlMasterModeError),
    #[error("Error reading configuration")]
    ConfigError(#[from] ffx_config::ConfigError),
    #[error("Error spawning ssh control master")]
    SpawnError(#[from] SpawnControlMasterError),
}

const MAX_SOCKET_LEN: usize = 100;

#[derive(Error, Debug)]
pub enum SpawnControlMasterError {
    #[error("parsing ssh configuration")]
    ParseSshConfig(#[from] crate::config::SshConfigError),
    #[error("Socket path \"{path}\" is too long. Maximum is {max_allowed}")]
    SocketPathTooLong { path: PathBuf, max_allowed: usize },
    #[error("failed to start")]
    ControlMasterStartError(#[source] SshError),
    #[error("io error")]
    IOError(#[from] std::io::Error),
}

impl TryFrom<String> for ControlMasterMode {
    type Error = ParseControlMasterModeError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        match value.as_str() {
            "explicit" => Ok(Self::Explicit),
            "managed" => Ok(Self::Managed),
            "none" => Ok(Self::None),
            other => Err(ParseControlMasterModeError::InvalidMode { mode: other.to_owned() }),
        }
    }
}

async fn get_controlmaster_path(
    env: &EnvironmentContext,
    ssh_path: &str,
    addr: &ScopedSocketAddr,
    ssh_keys: &Vec<String>,
    config: &SshConfig,
) -> Result<Option<PathBuf>, ManageSshControlMasterError> {
    let controlmaster_mode_string: Option<String> = env.get(SSH_CONTROLMASTER_MODE)?;
    let controlmaster_mode = match controlmaster_mode_string {
        None => {
            if env.is_isolated() {
                ControlMasterMode::None
            } else {
                ControlMasterMode::Managed
            }
        }
        Some(v) => ControlMasterMode::try_from(v)?,
    };

    match controlmaster_mode {
        ControlMasterMode::None => Ok(None),
        ControlMasterMode::Explicit => {
            // We are told to get the value explicitly
            // Map this error
            let path: Option<String> = env.get(SSH_CONTROLMASTER_PATH)?;
            let Some(path) = path else {
                return Err(ManageSshControlMasterError::ControlMasterPathNotSpecified);
            };
            Ok(Some(PathBuf::from(path)))
        }
        ControlMasterMode::Managed => {
            let controlmaster_dir: Option<String> = env.get(SSH_CONTROLMASTER_DIR)?;
            let Some(controlmaster_dir) = controlmaster_dir else {
                return Err(ManageSshControlMasterError::ControlMasterDirNotSpecified);
            };
            let path = Box::pin(spawn_controlmaster(
                ssh_path,
                controlmaster_dir.into(),
                &ssh_keys,
                addr,
                config,
            ))
            .await?;
            Ok(Some(path))
        }
    }
}

/// Builds the ssh command using the specified ssh configuration and path to the ssh command.
async fn build_ssh_command_with_ssh_config_and_env(
    ssh_path: &str,
    addr: ScopedSocketAddr,
    config: &mut SshConfig,
    command: Vec<&str>,
    env: &EnvironmentContext,
) -> Result<Command, SshCommandError> {
    if ssh_path.is_empty() {
        return Err(SshCommandError::MissingSshCommand);
    }

    let keys = get_ssh_key_paths_from_env(env)?;
    if let Some(keepalive_timeout) = env.get::<Option<u64>, _>(KEEPALIVE_TIMEOUT_CONFIG)? {
        config.set_server_alive_count_max(keepalive_timeout as u16)?;
    }
    if let Some(connect_timeout) = env.get::<Option<u64>, _>(CONNECT_TIMEOUT_CONFIG)? {
        config.set("ConnectTimeout", connect_timeout.to_string())?;
    }
    if let Some(connection_attempts) = env.get::<Option<u64>, _>(CONNECTION_ATTEMPTS_CONFIG)? {
        config.set("ConnectionAttempts", connection_attempts.to_string())?;
    }
    if let Some(identities_only) = env.get::<Option<bool>, _>(IDENTITIES_ONLY_CONFIG)? {
        let val = if identities_only { "yes" } else { "no" };
        config.set("IdentitiesOnly", val.to_string())?;
    }

    // Okay there are two ways we can get here
    // if we have config value ssh.target.control_path, dont spawn one, just use it
    // if we have config value ssh.controlmaster_dir, check the contents of that dir for a
    // properly named socket, use that one. Otherwise create one and then use iter
    // if neither of those config values are set, just dont use a ControlMaster
    let controlmaster_path = get_controlmaster_path(env, ssh_path, &addr, &keys, config).await?;

    let mut c = Command::new(ssh_path);
    // We want to avoid adding the SSH_AUTH_SOCK unless explicitly said to include it (the line
    // after this). Adding SSH_AUTH_SOCK can trigger 1 or more seconds delay depending on the
    // setup in use.
    c.env_remove("SSH_AUTH_SOCK");
    apply_auth_sock(&mut c, env);
    apply_identities_only(config, env)?;
    c.args(["-F", "none"]);
    c.args(config.to_args());

    // And then we'll just use the contromaster part from above here if it exists
    if let Some(control_path) = controlmaster_path {
        c.arg("-S");
        c.arg(control_path);
    }

    for key in keys {
        c.arg("-i").arg(key);
    }

    match addr.addr() {
        SocketAddr::V4(_) => c.arg("-o").arg("AddressFamily=inet"),
        SocketAddr::V6(_) => c.arg("-o").arg("AddressFamily=inet6"),
    };

    if ffx_config::logging::debugging_on(env) {
        c.arg("-vv");
    }

    let (addr_arg, port_arg) = get_addr_port(&addr);
    c.arg("-p").arg(port_arg);
    c.arg(addr_arg);

    c.args(&command);

    return Ok(c);
}

/// Build the ssh command using the default ssh command and configuration.
pub async fn build_ssh_command(
    env: &EnvironmentContext,
    addr: ScopedSocketAddr,
    command: Vec<&str>,
) -> Result<Command, SshCommandError> {
    build_ssh_command_with_ssh_path("ssh", addr, command, env).await
}

/// Queries the target's perspective of the host's IP address by invoking SSH with `echo $SSH_CONNECTION`.
pub async fn get_ssh_host_address(
    ssh_path: &str,
    addr: ScopedSocketAddr,
    env: &EnvironmentContext,
) -> Result<crate::parse::HostAddr, SshError> {
    let args = vec!["echo", "$SSH_CONNECTION"];
    let mut ssh = tokio::process::Command::from(
        build_ssh_command_with_env(ssh_path, addr, env, args)
            .await
            .map_err(|e| SshError::Unknown(e.to_string()))?,
    );

    let ssh_cmd = ssh
        .stdout(std::process::Stdio::piped())
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    let mut child = ssh_cmd.spawn().map_err(|e| SshError::Unknown(e.to_string()))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| SshError::Unknown("unable to get stdout from target pipe".into()))?;

    let mut stdout = tokio::io::BufReader::with_capacity(crate::parse::BUFSIZE, stdout);
    let line = crate::parse::read_ssh_line_with_timeouts(&mut stdout)
        .await
        .map_err(|e| SshError::Unknown(format!("{e:?}")))?;

    line.split(" ")
        .next()
        .filter(|x| !x.is_empty())
        .map(|x| crate::parse::HostAddr(x.to_owned()))
        .ok_or_else(|| SshError::Unknown(format!("Could not parse {line:?}")))
}

/// Build the ssh command using a provided sshconfig file.
pub fn build_ssh_command_with_config_file(
    config_file: &PathBuf,
    addr: ScopedSocketAddr,
    command: Vec<&str>,
    env: &EnvironmentContext,
) -> Result<Command, SshCommandError> {
    let keys = get_ssh_key_paths_from_env(env)?;

    let mut c = Command::new("ssh");
    apply_auth_sock(&mut c, env);
    c.arg("-F").arg(config_file);

    for k in keys {
        c.arg("-i").arg(k);
    }

    let mut addr_str = format!("{}", addr);
    let colon_port = addr_str.split_off(addr_str.rfind(':').expect("socket format includes port"));

    // Remove the enclosing [] used in IPv6 socketaddrs
    let addr_start = if addr_str.starts_with("[") { 1 } else { 0 };
    let addr_end = addr_str.len() - if addr_str.ends_with("]") { 1 } else { 0 };
    let addr_arg = &addr_str[addr_start..addr_end];

    c.arg("-p").arg(&colon_port[1..]);
    c.arg(addr_arg);

    c.args(&command);

    return Ok(c);
}

#[cfg(test)]
mod test {
    use super::*;
    use anyhow::Result;
    use ffx_config::environment::TestEnvBuilder;
    use pretty_assertions::assert_eq;
    use std::io::BufRead;

    fn init_ssh_key(mut builder: TestEnvBuilder) -> Result<(TestEnvBuilder, PathBuf)> {
        let isolate_root = builder.isolate_root();
        let private_path = isolate_root.join("privatekey");
        std::fs::File::create(&private_path)?;

        let builder = builder.user_config(SSH_PRIV, private_path.to_string_lossy());
        Ok((builder, private_path))
    }

    #[fuchsia::test]
    async fn test_build_ssh_command_ipv4() {
        let builder = ffx_config::test_env();
        let (builder, key) = init_ssh_key(builder).unwrap();
        let env = builder.build().unwrap();

        let config = SshConfig::new().expect("default ssh config");
        let addr: SocketAddr = "192.168.0.1:22".parse().unwrap();

        let result = build_ssh_command(
            &env.context,
            ScopedSocketAddr::from_socket_addr(addr).unwrap(),
            vec!["ls"],
        )
        .await
        .unwrap();
        let actual_args: Vec<_> = result.as_std().get_args().map(|a| a.to_string_lossy()).collect();
        let mut expected_args: Vec<String> = vec!["-F".into(), "none".into()];
        expected_args.extend(config.to_args());
        expected_args.extend(
            [
                "-i",
                key.to_str().expect("valid path"),
                "-o",
                "AddressFamily=inet",
                "-p",
                "22",
                "192.168.0.1",
                "ls",
            ]
            .map(String::from),
        );
        assert_eq!(actual_args, expected_args);
    }

    #[fuchsia::test]
    async fn test_build_ssh_command_ipv6() {
        let builder = ffx_config::test_env();
        let (builder, key) = init_ssh_key(builder).unwrap();
        let env = builder.build().unwrap();

        let config = SshConfig::new().expect("default ssh config");
        let addr: SocketAddr = "[fe80::12%1]:8022".parse().unwrap();
        // This presumes the host device running the test is linux and has a `lo` loopback device.
        let result = build_ssh_command(
            &env.context,
            ScopedSocketAddr::from_socket_addr(addr).unwrap(),
            vec!["ls"],
        )
        .await
        .unwrap();
        let actual_args: Vec<_> = result.as_std().get_args().map(|a| a.to_string_lossy()).collect();
        let mut expected_args: Vec<String> = vec!["-F".into(), "none".into()];
        expected_args.extend(config.to_args());
        expected_args.extend(
            [
                "-i",
                key.to_str().expect("valid path"),
                "-o",
                "AddressFamily=inet6",
                "-p",
                "8022",
                "fe80::12%lo",
                "ls",
            ]
            .map(String::from),
        );
        assert_eq!(actual_args, expected_args);
    }

    #[fuchsia::test]
    async fn test_apply_auth_sock() {
        let mut builder = ffx_config::test_env();
        let expect_path =
            builder.isolate_root().join("ssh-auth.sock").to_string_lossy().to_string();

        let env = builder.user_config("ssh.auth-sock", expect_path.clone()).build().unwrap();

        let mut cmd = Command::new("env");
        apply_auth_sock(&mut cmd, &env.context);
        let lines = cmd
            .output()
            .await
            .unwrap()
            .stdout
            .lines()
            .filter_map(|res| res.ok())
            .collect::<Vec<_>>();

        let expected_var = format!("SSH_AUTH_SOCK={}", expect_path);
        assert!(
            lines.iter().any(|line| line.starts_with(&expected_var)),
            "Looking for {} in {}",
            expected_var,
            lines.join("\n")
        );
    }

    #[fuchsia::test]
    async fn test_build_ssh_command_with_ssh_config() {
        let builder = ffx_config::test_env();
        let (builder, _key) = init_ssh_key(builder).unwrap();
        let env = builder.build().unwrap();
        let mut config = SshConfig::new().expect("default ssh config");
        let addr: SocketAddr = "[fe80::12]:8022".parse().unwrap();

        // Override some options
        config.set("LogLevel", "DEBUG3").expect("setting loglevel");

        let result = build_ssh_command_with_ssh_config(
            "ssh",
            ScopedSocketAddr::from_socket_addr(addr).unwrap(),
            &mut config,
            vec!["ls"],
            &env.context,
        )
        .await
        .unwrap();
        let actual_args: Vec<_> =
            result.as_std().get_args().map(|a| a.to_string_lossy().to_string()).collect();

        // Check the default
        assert_eq!(config.get("CheckHostIP").expect("CheckHostIP value").unwrap(), "no");
        assert!(actual_args.contains(&"CheckHostIP=no".to_string()));

        // Check the override
        assert!(actual_args.contains(&"LogLevel=DEBUG3".to_string()));
    }

    #[fuchsia::test]
    fn test_host_pipe_err_from_str() {
        assert_eq!(SshError::from("Permission denied"), SshError::PermissionDenied);
        assert_eq!(SshError::from("Connection refused"), SshError::ConnectionRefused);
        assert_eq!(SshError::from("Name or service not known"), SshError::UnknownNameOrService);
        assert_eq!(SshError::from("Connection timed out"), SshError::Timeout);
        assert_eq!(
            SshError::from("Host key verification failedddddd"),
            SshError::KeyVerificationFailure
        );
        assert_eq!(SshError::from("There is No route to host"), SshError::NoRouteToHost);
        assert_eq!(SshError::from("The Network is unreachable"), SshError::NetworkUnreachable);
        assert_eq!(SshError::from("Invalid argument"), SshError::InvalidArgument);
        assert_eq!(SshError::from("ABI 123 is not compatible"), SshError::TargetIncompatible);

        let unknown_str = "OIHWOFIHOIWHFW";
        assert_eq!(SshError::from(unknown_str), SshError::Unknown(String::from(unknown_str)));
    }

    #[test]
    fn test_get_addr_port_ipv4() {
        let addr: SocketAddr = "192.168.0.1:8022".parse().unwrap();
        let scoped_addr = ScopedSocketAddr::from_socket_addr(addr).unwrap();
        let (addr, port) = get_addr_port(&scoped_addr);
        assert_eq!(addr, "192.168.0.1");
        assert_eq!(port, "8022");
    }

    #[test]
    fn test_get_addr_port_ipv6() {
        let addr: SocketAddr = "[fe80::12%1]:8022".parse().unwrap();
        let scoped_addr = ScopedSocketAddr::from_socket_addr(addr).unwrap();
        let (addr, port) = get_addr_port(&scoped_addr);
        assert_eq!(addr, "fe80::12%lo");
        assert_eq!(port, "8022");
    }

    #[test]
    fn test_get_addr_port_ipv6_no_scope() {
        let addr: SocketAddr = "[fe80::12]:22".parse().unwrap();
        let scoped_addr = ScopedSocketAddr::from_socket_addr(addr).unwrap();
        let (addr, port) = get_addr_port(&scoped_addr);
        assert_eq!(addr, "fe80::12");
        assert_eq!(port, "22");
    }

    #[test]
    fn test_controlmaster_mode_try_from() {
        assert!(matches!(
            ControlMasterMode::try_from("explicit".to_string()).unwrap(),
            ControlMasterMode::Explicit
        ));
        assert!(matches!(
            ControlMasterMode::try_from("managed".to_string()).unwrap(),
            ControlMasterMode::Managed
        ));
        assert!(matches!(
            ControlMasterMode::try_from("none".to_string()).unwrap(),
            ControlMasterMode::None
        ));

        let result = ControlMasterMode::try_from("invalid-mode".to_string());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(format!("{}", err), "Invalid ssh ControlMaster mode: invalid-mode");
    }

    #[fuchsia::test]
    async fn test_get_controlmaster_path_mode_none() {
        let env =
            ffx_config::test_env().user_config("ssh.controlmaster.mode", "none").build().unwrap();

        let addr: SocketAddr = "127.0.0.1:22".parse().unwrap();
        let scoped_addr = ScopedSocketAddr::from_socket_addr(addr).unwrap();
        let ssh_keys = vec!["key".to_string()];

        let res = get_controlmaster_path(
            &env.context,
            "ssh",
            &scoped_addr,
            &ssh_keys,
            &SshConfig::default(),
        )
        .await
        .unwrap();
        assert_eq!(res, None);
    }

    #[fuchsia::test]
    async fn test_get_controlmaster_path_mode_explicit() {
        let expected_path = "/tmp/test_socket";
        let env = ffx_config::test_env()
            .user_config("ssh.controlmaster.mode", "explicit")
            .user_config("ssh.controlmaster.path", expected_path)
            .build()
            .unwrap();

        let addr: SocketAddr = "127.0.0.1:22".parse().unwrap();
        let scoped_addr = ScopedSocketAddr::from_socket_addr(addr).unwrap();
        let ssh_keys = vec!["key".to_string()];

        let res = get_controlmaster_path(
            &env.context,
            "ssh",
            &scoped_addr,
            &ssh_keys,
            &SshConfig::default(),
        )
        .await
        .unwrap();
        assert_eq!(res, Some(PathBuf::from(expected_path)));
    }

    #[fuchsia::test]
    async fn test_get_controlmaster_path_mode_explicit_no_path() {
        let env = ffx_config::test_env()
            .user_config("ssh.controlmaster.mode", "explicit")
            .build()
            .unwrap();

        let addr: SocketAddr = "127.0.0.1:22".parse().unwrap();
        let scoped_addr = ScopedSocketAddr::from_socket_addr(addr).unwrap();
        let ssh_keys = vec!["key".to_string()];

        let res = get_controlmaster_path(
            &env.context,
            "ssh",
            &scoped_addr,
            &ssh_keys,
            &SshConfig::default(),
        )
        .await;
        assert!(matches!(res, Err(ManageSshControlMasterError::ControlMasterPathNotSpecified)));
    }

    #[fuchsia::test]
    #[ignore = "dir is compiled in so cannot be unset"]
    async fn test_get_controlmaster_path_mode_managed_no_dir() {
        let env = ffx_config::test_env()
            .user_config("ssh.controlmaster.mode", "managed")
            .build()
            .unwrap();

        let addr: SocketAddr = "127.0.0.1:22".parse().unwrap();
        let scoped_addr = ScopedSocketAddr::from_socket_addr(addr).unwrap();
        let ssh_keys = vec!["key".to_string()];

        let res = get_controlmaster_path(
            &env.context,
            "ssh",
            &scoped_addr,
            &ssh_keys,
            &SshConfig::default(),
        )
        .await;
        assert!(matches!(res, Err(ManageSshControlMasterError::ControlMasterDirNotSpecified)));
    }

    #[fuchsia::test]
    async fn test_get_controlmaster_path_socket_path_too_long() {
        let long_dir = "a".repeat(MAX_SOCKET_LEN);
        let env = ffx_config::test_env()
            .user_config("ssh.controlmaster.mode", "managed")
            .user_config("ssh.controlmaster.dir", long_dir)
            .build()
            .unwrap();

        let addr: SocketAddr = "127.0.0.1:22".parse().unwrap();
        let scoped_addr = ScopedSocketAddr::from_socket_addr(addr).unwrap();
        let ssh_keys = vec!["key".to_string()];

        let res = get_controlmaster_path(
            &env.context,
            "ssh",
            &scoped_addr,
            &ssh_keys,
            &SshConfig::default(),
        )
        .await;
        assert!(matches!(
            res,
            Err(ManageSshControlMasterError::SpawnError(
                SpawnControlMasterError::SocketPathTooLong { .. }
            ))
        ));
    }
}
