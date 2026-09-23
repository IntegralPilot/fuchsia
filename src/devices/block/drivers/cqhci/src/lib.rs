// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::BTreeMap;
use std::sync::{Arc, Weak};

use crate::command_queue::{CommandQueue, CommandQueueHost, ResumeHandle, TaskStatusReceiver};
use crate::partition::EmmcPartition;
use block_server::callback_interface::SessionManager;
use block_server::{BlockServer, RequestId};
use cqhci_config::Config;
use fdf_component::{Driver, DriverContext, DriverError, Node, ServiceInstance, driver_register};
use fdf_power::{Suspendable, SuspendableDriver};
use fidl_fuchsia_driver_framework::PowerElementArgs;
use fidl_fuchsia_driver_token as ftoken;
use fidl_fuchsia_hardware_block_volume as fvolume;
use fidl_fuchsia_hardware_inlineencryption as finlineencryption;
use fidl_fuchsia_power_system as fpower;
use fidl_fuchsia_storage_block as fblock;
use fidl_fuchsia_storage_block::BlockInfo;
use fidl_next_fuchsia_hardware_cqhci::{self as cqhci, EmmcPartitionId};
use fidl_next_fuchsia_hardware_rpmb as rpmb;
use fuchsia_async as fasync;
use fuchsia_async::Scope;
use fuchsia_component::server::ServiceFs;
use fuchsia_sync::Mutex;
use futures::StreamExt as _;
use log::{debug, error, info, warn};

mod command_queue;
mod dma_buffer;
mod partition;
mod transfer_manager;

#[cfg(test)]
mod tests;

pub fn partition_name(id: EmmcPartitionId) -> &'static str {
    match id {
        EmmcPartitionId::UserDataPartition => "user",
        EmmcPartitionId::BootPartition1 => "boot1",
        EmmcPartitionId::BootPartition2 => "boot2",
    }
}

async fn handle_token_requests(
    token: Arc<zx::Event>,
    mut requests: ftoken::NodeTokenRequestStream,
) -> Result<(), anyhow::Error> {
    while let Some(request) = requests.next().await {
        match request? {
            ftoken::NodeTokenRequest::Get { responder } => {
                responder.send(
                    token.duplicate_handle(zx::Rights::SAME_RIGHTS).map_err(zx::Status::into_raw),
                )?;
            }
        }
    }
    Ok(())
}

async fn handle_inline_encryption_requests(
    client: fidl_next::Client<fidl_next_fuchsia_hardware_inlineencryption::DriverDevice>,
    mut stream: finlineencryption::DeviceRequestStream,
) -> Result<(), anyhow::Error> {
    while let Some(request) = stream.next().await {
        match request? {
            finlineencryption::DeviceRequest::ProgramKey {
                wrapped_key,
                data_unit_size,
                responder,
            } => match client.program_key(wrapped_key, data_unit_size).await? {
                Ok(response) => responder.send(Ok(response.slot))?,
                Err(Err(status)) => responder.send(Err(status.into_raw()))?,
                Err(Ok(())) => responder.send(Err(zx::Status::INTERNAL.into_raw()))?,
            },
            finlineencryption::DeviceRequest::DeriveRawSecret { wrapped_key, responder } => {
                match client.derive_raw_secret(wrapped_key).await? {
                    Ok(response) => responder.send(Ok(&response.secret))?,
                    Err(Err(status)) => responder.send(Err(status.into_raw()))?,
                    Err(Ok(())) => responder.send(Err(zx::Status::INTERNAL.into_raw()))?,
                }
            }
        }
    }
    Ok(())
}

struct PartitionServer {
    server: BlockServer<SessionManager<EmmcPartition>>,
}

impl PartitionServer {
    fn new(
        block_info: BlockInfo,
        partition: EmmcPartitionId,
        command_queue: Weak<CommandQueue>,
    ) -> Self {
        Self {
            server: BlockServer::new(
                block_info.block_size,
                Arc::new(SessionManager::new(
                    Arc::new(EmmcPartition::new(partition, command_queue, block_info)),
                    block_info.block_size,
                )),
            ),
        }
    }

    fn shutdown(&self) {
        self.server.session_manager().terminate();
    }
}

impl TaskStatusReceiver for PartitionServer {
    fn complete(&self, request_id: RequestId, status: Result<(), zx::Status>) {
        self.server.session_manager().complete_request(request_id, status);
    }
}

