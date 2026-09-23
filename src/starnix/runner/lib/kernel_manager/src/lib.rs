// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod kernels;
pub mod pager;
pub mod proxy;
pub mod suspend;

use crate::pager::{Pager, run_pager};
use anyhow::{Error, anyhow};
use fidl::Peered;
use fidl::endpoints::{DiscoverableProtocolMarker, Proxy, ServerEnd};
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_component_runner as frunner;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_starnix_container as fstarnix;
use fidl_fuchsia_starnix_runner as fstarnixrunner;
use fuchsia_async as fasync;
use fuchsia_component::client as fclient;
use fuchsia_inspect::{HistogramProperty, NumericProperty};
use fuchsia_sync::Mutex;
use futures::TryStreamExt;
use kernels::Kernels;
use log::warn;
use proxy::ChannelProxy;
use rand::RngExt as _;
use starnix_sync::{LockDepMutex, WakeSourcesLock};
use std::future::Future;
use std::sync::Arc;
use suspend::{
    ASLEEP_SIGNAL, AWAKE_SIGNAL, SuspendContext, WakeSource, WakeSources, suspend_container,
};

/// The name of the collection that the starnix_kernel is run in.
const KERNEL_COLLECTION: &str = "kernels";

/// The name of the protocol the kernel exposes for running containers.
///
/// This protocol is actually fuchsia.component.runner.ComponentRunner. We
/// expose the implementation using this name to avoid confusion with copy
/// of the fuchsia.component.runner.ComponentRunner protocol used for
/// running component inside the container.
const CONTAINER_RUNNER_PROTOCOL: &str = "fuchsia.starnix.container.Runner";

#[allow(dead_code)]
pub struct StarnixKernel {
    /// The name of the kernel intsance in the kernels collection.
    name: String,

    /// The directory exposed by the Starnix kernel.
    ///
    /// This directory can be used to connect to services offered by the kernel.
    exposed_dir: fio::DirectoryProxy,

    /// An opaque token representing the container component.
    component_instance: zx::Event,

    /// The job the kernel lives under.
    job: Arc<zx::Job>,

    /// The currently active wake lease for the container.
    wake_lease: Mutex<Option<zx::EventPair>>,
}

impl StarnixKernel {
    /// Creates a new instance of `starnix_kernel`.
    ///
    /// This is done by creating a new child in the `kernels` collection.
    ///
    /// Returns the kernel and a future that will resolve when the kernel has stopped.
    pub async fn create(
        realm: fcomponent::RealmProxy,
        kernel_url: &str,
        start_info: frunner::ComponentStartInfo,
        controller: ServerEnd<frunner::ComponentControllerMarker>,
    ) -> Result<(Self, impl Future<Output = ()>), Error> {
        let kernel_name = generate_kernel_name(&start_info)?;
        let component_instance = start_info
            .component_instance
            .as_ref()
            .ok_or_else(|| {
                anyhow::anyhow!("expected to find component_instance in ComponentStartInfo")
            })?
            .duplicate_handle(zx::Rights::SAME_RIGHTS)?;

        // Create the `starnix_kernel`.
        let (controller_proxy, controller_server_end) = fidl::endpoints::create_proxy();
        realm
            .create_child(
                &fdecl::CollectionRef { name: KERNEL_COLLECTION.into() },
                &fdecl::Child {
                    name: Some(kernel_name.clone()),
                    url: Some(kernel_url.to_string()),
                    startup: Some(fdecl::StartupMode::Lazy),
                    ..Default::default()
                },
                fcomponent::CreateChildArgs {
                    controller: Some(controller_server_end),
                    ..Default::default()
                },
            )
            .await?
            .map_err(|e| anyhow::anyhow!("failed to create kernel: {:?}", e))?;

        let exposed_dir = open_exposed_directory(&realm, &kernel_name, KERNEL_COLLECTION).await?;
        let container_runner = fclient::connect_to_named_protocol_at_dir_root::<
            frunner::ComponentRunnerMarker,
        >(&exposed_dir, CONTAINER_RUNNER_PROTOCOL)?;

        // Actually start the container.
        container_runner.start(start_info, controller)?;

        // Ask the kernel for its job.
        let container_controller =
            fclient::connect_to_protocol_at_dir_root::<fstarnix::ControllerMarker>(&exposed_dir)?;
        let fstarnix::ControllerGetJobHandleResponse { job, .. } =
            container_controller.get_job_handle().await?;
        let Some(job) = job else {
            anyhow::bail!("expected to find job in ControllerGetJobHandleResponse");
        };

        let kernel = Self {
            name: kernel_name,
            exposed_dir,
            component_instance,
            job: Arc::new(job),
            wake_lease: Default::default(),
        };
        let on_stop = async move {
            _ = controller_proxy.into_channel().unwrap().on_closed().await;
        };
        Ok((kernel, on_stop))
    }

    /// Gets the opaque token representing the container component.
    pub fn component_instance(&self) -> &zx::Event {
        &self.component_instance
    }

    /// Gets the job the kernel lives under.
    pub fn job(&self) -> &Arc<zx::Job> {
        &self.job
    }

    /// Connect to the specified protocol exposed by the kernel.
    pub fn connect_to_protocol<P: DiscoverableProtocolMarker>(&self) -> Result<P::Proxy, Error> {
        fclient::connect_to_protocol_at_dir_root::<P>(&self.exposed_dir)
    }
}

