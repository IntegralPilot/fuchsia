// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use addr::TargetIpAddr;
use anyhow::Result;
use async_trait::async_trait;
use ffx_config::EnvironmentContext;
use ffx_config::environment::EnvironmentKind;
use ffx_ssh::ssh::{build_ssh_command, build_ssh_command_with_config_file};
use ffx_target_ssh_args::SshCommand;
use ffx_writer::SimpleWriter;
use fho::{FfxContext, FfxMain, FfxTool};
use std::net::IpAddr;
use std::path::PathBuf;
use target_holders::SshAddrHolder;
use tokio::process::Command;

#[derive(FfxTool)]
pub struct SshTool {
    #[command]
    cmd: SshCommand,
    ssh_addr: SshAddrHolder,
    context: EnvironmentContext,
}

fho::embedded_plugin!(SshTool);

#[async_trait(?Send)]
impl FfxMain for SshTool {
    // This command doesn't actually produce any normal output itself.
    type Writer = SimpleWriter;
    type Error = ::fho::Error;

    async fn main(self, _writer: Self::Writer) -> fho::Result<()> {
        let addr = get_addr(&self.context, (*self.ssh_addr).into())?;
        let mut ssh_cmd = make_ssh_command(&self.context, self.cmd, addr)
            .await
            .bug_context("Building command to ssh to target")?;

        log::debug!("About to ssh with command: {:#?}", ssh_cmd);
        let mut ssh = ssh_cmd.spawn().user_message("Failed to run ssh command to target")?;
        let status = ssh.wait().await.user_message("Command 'ssh' exited with error.")?;

        // We want to give callers of this command a meaningful error code if it fails. First,
        // check if it was terminated due to a signal and report that. If the process terminated
        // normally, exit this process with its status code.
        #[cfg(unix)]
        {
            use nix::sys::signal::Signal;
            use std::os::unix::process::ExitStatusExt;
            if let Some(signal) = status.signal() {
                // ssh exited due to receiving a signal, try to parse it for pretty-printing
                let signal = Signal::try_from(signal)
                    .with_bug_context(|| format!("parsing raw ssh signal code {signal}"))?;
                return Err(fho::Error::User(anyhow::format_err!(
                    "ssh process received {signal} signal."
                )));
            }
        }

        if let Some(status) = status.code() {
            match status {
                0 => Ok(()),
                i => {
                    ffx_command::exit_with_code!(i)
                }
            }
        } else {
            return Err(fho::Error::Unexpected(anyhow::format_err!(
                "ssh exited without an exit status or due to receiving a signal."
            )));
        }
    }
}

/// If we're running in an isolated environment against an emulator with user networking,
/// pretend we're connecting to localhost because user networking is implemented as a port on the
/// host machine.
// TODO(https://fxbug.dev/42077822) this should not be required
fn get_addr(ctx: &EnvironmentContext, addr: TargetIpAddr) -> fho::Result<TargetIpAddr> {
    fn is_in_ipv4_10_net(ip: IpAddr) -> bool {
        match ip {
            IpAddr::V4(ipv4_addr) => ipv4_addr.octets()[0] == 10,
            IpAddr::V6(_) => false,
        }
    }
    Ok(match (ctx.env_kind(), addr.ip()) {
        // we only care about 10.* addresses if we're in an isolated environment
        (EnvironmentKind::Isolated { .. }, ip) if is_in_ipv4_10_net(ip) => {
            format!("127.0.0.1:{}", addr.port())
                .parse()
                .bug_context("Parsing localhost ssh address")?
        }
        _ => addr,
    })
}

async fn make_ssh_command(
    context: &EnvironmentContext,
    cmd: SshCommand,
    addr: TargetIpAddr,
) -> Result<Command> {
    let addr = netext::ScopedSocketAddr::from_socket_addr(addr.into())?;
    let ssh_cmd = if let Some(config_file) = cmd.sshconfig {
        build_ssh_command_with_config_file(
            &PathBuf::from(config_file),
            addr,
            cmd.command.iter().map(|s| s.as_str()).collect(),
            context,
        )?
    } else {
        build_ssh_command(context, addr, cmd.command.iter().map(|s| s.as_str()).collect()).await?
    };

    Ok(ssh_cmd)
}

#[cfg(test)]
mod test {
    use super::*;

    use itertools::Itertools;
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use std::fs;
    use std::str::FromStr;

    #[fuchsia::test]
    async fn test_address_family() -> Result<()> {
        let env1 = ffx_config::test_env().build().unwrap();
        let keys = [env1.isolate_root.path().join("path2"), env1.isolate_root.path().join("path1")];
        for k in &keys {
            fs::write(k, "")?;
        }
        let test_env =
            ffx_config::test_env().user_config("ssh.priv", json!(&keys)).build().unwrap();
        let addr = TargetIpAddr::from_str("127.0.0.1:34522")?;
        let cmd = SshCommand { sshconfig: None, command: vec![] };
        let ssh_cmd = make_ssh_command(&test_env.context, cmd, addr).await?;
        assert_eq!(ssh_cmd.as_std().get_program(), "ssh");

        // assert that the keys are added,
        // the address family is ipv4
        // the port and address are correct.
        // ignore the configuration since that is tested in the make_ssh_command, and
        // could change over time.
        let actual_command = ssh_cmd.as_std().get_args().filter_map(|a| a.to_str()).join(" ");
        assert!(actual_command.contains("-o AddressFamily=inet"));
        let key_args =
            format!("-i {} -i {}", &keys[0].to_string_lossy(), &keys[1].to_string_lossy());
        assert!(
            actual_command.contains(&key_args),
            "missing ssh keys: {actual_command} does not contain {key_args}"
        );
        assert!(actual_command.ends_with("-p 34522 127.0.0.1"));

        Ok(())
    }

    #[fuchsia::test]
    async fn test_custom_config_file() -> Result<()> {
        let env1 = ffx_config::test_env().build().unwrap();
        let keys = [env1.isolate_root.path().join("path2"), env1.isolate_root.path().join("path1")];
        for k in &keys {
            fs::write(k, "")?;
        }
        let test_env = ffx_config::test_env().user_config("ssh.priv", json!(keys)).build().unwrap();
        let addr = TargetIpAddr::from_str("[fe80::1%1]:22")?;
        let cmd = SshCommand {
            sshconfig: Some("/foo/bar/baz.conf".to_string()),
            command: vec!["echo".to_string(), "'foo'".to_string()],
        };
        let ssh_cmd = make_ssh_command(&test_env.context, cmd, addr).await?;
        assert_eq!(ssh_cmd.as_std().get_program(), "ssh");
        assert_eq!(
            ssh_cmd.as_std().get_args().map(|a| a.to_str().unwrap()).collect::<Vec<&str>>(),
            vec![
                "-F",
                "/foo/bar/baz.conf",
                "-i",
                &keys[0].to_string_lossy(),
                "-i",
                &keys[1].to_string_lossy(),
                "-p",
                "22",
                "fe80::1%lo",
                "echo",
                "'foo'",
            ]
        );
        Ok(())
    }
}