struct CqhciDriver {
    _node: Arc<Node>,
    // This scope handles incoming FIDL requests.  Tasks in this scope may retain strong references
    // to [`Self::command_queue`].
    scope: Mutex<Option<Scope>>,
    partitions: Arc<Mutex<BTreeMap<String, Arc<PartitionServer>>>>,
    command_queue: Mutex<Option<Arc<CommandQueue>>>,
    resume_handle: Mutex<Option<ResumeHandle>>,
    suspend_enabled: bool,
}

driver_register!(Suspendable<CqhciDriver>);

#[cfg(test)]
pub(crate) static SHUTTING_DOWN_FLAG: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

struct RpmbService {
    scope: Scope,
    command_queue: Weak<CommandQueue>,
}

impl rpmb::ServiceHandler for RpmbService {
    fn device(&self, server_end: fidl_next::ServerEnd<rpmb::Rpmb>) {
        if let Some(command_queue) = self.command_queue.upgrade() {
            server_end.spawn_on(RpmbConnection { command_queue }, &self.scope);
        }
    }
}

struct RpmbConnection {
    command_queue: Arc<CommandQueue>,
}

impl rpmb::RpmbServerHandler for RpmbConnection {
    async fn get_device_info(
        &mut self,
        responder: fidl_next::Responder<rpmb::rpmb::GetDeviceInfo>,
    ) {
        match self.command_queue.get_rpmb_info().await {
            Ok(info) => {
                if let Err(error) = responder.respond(info).await {
                    log::warn!(error:?; "Failed to send rpmb GetDeviceInfo response");
                }
            }
            Err(status) => {
                log::warn!("Failed to get rpmb info: {}", status);
            }
        }
    }

    async fn request(
        &mut self,
        request: fidl_next::Request<rpmb::rpmb::Request>,
        responder: fidl_next::Responder<rpmb::rpmb::Request>,
    ) {
        self.command_queue.rpmb_request(
            request.payload().request,
            async |result: Result<(), zx::Status>| {
                if let Err(error) = match result {
                    Ok(()) => responder.respond(()).await,
                    Err(status) => responder.respond_err(status).await,
                } {
                    log::warn!(error:?; "Failed to send rpmb response");
                };
            },
        );
    }
}

#[cfg(not(test))]
fn get_cqhci_client(
    service: &ServiceInstance<cqhci::Service>,
) -> Result<Box<dyn CommandQueueHost>, zx::Status> {
    let (cqhci_client_end, cqhci_server_end) = fdf_fidl::create_channel();
    service.cqhci(cqhci_server_end).map_err(|error| {
        error!(error:?; "Failed to connect to Cqhci protocol");
        zx::Status::INVALID_ARGS
    })?;
    Ok(Box::new(cqhci_client_end.spawn()))
}

/// Inject a fake instance of CommandQueueHost for testing hooks.
/// We do this in order to substitute fake MMIOs for testing purposes.
#[cfg(test)]
fn get_cqhci_client(
    _service: &ServiceInstance<cqhci::Service>,
) -> Result<Box<dyn CommandQueueHost>, zx::Status> {
    Ok(tests::TestCommandQueueHost::global())
}

impl Driver for CqhciDriver {
    const NAME: &str = "cqhci";

