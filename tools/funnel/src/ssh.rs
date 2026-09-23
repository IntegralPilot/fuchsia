// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::errors::IntoExitCode;
use crate::target::TargetMode;
use crate::TargetInfo;
use addr::TargetAddr;
use anyhow::Result;
use camino::{Utf8Path, Utf8PathBuf};
use home::home_dir;
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use thiserror::Error;

const CONTROL_MASTER_PATH: &str = ".ssh/control/funnel_control_master";

const CLEANUP_COMMAND: &'static str = include_str!("cleanup_command");

#[derive(Debug, Error)]
pub enum TunnelError {
    #[error("Target {target} did not have any valid Ip Addresses associated with it.")]
    NoAddressesError { target: String },
    #[error(
        "Failed to forward a port to remote host {remote_host}.\n\
        There may already be a tunnel running. You can try:\n\n\
        `funnel cleanup-remote-host {remote_host}` on local\n\n\
        If you see `Error: remote port forwarding failed for listen port $PORT` you can try:\n\n\
        `netstat -tulpne | grep :$PORT` on the remote host to find if a process has already \
        taken the port."
    )]
    PortForwardingFailed { remote_host: String },
    #[error("Ssh was terminated by a signal")]
    SshTerminatedFromSignal,
    #[error("User's home dir is not valid UTF8: {home_dir}")]
    InvalidHomeDir { home_dir: PathBuf },
    #[error("User does not have a Home Dir")]
    NoHomeDir,
    #[error("Control master path cannot be at root")]
    ControlMasterPathCannotBeAtRoot,
    #[error("Could not create path to control master: {source}")]
    CannotCreateControlMasterPath { source: std::io::Error },
    #[error("Ssh Did not start: {0}")]
    SshDidNotStart(#[from] std::io::Error),
    #[error("unknown error code during ssh: {0}")]
    SshError(i32),
}

impl IntoExitCode for TunnelError {
    fn exit_code(&self) -> i32 {
        match self {
            Self::NoAddressesError { target: _ } => 31,
            Self::PortForwardingFailed { remote_host: _ } => 32,
            Self::SshTerminatedFromSignal => 128,
            Self::InvalidHomeDir { home_dir: _ } => 33,
            Self::NoHomeDir => 34,
            Self::ControlMasterPathCannotBeAtRoot => 35,
            Self::CannotCreateControlMasterPath { source } => source.raw_os_error().unwrap_or(36),
            Self::SshDidNotStart(e) => e.raw_os_error().unwrap_or(37),
            Self::SshError(i) => *i,
        }
    }
}

fn get_control_path() -> Result<Utf8PathBuf, TunnelError> {
    let home_path = home_dir().ok_or(TunnelError::NoHomeDir)?;
    let home_path = Utf8PathBuf::from_path_buf(home_path)
        .map_err(|e| TunnelError::InvalidHomeDir { home_dir: e })?;
    let funnel_control_path = home_path.join(CONTROL_MASTER_PATH);
    let funnel_control_path_parent =
        funnel_control_path.parent().ok_or(TunnelError::ControlMasterPathCannotBeAtRoot)?;
    match funnel_control_path_parent.try_exists() {
        Ok(true) => {}
        Ok(false) => std::fs::create_dir_all(funnel_control_path_parent)
            .map_err(|e| TunnelError::CannotCreateControlMasterPath { source: e })?,
        Err(e) => return Err(TunnelError::CannotCreateControlMasterPath { source: e }),
    };
    Ok(funnel_control_path)
}

