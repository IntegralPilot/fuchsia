// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use fdomain_client::fidl::{DiscoverableProtocolMarker, Proxy};
use fdomain_fuchsia_developer_remotecontrol::RemoteControlProxy;
use ffx_command_error::{FfxContext as _, Result};
use fho::{FhoEnvironment, TryFromEnv};
use std::ops::Deref;
use std::sync::Arc;
use std::time::Duration;
use target_behavior::{ConnectionBehavior, target_interface};

#[derive(Clone, Debug)]
pub struct RemoteControlProxyHolder(RemoteControlProxy);

impl Deref for RemoteControlProxyHolder {
    type Target = RemoteControlProxy;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<RemoteControlProxy> for RemoteControlProxyHolder {
    fn from(value: RemoteControlProxy) -> Self {
        RemoteControlProxyHolder(value)
    }
}

#[async_trait(?Send)]
impl TryFromEnv for RemoteControlProxyHolder {
    type Error = ffx_command_error::Error;
    async fn try_from_env(env: &FhoEnvironment) -> std::result::Result<Self, Self::Error> {
        let target_env = target_interface(env);
        let behavior = target_env.init_connection_behavior(env.environment_context()).await?;
        let ConnectionBehavior::Direct(dc) = &*behavior;
        let conn = dc
            .resolution()
            .await
            .map_err(|e| e.into_command_error())?
            .get_connection(env.environment_context())
            .await
            .map_err(|e| e.into_command_error())?;
        conn.rcs_proxy_fdomain().await.bug().map(Into::into).map_err(Into::into)
    }
}

pub async fn open_moniker<P>(
    rcs: &RemoteControlProxy,
    capability_set: rcs::OpenDirType,
    moniker: &str,
    timeout: Duration,
) -> Result<P>
where
    P: Proxy + 'static,
    P::Protocol: DiscoverableProtocolMarker,
{
    rcs::open_with_timeout::<P::Protocol>(
        timeout,
        moniker,
        capability_set,
        rcs,
    )
    .await
    .with_user_message(|| {
        let protocol_name = P::Protocol::PROTOCOL_NAME;
        format!("Failed to connect to protocol '{protocol_name}' at moniker '{moniker}' within {} seconds", timeout.as_secs_f64())
    })
}

/// Sets up a fake FDomain proxy of type `T` handing requests to the given
/// callback and returning their responses.
///
/// This is basically the same thing as `ffx_plugin` used to generate for
/// each proxy argument, but uses a generic instead of text replacement.
pub fn fake_proxy<T: fdomain_client::fidl::Proxy>(
    client: Arc<fdomain_client::Client>,
    mut handle_request: impl FnMut(fdomain_client::fidl::Request<T::Protocol>) + 'static,
) -> T {
    fake_async_proxy(client, async move |req| {
        handle_request(req);
    })
}

/// Sets up a fake FDomain proxy of type `T` handing requests to the given
/// callback and returning their responses.
///
/// This is basically the same thing as `ffx_plugin` used to generate for
/// each proxy argument, but uses a generic instead of text replacement.
pub fn fake_async_proxy<T: fdomain_client::fidl::Proxy>(
    client: Arc<fdomain_client::Client>,
    mut handle_request: impl AsyncFnMut(fdomain_client::fidl::Request<T::Protocol>) + 'static,
) -> T {
    use futures::TryStreamExt;
    let (proxy, mut stream) = client.create_proxy_and_stream::<T::Protocol>();
    fuchsia_async::Task::local(async move {
        // Capture the client so it doesn't go out of scope
        let _client = client;
        while let Ok(Some(req)) = stream.try_next().await {
            handle_request(req).await;
        }
    })
    .detach();
    proxy
}