    async fn start(mut context: DriverContext) -> Result<Self, DriverError> {
        let config = context.take_config::<Config>().map_err(|error| {
            error!(error:?; "Failed to take config");
            error
        })?;
        info!(config:?; "cqhci driver starting");
        let suspend_enabled = config.suspend_enabled;
        let (cqhci, rpmb, inline_crypto) = {
            let service: ServiceInstance<cqhci::Service> =
                context.incoming.service().connect_next().inspect_err(|status| {
                    error!(status:?; "Failed to connect to Cqhci service");
                })?;
            let (rpmb_client_end, rpmb_server_end) = fdf_fidl::create_channel();
            service.rpmb(rpmb_server_end).map_err(|error| {
                error!(error:?; "Failed to connect to Rpmb protocol");
                zx::Status::INVALID_ARGS
            })?;
            let rpmb = rpmb_client_end.spawn();

            let inline_crypto = {
                let (client_end, server_end) = fdf_fidl::create_channel();
                if let Err(error) = service.inline_crypto(server_end) {
                    warn!(error:?; "Failed to connect to InlineCrypto protocol");
                    None
                } else {
                    Some(client_end.spawn())
                }
            };

            (get_cqhci_client(&service)?, rpmb, inline_crypto)
        };

        let vmar =
            context.vmar().duplicate_handle(zx::Rights::SAME_RIGHTS).inspect_err(|status| {
                error!(status:?; "Failed to duplicate VMAR");
            })?;

        let host_info = cqhci.info().await.inspect_err(|status| {
            error!(status:?; "Failed to get host info");
        })?;

        if suspend_enabled {
            if let Some(PowerElementArgs { token: Some(token), .. }) =
                &context.start_args.power_element_args
            {
                let token = token.duplicate_handle(zx::Rights::SAME_RIGHTS)?;
                configure_power_management(&context, token).await.inspect_err(|error| {
                    error!(error:?; "Failed to configure power management");
                })?;
                info!("Configured power management successfully (cqhci)");
            }
        }

        let mut fs = ServiceFs::new();
        let scope = Scope::new();

        let (command_queue, partitions_by_id) = CommandQueue::initialize(
            vmar,
            cqhci,
            rpmb,
            host_info,
            move |weak_cq, host_info, device_flags| {
                let mut partitions = BTreeMap::new();
                for partition in &host_info.partitions {
                    let block_info = fblock::BlockInfo {
                        block_count: partition.block_count,
                        block_size: partition.block_size,
                        max_transfer_size: host_info.sdmmc_host_info.max_transfer_size,
                        flags: device_flags,
                    };
                    let partition_server =
                        Arc::new(PartitionServer::new(block_info, partition.id, weak_cq.clone()));
                    partitions.insert(partition.id, partition_server);
                }
                partitions
            },
        )
        .await
        .inspect_err(|error| {
            error!(error:?; "Failed to initialize command queueing");
        })?;

        let mut partitions = BTreeMap::new();
        for (id, partition) in partitions_by_id {
            let name = partition_name(id);
            fs.dir("svc")
                .add_fidl_service_instance(name, move |request| (request, name.to_string()));
            partitions.insert(name.to_string(), partition);
        }
        let partitions = Arc::new(Mutex::new(partitions));
        let partitions_clone = partitions.clone();

        fs.dir("svc").add_fidl_next_service_instance::<rpmb::Service, _>(
            "default",
            RpmbService { scope: scope.new_child(), command_queue: Arc::downgrade(&command_queue) },
        );

        context.serve_outgoing(&mut fs)?;

        let node = Arc::new(context.take_node()?);
        let node_token = context.start_args.node_token.take().map(Arc::new);
        let inline_crypto_clone = inline_crypto.clone();
        let driver = CqhciDriver {
            _node: node,
            scope: Mutex::new(Some(scope)),
            partitions,
            command_queue: Mutex::new(Some(command_queue)),
            resume_handle: Mutex::default(),
            suspend_enabled,
        };
        if let Some(scope) = driver.scope.lock().as_ref() {
            scope.spawn(async move {
                fs.for_each_concurrent(None, move |(request, partition_name)| {
                    let partitions_clone = partitions_clone.clone();
                    let node_token = node_token.clone();
                    let inline_crypto_clone = inline_crypto_clone.clone();
                    async move {
                        match request {
                            fvolume::ServiceRequest::Volume(requests) => {
                                let partition =
                                    partitions_clone.lock().get(&partition_name).cloned();
                                if let Some(partition_server) = partition {
                                    if let Err(error) =
                                        partition_server.server.handle_requests(requests).await
                                    {
                                        error!(
                                            error:?;
                                            "Failed to handle requests for part {partition_name}"
                                        );
                                    }
                                } else {
                                    error!("Invalid partition {partition_name}");
                                }
                            }
                            fvolume::ServiceRequest::Mapper(requests) => {
                                let partition =
                                    partitions_clone.lock().get(&partition_name).cloned();
                                if let Some(partition_server) = partition {
                                    if let Err(error) = partition_server
                                        .server
                                        .handle_mapper_requests(requests)
                                        .await
                                    {
                                        error!(
                                            error:?;
                                            "Failed to handle mapper requests \
                                             for part {partition_name}"
                                        );
                                    }
                                } else {
                                    error!("Invalid partition {partition_name}");
                                }
                            }
                            fvolume::ServiceRequest::InlineEncryption(requests) => {
                                if let Some(inline_crypto) = inline_crypto_clone.clone() {
                                    if let Err(error) =
                                        handle_inline_encryption_requests(inline_crypto, requests)
                                            .await
                                    {
                                        error!(
                                            error:?;
                                            "Failed to handle inline encryption requests \
                                             for part {partition_name}"
                                        );
                                    }
                                } else {
                                    error!("Inline encryption not supported by underlying device.");
                                }
                            }
                            fvolume::ServiceRequest::Token(stream) => {
                                if let Some(token) = node_token {
                                    if let Err(error) = handle_token_requests(token, stream).await {
                                        error!(
                                            error:?;
                                            "Failed to handle token requests \
                                                for part {partition_name}"
                                        );
                                    }
                                } else {
                                    error!("Node token wasn't provided by framework");
                                }
                            }
                        }
                    }
                })
                .await;
            });
        }

        info!("cqhci driver started");
        Ok(driver)
    }

