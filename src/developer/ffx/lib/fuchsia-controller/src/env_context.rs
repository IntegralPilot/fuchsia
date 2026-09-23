// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{LibContext, logging};
use anyhow::Result;
use async_lock::Mutex;
use camino::Utf8PathBuf;
use discovery::query::TargetInfoQuery;
use errors::ffx_error;
use fdomain_client::HandleBased;
use fdomain_client::fidl::Proxy;
use fdomain_fuchsia_device::ControllerMarker;
use ffx_config::EnvironmentContext;
use ffx_config::environment::ExecutableKind;
use ffx_target::connection::Connection;
use rcs;
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Weak};
use std::time::Duration;
use zx_types;

fn unspecified_target() -> anyhow::Error {
    anyhow::anyhow!(concat!(
        "no device has been specified for this `Context`. ",
        "A device must be specified in order to connect to the remote control proxy"
    ))
}

fn fxe<E: std::fmt::Debug>(e: E) -> anyhow::Error {
    ffx_error!("{e:?}").into()
}

#[derive(Debug)]
pub struct FfxConfigEntry {
    pub(crate) key: String,
    pub(crate) value: String,
}

pub struct EnvContext {
    lib_ctx: Weak<LibContext>,
    target_spec: TargetInfoQuery,
    device_connection: Mutex<Option<Arc<Connection>>>,
    pub(crate) context: EnvironmentContext,
}

async fn new_device_connection(
    ctx: &EnvironmentContext,
    target_spec: &TargetInfoQuery,
) -> Result<Arc<Connection>> {
    // We pass use_cache=false because in Fuchsia Controller, we don't want to
    // scripts to use potentially stale cache data, and the caller can make sure
    // to pass an address directly if they don't want to wait for discovery.
    let resolution = ffx_target::resolve_target_address(target_spec, false, ctx).await?;
    resolution.get_connection(ctx).await.map_err(anyhow::Error::from)
}

impl EnvContext {
    pub(crate) fn write_err<T: std::fmt::Display>(&self, thread_id: std::thread::ThreadId, err: T) {
        let lib = self.lib_ctx.upgrade().expect("library context instance deallocated early");
        lib.write_err(thread_id, err)
    }

    pub(crate) fn lib_ctx(&self) -> Arc<LibContext> {
        self.lib_ctx.upgrade().expect("library context instance deallocated early")
    }

    pub fn new(
        lib_ctx: Weak<LibContext>,
        config: Vec<FfxConfigEntry>,
        isolate_dir: Option<PathBuf>,
    ) -> Result<Self> {
        // TODO(https://fxbug.dev/42079638): This is a lot of potentially unnecessary data transformation
        // going through several layers of structured into unstructured and then back to structured
        // again. Likely the solution here is to update the input of the config runtime population
        // to accept structured data.
        let formatted_config = config
            .iter()
            .map(|entry| format!("{}={}", entry.key, entry.value))
            .collect::<Vec<String>>()
            .join(",");
        let runtime_config =
            if formatted_config.is_empty() { None } else { Some(formatted_config) };
        let runtime_args = ffx_config::runtime::populate_runtime(&[], runtime_config)?;
        let env_path = None;
        let current_dir = std::env::current_dir()?;
        let context = match isolate_dir {
            Some(d) => EnvironmentContext::isolated(
                ExecutableKind::Test,
                d,
                std::collections::HashMap::from_iter(std::env::vars()),
                runtime_args,
                env_path,
                Utf8PathBuf::try_from(current_dir).ok().as_deref(),
                false,
            )
            .map_err(fxe)?,
            None => EnvironmentContext::detect(
                ExecutableKind::Test,
                runtime_args,
                &current_dir,
                env_path,
                false,
            )
            .map_err(fxe)?,
        };
        logging::init_logging(&context);
        logging::LOG_SINK.add_log_output(&context)?;
        log::info!("Logging setup for EnvContext instance: {}", logging::log_id(&context));
        let target_spec: TargetInfoQuery =
            TargetInfoQuery::try_from(ffx_target::get_target_specifier(&context)?)?;

        let device_connection = if matches!(target_spec, TargetInfoQuery::First) {
            log::info!("No target specified. Creating local/testing FDomain.");
            Mutex::new(Some(Arc::new(Connection::from_fdomain_client(
                fdomain_local::local_client_empty(),
            ))))
        } else {
            Mutex::new(None)
        };
        let cache_path = context.get_cache_path()?;
        std::fs::create_dir_all(&cache_path)?;
        Ok(Self { context, device_connection, target_spec, lib_ctx })
    }

