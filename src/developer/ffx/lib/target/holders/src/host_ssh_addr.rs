// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::ops::Deref;

use async_trait::async_trait;

use ffx_ssh::parse::HostAddr;
use fho::{FhoEnvironment, TryFromEnv};
use fidl_fuchsia_developer_ffx as ffx_fidl;
use target_behavior::{ConnectionBehavior, target_interface};

#[derive(Clone, Debug)]
pub struct HostAddrHolder(Option<HostAddr>);

impl Deref for HostAddrHolder {
    type Target = Option<HostAddr>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<HostAddr> for HostAddrHolder {
    fn from(value: HostAddr) -> Self {
        HostAddrHolder::from(Some(value))
    }
}

impl From<Option<HostAddr>> for HostAddrHolder {
    fn from(value: Option<HostAddr>) -> Self {
        HostAddrHolder(value)
    }
}

impl From<HostAddrHolder> for Option<HostAddr> {
    fn from(value: HostAddrHolder) -> Self {
        value.0
    }
}

impl From<Option<ffx_fidl::SshHostAddrInfo>> for HostAddrHolder {
    fn from(value: Option<ffx_fidl::SshHostAddrInfo>) -> Self {
        HostAddrHolder::from(value.map(|x| HostAddr::from(x.address)))
    }
}

impl From<String> for HostAddrHolder {
    fn from(value: String) -> Self {
        HostAddrHolder::from(Some(HostAddr::from(value)))
    }
}

#[async_trait(?Send)]
impl TryFromEnv for HostAddrHolder {
    type Error = ffx_command_error::Error;
    async fn try_from_env(env: &FhoEnvironment) -> std::result::Result<Self, Self::Error> {
        let target_env = target_interface(env);
        let behavior = target_env.init_connection_behavior(env.environment_context()).await?;
        let ConnectionBehavior::Direct(ref dc) = *behavior;
        let resolution = dc.resolution().await.map_err(|e| e.into_command_error())?;
        let conn = resolution
            .get_connection(env.environment_context())
            .await
            .map_err(|e| e.into_command_error())?;
        let host_addr_info = match conn.host_ssh_address() {
            Some(addr) => Some(addr),
            None => {
                if let Ok(target_addr) = resolution.addr() {
                    let ssh_path: String = env
                        .environment_context()
                        .get("ssh.path")
                        .unwrap_or_else(|_| "ssh".to_string());
                    if let Ok(scoped) = netext::ScopedSocketAddr::from_socket_addr(target_addr) {
                        ffx_ssh::ssh::get_ssh_host_address(
                            &ssh_path,
                            scoped,
                            env.environment_context(),
                        )
                        .await
                        .ok()
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
        };
        Ok(HostAddrHolder::from(host_addr_info))
    }
}