    async fn stop(&self) {
        info!("cqhci driver stopping");
        #[cfg(test)]
        SHUTTING_DOWN_FLAG.store(true, std::sync::atomic::Ordering::SeqCst);

        let Some(command_queue) = self.command_queue.lock().take() else { unreachable!() };
        // This will resume if currently suspended.
        let resume_handle = self.resume_handle.lock().take();
        if let Some(resume_handle) = resume_handle {
            let _ = resume_handle.resume().await;
        }
        command_queue.shutdown().await;
        debug!("cqhci shut down");

        {
            let partitions = std::mem::take(&mut *self.partitions.lock());
            for (_, partition_server) in partitions {
                fasync::unblock(move || {
                    partition_server.shutdown();
                })
                .await;
            }
        }
        debug!("sessions closed");
        let scope = self.scope.lock().take();
        if let Some(scope) = scope {
            scope.cancel().await;
        }
        debug!("scope cancelled");
        debug_assert!(Arc::strong_count(&command_queue) == 1);
        command_queue.unpin_buffers();
        info!("cqhci driver stopped");
    }
}

impl SuspendableDriver for CqhciDriver {
    async fn suspend(&self) {
        let Some(cq) = self.command_queue.lock().as_ref().cloned() else { return };
        match cq.suspend().await {
            Ok(resume_handle) => {
                let _ = self.resume_handle.lock().replace(resume_handle);
            }
            Err(error) => {
                error!(error:?; "Failed to suspend CQHCI");
            }
        }
    }

    async fn resume(&self) {
        let resume_handle = self.resume_handle.lock().take();
        if let Some(resume_handle) = resume_handle {
            if let Err(error) = resume_handle.resume().await {
                error!(error:?; "Failed to resume CQHCI");
            }
        } else {
            warn!("Nothing to resume because not suspended");
        }
    }

    fn suspend_enabled(&self) -> bool {
        self.suspend_enabled
    }
}

/// Sets up the command queuing driver as the main dependency for the CPU element so that we
/// properly record this driver as needing to be functioning in order to handle blob page requests
/// (which most executable code relies on).  When command queuing is enabled, it is the top-level
/// driver so it is responsible for setting up this relationship.  The command queuing driver is the
/// root of a dependency tree (i.e. it is dependent on its parent driver and it might have further
/// dependencies) which the power framework manages.
async fn configure_power_management(
    context: &DriverContext,
    token: zx::Event,
) -> Result<(), zx::Status> {
    let cpu_element_manager =
        context.incoming.connect_protocol::<fpower::CpuElementManagerProxy>()?;
    cpu_element_manager
        .add_execution_state_dependency(
            fpower::CpuElementManagerAddExecutionStateDependencyRequest {
                dependency_token: Some(token),
                power_level: Some(1),
                ..Default::default()
            },
        )
        .await
        .map_err(|error| {
            error!(error:?; "CpuElementManager FIDL error");
            zx::Status::INTERNAL
        })?
        .map_err(|error| {
            error!(error:?; "CpuElementManager domain error");
            match error {
                fpower::AddExecutionStateDependencyError::InvalidArgs => zx::Status::INVALID_ARGS,
                fpower::AddExecutionStateDependencyError::BadState => zx::Status::BAD_STATE,
                _ => zx::Status::INTERNAL,
            }
        })
}