    async fn invariant_check(&self) -> Result<()> {
        log::debug!(
            "Checking connectivity invariant for EnvContext: {}",
            logging::log_id(&self.context)
        );
        let mut device_connection = self.device_connection.lock().await;
        // This is a race condition here. It is possible that the connection
        // will have been terminated between here and when this function completes even if
        // `is_terminated` returns `false`, meaning we would end up hitting the timeout in
        // functions like `connect_remote_control_proxy`.
        let device_connection_is_terminated =
            device_connection.as_ref().map(|c| c.is_terminated()).unwrap_or(false);
        if device_connection_is_terminated {
            log::warn!(
                "connection has been interrupted. Attempting to reconnect. Any closed FIDL proxies seen will have been related to this EnvContext's connection having been lost. This is for EnvContext: {}",
                logging::log_id(&self.context)
            );
        }
        if device_connection.is_none() || device_connection_is_terminated {
            *device_connection =
                Some(new_device_connection(&self.context, &self.target_spec).await?);
        }
        log::debug!("Invariant check successful: {}", logging::log_id(&self.context));
        Ok(())
    }

    async fn connect_remote_control_helper<F, Fut>(&self, func: F) -> Result<zx_types::zx_handle_t>
    where
        F: Fn(fdomain_fuchsia_developer_remotecontrol::RemoteControlProxy) -> Fut,
        Fut: Future<Output = Result<zx_types::zx_handle_t>>,
    {
        if matches!(self.target_spec, TargetInfoQuery::First) {
            return Err(unspecified_target());
        }
        // For a bit of history: originally this was written to deal with a race condition in
        // Overnet. What is rare but possible is for us to establish a connection successfully
        // (usually SSH) and at some point afterward drop the connection.
        //
        // The race condition works like follows:
        // 1. We enter a loop that waits for a RemoteControlProxy advertisement to show up in
        //    Overnet.
        // 2. We lose a connection before this advertisement shows up.
        // 3. Because of this if we don't time out we will loop forever given the way the logic for
        //    this is written (see `locate_remote_control_node` in
        //    //src/developer/ffx/lib/target/src/connection.rs if it still exists at the time of
        //    reading this). That code just waits for Overnet to announce that it sees something
        //    advertising the remote control protocol.
        // 4. Once we timeout we run the `invariant_check` again to check if we've somehow
        //    disconnected and that's the reason we've timed out.
        //
        // All that is to say, we might not need this code so much with FDomain, as the method for
        // connecting to RemoteControlProxy really just requires a connection to the device, and
        // there's not a signal we're expecting to surface from a black box. It probably doesn't
        // hurt to keep this here, but we may want to re-examine its usefulness in the future.
        const MAX_RECONNECT_ATTEMPTS: u32 = 1;
        for attempt in 0..=MAX_RECONNECT_ATTEMPTS {
            self.invariant_check().await?;
            let t = Duration::from_secs_f64(self.context.get(ffx_config::keys::PROXY_TIMEOUT)?);
            match timeout::timeout(t, async {
                let rcs_proxy = self.device_connection.lock().await.as_ref().unwrap().rcs_proxy_fdomain().await?;
                log::debug!(
                    "Acquired remote_control_proxy for EnvContext instance: {}",
                    logging::log_id(&self.context)
                );
                func(rcs_proxy).await
            }).await.map_err(|_| {
            anyhow::anyhow!("Timed out attempting to get remote control proxy. This happened after verifying that we can connect to the device, so the device has likely disconnected in the interim.")
                }) {
                // No timeout here (there are two layers of errors)
                Ok(res) => {
                    return Ok(res?);
                }
                Err(e) => {
                    if attempt < MAX_RECONNECT_ATTEMPTS {
                        log::warn!("{e} Attempting to connect once more");
                    } else {
                        log::warn!("{e} Max attempts reached. Giving up and returning error.");
                        return Err(e);
                    }
                }
            }
        }
        // The above will always return eventually.
        unreachable!();
    }