pub(crate) async fn do_ssh<W: Write>(
    writer: W,
    host: String,
    target: TargetInfo,
    repo_port: Vec<u32>,
    additional_port_forwards: Vec<u32>,
) -> Result<(), TunnelError> {
    // Set up the control master
    let funnel_control_path = get_control_path()?;
    // Set up ssh command
    let mut ssh_cmd = &mut Command::new("ssh");
    for arg in
        build_ssh_args(writer, target, funnel_control_path, repo_port, additional_port_forwards)?
            .iter()
    {
        ssh_cmd = ssh_cmd.arg(arg);
    }
    ssh_cmd = ssh_cmd.arg(host.clone());
    ssh_cmd = ssh_cmd.arg(format!(r#"echo 'Tunnel established';echo 'Do not forget to run `ffx target add [::1]:8022`'; sleep infinity"#));

    // Disable stdin
    ssh_cmd = ssh_cmd.stdin(Stdio::null());
    // Use the PID as the process group so that SIGINT doesnt get forwarded
    ssh_cmd = ssh_cmd.process_group(0);

    // Spawn
    log::debug!("About to ssh with command: {:#?}", ssh_cmd);
    let mut ssh = ssh_cmd.spawn()?;
    match ssh.wait() {
        Ok(e) => match e.code() {
            None => Err(TunnelError::SshTerminatedFromSignal),
            Some(255) => Err(TunnelError::PortForwardingFailed { remote_host: host }),
            Some(0) => Ok(()),
            Some(i) => Err(TunnelError::SshError(i)),
        },
        Err(e) => Err(TunnelError::SshDidNotStart(e)),
    }
}

fn build_ssh_args<W: Write>(
    mut writer: W,
    target: TargetInfo,
    control_master_path: impl AsRef<Utf8Path>,
    repo_port: Vec<u32>,
    additional_port_forwards: Vec<u32>,
) -> Result<Vec<String>, TunnelError> {
    let mut addrs: Vec<TargetAddr> = target.addresses.into_iter().collect::<Vec<TargetAddr>>();
    log::debug!("Discovered addresses for target: {:?}", addrs);
    // Flip the sorting so that Ipv6 comes before Ipv4 as we will take the first
    // address, and (generally) Ipv4 addresses from the Target are ephemeral
    addrs.sort_by(|a, b| b.cmp(a));

    let target_ip = addrs
        .first()
        .ok_or_else(|| TunnelError::NoAddressesError { target: target.nodename.clone() })?;

    if addrs.len() > 1 {
        log::warn!(
            "Target: {} has {} addresses associated with it: {:?}. Choosing the first one: {}",
            target.nodename,
            addrs.len(),
            addrs,
            target_ip
        );
    }

    let mut remote_forwards = vec![
        // Requests from the remote to ssh to localhost:8022 will be forwarded to the
        // target.
        (8022, 22, "SSH and Overnet"),
        // zxdb & fidlcat requests from the remote to 2345 are forwarded to the target.
        (2345, 2345, "zxdb and fidlcat"),
        // libassistant debug requests from the remote to 8007 are forwarded to the
        // target.
        (8007, 8007, "libassistant"),
        (8008, 8008, "libassistant"),
        (8443, 8443, "libassistant"),
        // SL4F requests to port 9080 on the remote are forwarded to target port 80.
        (9080, 80, "SL4F"),
        // UMA log requests to port 8888 on the remote are forwarded to target port 8888.
        (8888, 8888, "UMA Log"),
        (5555, 5555, "Android ADB"),
    ];

    if target.mode == TargetMode::Fastboot {
        // Some targets use Fastboot over TCP which listens on 5554
        remote_forwards.push((5554, 5554, "Fastboot over TCP"));
    }

    let mut res: Vec<String> = vec![
        // We want all binds for the port forwards, since the device may be using IPv4 or IPv6, and the server may be using an IPv6 address to look it up.
        "-o AddressFamily any".into(),
        // We do not want multiplexing
        format!("-o ControlPath {}", control_master_path.as_ref()),
        "-o ControlMaster auto".into(),
        // Disable pseudo-tty allocation for screen based programs over the SSH tunnel.
        "-o RequestTTY no".into(),
        "-o ExitOnForwardFailure yes".into(),
        "-o StreamLocalBindUnlink yes".into(),
    ];

    let remote_forward_spec = remote_forwards
        .clone()
        .into_iter()
        .map(|pair| format!("-o RemoteForward {0} [{target_ip}]:{1}", pair.0, pair.1));
    res.extend(remote_forward_spec);

    let additional_forwards = additional_port_forwards
        .clone()
        .into_iter()
        .map(|p| format!("-o RemoteForward {p} [{target_ip}]:{p}"));
    res.extend(additional_forwards);

    let repo_forwards =
        repo_port.clone().into_iter().map(|p| format!("-o LocalForward *:{p} localhost:{p}"));
    res.extend(repo_forwards);

    writeln!(writer, "Setting up ssh forwards like so:")?;
    for spec in remote_forwards {
        writeln!(
            writer,
            "  * Remote Host port {0} --> Target port {1} [ {2} ]",
            spec.0, spec.1, spec.2
        )?;
    }
    for forward in additional_port_forwards {
        writeln!(
            writer,
            "  * Remote Host port {0} --> Target port {0}   [ User Defined ]",
            forward
        )?;
    }
    for forward in repo_port {
        writeln!(writer, "  * Local Host port {0} --> Cloudtop port {0} [ Repository ]", forward)?;
    }

    Ok(res)
}

pub(crate) async fn cleanup_remote_sshd(host: String) -> Result<()> {
    let mut ssh =
        Command::new("ssh").arg("-o RequestTTY yes").arg(host).arg(CLEANUP_COMMAND).spawn()?;
    ssh.wait()?;

    Ok(())
}

#[derive(Debug, Error)]
pub enum CloseExistingTunnelError {
    #[error("{}", .0)]
    Error(#[from] anyhow::Error),
    #[error("{}", .0)]
    TunnelError(#[from] TunnelError),
    #[error("{}", .0)]
    SshError(#[from] std::io::Error),
}

impl IntoExitCode for CloseExistingTunnelError {
    fn exit_code(&self) -> i32 {
        match self {
            Self::Error(_) => 1,
            Self::TunnelError(e) => e.exit_code(),
            Self::SshError(e) => e.raw_os_error().unwrap_or(1),
        }
    }
}

pub(crate) fn close_existing_tunnel(host: String) -> Result<(), CloseExistingTunnelError> {
    let funnel_control_path = get_control_path()?;
    let args = vec![
        "-o".to_string(),
        format!("ControlPath {}", funnel_control_path),
        "-O".to_string(),
        "exit".to_string(),
        host,
    ];
    log::info!("executing ssh command with args: {:?}", args);
    let mut ssh = Command::new("ssh").args(args).spawn()?;
    ssh.wait()?;
    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;
    use fidl_fuchsia_developer_ffx::{TargetAddrInfo, TargetIp};
    use fidl_fuchsia_net::{IpAddress, Ipv4Address, Ipv6Address};
    use pretty_assertions::assert_eq;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn test_make_args() -> Result<()> {
        let src = TargetAddrInfo::Ip(TargetIp {
            ip: IpAddress::Ipv6(Ipv6Address {
                addr: Ipv6Addr::new(0xff00, 0, 0, 0, 0, 0, 0, 0).octets(),
            }),
            scope_id: 2,
        });

        let src_ipv4 = TargetAddrInfo::Ip(TargetIp {
            ip: IpAddress::Ipv4(Ipv4Address { addr: Ipv4Addr::new(127, 0, 0, 1).octets() }),
            scope_id: 0,
        });

        let target = TargetInfo {
            nodename: "kiriona".to_string(),
            addresses: vec![src_ipv4.into(), src.into()],
            mode: TargetMode::Product,
        };

        let mut writer = vec![];
        let got = build_ssh_args(&mut writer, target, "/foo", vec![8081], vec![5556])?;

        let want: Vec<&str> = vec![
            "-o AddressFamily any",
            "-o ControlPath /foo",
            "-o ControlMaster auto",
            "-o RequestTTY no",
            "-o ExitOnForwardFailure yes",
            "-o StreamLocalBindUnlink yes",
            "-o RemoteForward 8022 [ff00::]:22",
            "-o RemoteForward 2345 [ff00::]:2345",
            "-o RemoteForward 8007 [ff00::]:8007",
            "-o RemoteForward 8008 [ff00::]:8008",
            "-o RemoteForward 8443 [ff00::]:8443",
            "-o RemoteForward 9080 [ff00::]:80",
            "-o RemoteForward 8888 [ff00::]:8888",
            "-o RemoteForward 5555 [ff00::]:5555",
            "-o RemoteForward 5556 [ff00::]:5556",
            "-o LocalForward *:8081 localhost:8081",
        ];

        assert_eq!(got, want);

        assert_eq!(
            String::from_utf8(writer).unwrap(),
            r#"Setting up ssh forwards like so:
  * Remote Host port 8022 --> Target port 22 [ SSH and Overnet ]
  * Remote Host port 2345 --> Target port 2345 [ zxdb and fidlcat ]
  * Remote Host port 8007 --> Target port 8007 [ libassistant ]
  * Remote Host port 8008 --> Target port 8008 [ libassistant ]
  * Remote Host port 8443 --> Target port 8443 [ libassistant ]
  * Remote Host port 9080 --> Target port 80 [ SL4F ]
  * Remote Host port 8888 --> Target port 8888 [ UMA Log ]
  * Remote Host port 5555 --> Target port 5555 [ Android ADB ]
  * Remote Host port 5556 --> Target port 5556   [ User Defined ]
  * Local Host port 8081 --> Cloudtop port 8081 [ Repository ]
"#
            .to_string()
        );
        Ok(())
    }

    #[test]
    fn test_make_args_fastboot() -> Result<()> {
        let src = TargetAddrInfo::Ip(TargetIp {
            ip: IpAddress::Ipv6(Ipv6Address {
                addr: Ipv6Addr::new(0xff00, 0, 0, 0, 0, 0, 0, 0).octets(),
            }),
            scope_id: 2,
        });

        let src_ipv4 = TargetAddrInfo::Ip(TargetIp {
            ip: IpAddress::Ipv4(Ipv4Address { addr: Ipv4Addr::new(127, 0, 0, 1).octets() }),
            scope_id: 0,
        });

        let target = TargetInfo {
            nodename: "kiriona".to_string(),
            addresses: vec![src_ipv4.into(), src.into()],
            mode: TargetMode::Fastboot,
        };

        let mut writer = vec![];
        let got = build_ssh_args(&mut writer, target, "/foo", vec![8081], vec![5556])?;

        let want: Vec<&str> = vec![
            "-o AddressFamily any",
            "-o ControlPath /foo",
            "-o ControlMaster auto",
            "-o RequestTTY no",
            "-o ExitOnForwardFailure yes",
            "-o StreamLocalBindUnlink yes",
            "-o RemoteForward 8022 [ff00::]:22",
            "-o RemoteForward 2345 [ff00::]:2345",
            "-o RemoteForward 8007 [ff00::]:8007",
            "-o RemoteForward 8008 [ff00::]:8008",
            "-o RemoteForward 8443 [ff00::]:8443",
            "-o RemoteForward 9080 [ff00::]:80",
            "-o RemoteForward 8888 [ff00::]:8888",
            "-o RemoteForward 5555 [ff00::]:5555",
            "-o RemoteForward 5554 [ff00::]:5554",
            "-o RemoteForward 5556 [ff00::]:5556",
            "-o LocalForward *:8081 localhost:8081",
        ];

        assert_eq!(got, want);

        assert_eq!(
            String::from_utf8(writer).unwrap(),
            r#"Setting up ssh forwards like so:
  * Remote Host port 8022 --> Target port 22 [ SSH and Overnet ]
  * Remote Host port 2345 --> Target port 2345 [ zxdb and fidlcat ]
  * Remote Host port 8007 --> Target port 8007 [ libassistant ]
  * Remote Host port 8008 --> Target port 8008 [ libassistant ]
  * Remote Host port 8443 --> Target port 8443 [ libassistant ]
  * Remote Host port 9080 --> Target port 80 [ SL4F ]
  * Remote Host port 8888 --> Target port 8888 [ UMA Log ]
  * Remote Host port 5555 --> Target port 5555 [ Android ADB ]
  * Remote Host port 5554 --> Target port 5554 [ Fastboot over TCP ]
  * Remote Host port 5556 --> Target port 5556   [ User Defined ]
  * Local Host port 8081 --> Cloudtop port 8081 [ Repository ]
"#
            .to_string()
        );
        Ok(())
    }
    #[test]
    fn test_make_args_empty_nodename() -> Result<()> {
        let src = TargetAddrInfo::Ip(TargetIp {
            ip: IpAddress::Ipv6(Ipv6Address {
                addr: Ipv6Addr::new(0xff00, 0, 0, 0, 0, 0, 0, 0).octets(),
            }),
            scope_id: 2,
        });

        let target = TargetInfo {
            nodename: "ianthe".to_string(),
            addresses: vec![src.into()],
            ..Default::default()
        };

        let writer = vec![];
        let got = build_ssh_args(writer, target, "/foo", vec![8081], vec![])?;

        let want: Vec<&str> = vec![
            "-o AddressFamily any",
            "-o ControlPath /foo",
            "-o ControlMaster auto",
            "-o RequestTTY no",
            "-o ExitOnForwardFailure yes",
            "-o StreamLocalBindUnlink yes",
            "-o RemoteForward 8022 [ff00::]:22",
            "-o RemoteForward 2345 [ff00::]:2345",
            "-o RemoteForward 8007 [ff00::]:8007",
            "-o RemoteForward 8008 [ff00::]:8008",
            "-o RemoteForward 8443 [ff00::]:8443",
            "-o RemoteForward 9080 [ff00::]:80",
            "-o RemoteForward 8888 [ff00::]:8888",
            "-o RemoteForward 5555 [ff00::]:5555",
            "-o LocalForward *:8081 localhost:8081",
        ];

        assert_eq!(got, want);
        Ok(())
    }

    #[test]
    fn test_make_args_multiple_repos() -> Result<()> {
        let src = TargetAddrInfo::Ip(TargetIp {
            ip: IpAddress::Ipv6(Ipv6Address {
                addr: Ipv6Addr::new(0xff00, 0, 0, 0, 0, 0, 0, 0).octets(),
            }),
            scope_id: 2,
        });

        let target = TargetInfo {
            nodename: "ianthe".to_string(),
            addresses: vec![src.into()],
            ..Default::default()
        };

        let writer = vec![];
        let got = build_ssh_args(writer, target, "/foo", vec![8081, 8085], vec![])?;

        let want: Vec<&str> = vec![
            "-o AddressFamily any",
            "-o ControlPath /foo",
            "-o ControlMaster auto",
            "-o RequestTTY no",
            "-o ExitOnForwardFailure yes",
            "-o StreamLocalBindUnlink yes",
            "-o RemoteForward 8022 [ff00::]:22",
            "-o RemoteForward 2345 [ff00::]:2345",
            "-o RemoteForward 8007 [ff00::]:8007",
            "-o RemoteForward 8008 [ff00::]:8008",
            "-o RemoteForward 8443 [ff00::]:8443",
            "-o RemoteForward 9080 [ff00::]:80",
            "-o RemoteForward 8888 [ff00::]:8888",
            "-o RemoteForward 5555 [ff00::]:5555",
            "-o LocalForward *:8081 localhost:8081",
            "-o LocalForward *:8085 localhost:8085",
        ];

        assert_eq!(got, want);
        Ok(())
    }

    #[test]
    fn test_make_args_returns_err_on_no_addresses() {
        let nodename = "cytherea".to_string();
        {
            let target = TargetInfo { nodename: nodename.clone(), ..Default::default() };

            let writer = vec![];
            let res = build_ssh_args(writer, target, "/foo", vec![9091], vec![]);
            assert!(res.is_err());
        }
        {
            let target = TargetInfo { nodename: nodename.clone(), ..Default::default() };

            let writer = vec![];
            let res = build_ssh_args(writer, target, "/foo", vec![9091], vec![]);
            assert!(res.is_err());
        }
        {
            let target =
                TargetInfo { nodename: nodename.clone(), addresses: vec![], ..Default::default() };

            let writer = vec![];
            let res = build_ssh_args(writer, target, "/foo", vec![9091], vec![]);
            assert!(res.is_err());
        }
    }

    #[test]
    fn test_make_args_returns_err_on_empty_addresses() {
        let target = TargetInfo {
            nodename: "cytherera".to_string(),
            addresses: vec![],
            ..Default::default()
        };

        let writer = vec![];
        let res = build_ssh_args(writer, target, "/foo", vec![9091], vec![]);
        assert!(res.is_err());
    }
}