/// Generate a random name for the kernel.
///
/// We used to include some human-readable parts in the name, but people were
/// tempted to make them load-bearing. We now just generate 7 random alphanumeric
/// characters.
fn generate_kernel_name(_start_info: &frunner::ComponentStartInfo) -> Result<String, Error> {
    let random_id: String =
        rand::rng().sample_iter(&rand::distr::Alphanumeric).take(7).map(char::from).collect();
    Ok(random_id)
}

async fn open_exposed_directory(
    realm: &fcomponent::RealmProxy,
    child_name: &str,
    collection_name: &str,
) -> Result<fio::DirectoryProxy, Error> {
    let (directory_proxy, server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
    realm
        .open_exposed_dir(
            &fdecl::ChildRef { name: child_name.into(), collection: Some(collection_name.into()) },
            server_end,
        )
        .await?
        .map_err(|e| {
            anyhow!(
                "failed to bind to child {} in collection {:?}: {:?}",
                child_name,
                collection_name,
                e
            )
        })?;
    Ok(directory_proxy)
}

pub struct SuspendMessage {
    pub payload: fstarnixrunner::ManagerSuspendContainerRequest,
    pub responder: fstarnixrunner::ManagerSuspendContainerResponder,
}

pub async fn run_suspend_worker(
    receiver: async_channel::Receiver<SuspendMessage>,
    suspend_context: Arc<SuspendContext>,
    kernels: &Kernels,
) {
    while let Ok(msg) = receiver.recv().await {
        suspend_context.suspend_attempts_count.add(1);
        match suspend_container(msg.payload, &suspend_context, &kernels).await {
            Ok(response) => {
                if let Err(e) = match response {
                    Ok(o) => {
                        suspend_context.suspend_successes_count.add(1);
                        if let Some(suspend_time) = o.suspend_time {
                            suspend_context.suspend_duration_histogram.insert(suspend_time as u64);
                        }
                        msg.responder.send(Ok(&o))
                    }
                    Err(e) => {
                        suspend_context.suspend_failures_count.add(1);
                        msg.responder.send(Err(e))
                    }
                } {
                    warn!("error replying to suspend request: {e}");
                }
            }
            Err(e) => {
                suspend_context.suspend_failures_count.add(1);
                warn!("error executing suspend: {e}");
                let _ = msg.responder.send(Err(fstarnixrunner::SuspendError::SuspendFailure));
            }
        }
    }
}

pub async fn serve_starnix_manager(
    mut stream: fstarnixrunner::ManagerRequestStream,
    suspend_context: Arc<SuspendContext>,
    sender: &async_channel::Sender<(ChannelProxy, Arc<LockDepMutex<WakeSources, WakeSourcesLock>>)>,
    pager: Arc<Pager>,
    suspend_sender: &async_channel::Sender<SuspendMessage>,
) -> Result<(), Error> {
    while let Some(event) = stream.try_next().await? {
        match event {
            fstarnixrunner::ManagerRequest::SuspendContainer { payload, responder, .. } => {
                if let Err(e) = suspend_sender.try_send(SuspendMessage { payload, responder }) {
                    warn!("failed to send suspend request to worker: {e}");
                }
            }
            fstarnixrunner::ManagerRequest::ProxyWakeChannel { payload, .. } => {
                let fstarnixrunner::ManagerProxyWakeChannelRequest {
                    // TODO: Handle more than one container.
                    container_job: Some(_container_job),
                    remote_channel: Some(remote_channel),
                    container_channel: Some(container_channel),
                    name: Some(name),
                    counter: Some(message_counter),
                    ..
                } = payload
                else {
                    continue;
                };

                suspend_context.wake_sources.lock().insert(
                    message_counter.koid().unwrap(),
                    WakeSource::from_counter(
                        message_counter.duplicate_handle(zx::Rights::SAME_RIGHTS)?,
                        name.clone(),
                    ),
                );

                let proxy =
                    ChannelProxy { container_channel, remote_channel, message_counter, name };

                sender.try_send((proxy, suspend_context.wake_sources.clone())).unwrap();
            }
            fstarnixrunner::ManagerRequest::RegisterWakeWatcher { payload, responder } => {
                if let Some(watcher) = payload.watcher {
                    let (clear_mask, set_mask) = (ASLEEP_SIGNAL, AWAKE_SIGNAL);
                    watcher.signal_peer(clear_mask, set_mask)?;

                    suspend_context.wake_watchers.lock().push(watcher);
                }
                if let Err(e) = responder.send() {
                    warn!("error registering power watcher: {e}");
                }
            }
            fstarnixrunner::ManagerRequest::AddWakeSource { payload, .. } => {
                let fstarnixrunner::ManagerAddWakeSourceRequest {
                    // TODO: Handle more than one container.
                    container_job: Some(_container_job),
                    name: Some(name),
                    handle: Some(handle),
                    signals: Some(signals),
                    ..
                } = payload
                else {
                    continue;
                };
                suspend_context.wake_sources.lock().insert(
                    handle.koid().unwrap(),
                    WakeSource::from_handle(handle, name, zx::Signals::from_bits_truncate(signals)),
                );
            }
            fstarnixrunner::ManagerRequest::RemoveWakeSource { payload, .. } => {
                let fstarnixrunner::ManagerRemoveWakeSourceRequest {
                    // TODO: Handle more than one container.
                    container_job: Some(_container_job),
                    handle: Some(handle),
                    ..
                } = payload
                else {
                    continue;
                };

                suspend_context.wake_sources.lock().remove(&handle.koid().unwrap());
            }
            fstarnixrunner::ManagerRequest::CreatePager { payload, .. } => {
                fasync::Task::spawn(run_pager(payload, pager.clone())).detach();
            }
            _ => {}
        }
    }
    Ok(())
}