    pub async fn fdomain_client(&self) -> Result<Arc<fdomain_client::Client>> {
        // While this may attempt to reconnect, we may hit similar race conditions that motivated
        // the original `connect_remote_control_helper` function in the future, so it's possible
        // there will need to be future work done here. However, unlike Overnet, there isn't a loop
        // in which we have to wait for something like remote-control-proxy to announce itself,
        // which is what led to the timeouts motivating `connect_remote_control_helper` in the
        // first place.
        //
        // See said function for the explanation of what it's doing and a "bit of history."
        self.invariant_check().await?;
        let (fdomain, _) =
            self.device_connection.lock().await.as_ref().unwrap().fdomain_client().await?;
        Ok(fdomain)
    }

    pub async fn connect_remote_control_proxy(&self) -> Result<zx_types::zx_handle_t> {
        log::debug!(
            "Entering connect_remote_control_proxy for EnvContext instance: {}",
            logging::log_id(&self.context)
        );
        self.connect_remote_control_helper(|proxy| async move {
            let hdl = proxy.into_channel().map_err(fxe)?.into_handle();
            let res = self.lib_ctx().fdomain_state().await.register(hdl);
            Ok(res)
        })
        .await
    }

    pub async fn connect_device_proxy(
        &self,
        moniker: String,
        capability_name: String,
    ) -> Result<zx_types::zx_handle_t> {
        log::debug!(
            "Entering connect_device_proxy for EnvContext instance: {}",
            logging::log_id(&self.context)
        );
        self.connect_remote_control_helper(|rcs_proxy| {
            let capability_name_clone = capability_name.clone();
            let moniker_clone = moniker.clone();
            async move {
                let proxy_timeout =
                    Duration::from_secs_f64(self.context.get(ffx_config::keys::PROXY_TIMEOUT)?);
                let proxy = rcs::connect_with_timeout_at::<ControllerMarker>(
                    proxy_timeout,
                    &moniker_clone,
                    &capability_name_clone,
                    &rcs_proxy,
                )
                .await?;
                log::debug!(
                    "Successfully connected to {moniker_clone}:{capability_name_clone} via RCS"
                );
                let hdl = proxy
                    .into_channel()
                    .map_err(fxe)?
                    .into_handle_based::<fdomain_client::Channel>()
                    .into_handle();
                let res = self.lib_ctx().fdomain_state().await.register(hdl);
                Ok(res)
            }
        })
        .await
    }

    pub async fn target_wait(&self, timeout: u64, offline: bool) -> Result<()> {
        log::debug!(
            "Executing target_wait for EnvContext instance: {}",
            logging::log_id(&self.context)
        );
        if matches!(self.target_spec, TargetInfoQuery::First) {
            return Err(unspecified_target());
        }
        let state = if offline {
            ffx_wait_args::TargetStateOption::Down
        } else {
            ffx_wait_args::TargetStateOption::Up
        };
        let cmd = ffx_wait_args::WaitOptions { timeout, down: offline, state: None };
        let tool = ffx_wait::WaitOperation {
            cmd,
            env: self.context.clone(),
            waiter: ffx_wait::DeviceWaiterImpl,
        };
        tool.wait_impl(state).await.map_err(Into::into)
    }
}

impl Drop for EnvContext {
    fn drop(&mut self) {
        log::info!("Dropping EnvContext {}", logging::log_id(&self.context));
        logging::LOG_SINK.remove_log_output(&self.context).expect("remove logger safely");
    }
}
