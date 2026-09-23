// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use fho::{FhoEnvironment, TryFromEnv};
use std::net::SocketAddr;
use std::ops::Deref;
use target_behavior::{ConnectionBehavior, target_interface};

/// Holder struct for the target's SshAddr.
#[derive(Debug, Clone)]
pub struct SshAddrHolder(SocketAddr);

#[async_trait(?Send)]
impl TryFromEnv for SshAddrHolder {
    type Error = ffx_command_error::Error;
    async fn try_from_env(env: &FhoEnvironment) -> std::result::Result<Self, Self::Error> {
        let target_env = target_interface(env);
        let behavior = target_env.init_connection_behavior(env.environment_context()).await?;
        let ConnectionBehavior::Direct(ref dc) = *behavior;
        let addr = dc
            .resolution()
            .await
            .map_err(|e| e.into_command_error())?
            .addr()
            .map_err(|e| e.into_command_error())?;
        Ok(SshAddrHolder(addr))
    }
}

impl Deref for SshAddrHolder {
    type Target = SocketAddr;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
