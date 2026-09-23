// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::config::Config;
use crate::gpt::GptManager;
use anyhow::{Context as _, Error};
use block_client::RemoteBlockClient;
use fidl::endpoints::{DiscoverableProtocolMarker as _, RequestStream as _, ServiceMarker as _};
use fidl_fuchsia_fs as ffs;
use fidl_fuchsia_fs_startup as fstartup;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_process_lifecycle as flifecycle;
use fidl_fuchsia_storage_block as fblock;
use fidl_fuchsia_storage_partitions as fpartitions;
use fuchsia_async as fasync;
use futures::lock::Mutex as AsyncMutex;
use futures::stream::TryStreamExt as _;
use std::sync::Arc;
use vfs::directory::helper::DirectlyMutable as _;
use vfs::execution_scope::ExecutionScope;

pub struct GptService {
    state: AsyncMutex<State>,

    // The execution scope of the pseudo filesystem.
    scope: ExecutionScope,

    // The root of the pseudo filesystem for the component.
    export_dir: Arc<vfs::directory::immutable::Simple>,

    // A directory where partitions are published.
    partitions_dir: Arc<vfs::directory::immutable::Simple>,
}

#[derive(Default)]
enum State {
    #[default]
    Stopped,
    /// The GPT is malformed and needs to be reformatted with ResetPartitionTables before it can be
    /// used.  The component will publish an empty partitions directory.
    NeedsFormatting(Config, fblock::BlockProxy),
    Running(Arc<GptManager>),
}

impl State {
    fn is_stopped(&self) -> bool {
        if let Self::Stopped = self { true } else { false }
    }
}

impl GptService {
    pub fn new() -> Arc<Self> {
        let export_dir = vfs::directory::immutable::simple();
        let partitions_dir = vfs::directory::immutable::simple();
        Arc::new(Self {
            state: Default::default(),
            scope: ExecutionScope::new(),
            export_dir,
            partitions_dir,
        })
    }

    pub async fn run(
        self: Arc<Self>,
        outgoing_dir: zx::Channel,
        lifecycle_channel: Option<zx::Channel>,
    ) -> Result<(), Error> {
        let svc_dir = vfs::directory::immutable::simple();
        self.export_dir.add_entry("svc", svc_dir.clone()).expect("Unable to create svc dir");

        svc_dir
            .add_entry(
                fpartitions::PartitionServiceMarker::SERVICE_NAME,
                self.partitions_dir.clone(),
            )
            .unwrap();
        let weak = Arc::downgrade(&self);
        svc_dir
            .add_entry(
                fstartup::StartupMarker::PROTOCOL_NAME,
                vfs::service::host(move |requests| {
                    let weak = weak.clone();
                    async move {
                        if let Some(me) = weak.upgrade() {
                            let _ = me.handle_start_requests(requests).await;
                        }
                    }
                }),
            )
            .unwrap();
        let weak = Arc::downgrade(&self);
        svc_dir
            .add_entry(
                fpartitions::PartitionsAdminMarker::PROTOCOL_NAME,
                vfs::service::host(move |requests| {
                    let weak = weak.clone();
                    async move {
                        if let Some(me) = weak.upgrade() {
                            let _ = me.handle_partitions_admin_requests(requests).await;
                        }
                    }
                }),
            )
            .unwrap();
        let weak = Arc::downgrade(&self);
        svc_dir
            .add_entry(
                fpartitions::PartitionsManagerMarker::PROTOCOL_NAME,
                vfs::service::host(move |requests| {
                    let weak = weak.clone();
                    async move {
                        if let Some(me) = weak.upgrade() {
                            let _ = me.handle_partitions_manager_requests(requests).await;
                        }
                    }
                }),
            )
            .unwrap();
        let weak = Arc::downgrade(&self);
        svc_dir
            .add_entry(
                ffs::AdminMarker::PROTOCOL_NAME,
                vfs::service::host(move |requests| {
                    let weak = weak.clone();
                    async move {
                        if let Some(me) = weak.upgrade() {
                            let _ = me.handle_admin_requests(requests).await;
                        }
                    }
                }),
            )
            .unwrap();

        vfs::directory::serve_on(
            self.export_dir.clone(),
            fio::PERM_READABLE | fio::PERM_WRITABLE | fio::PERM_EXECUTABLE,
            self.scope.clone(),
            fidl::endpoints::ServerEnd::new(outgoing_dir),
        );

        if let Some(channel) = lifecycle_channel {
            let me = self.clone();
            self.scope.spawn(async move {
                if let Err(e) = me.handle_lifecycle_requests(channel).await {
                    log::warn!(error:? = e; "handle_lifecycle_requests");
                }
            });
        }

        self.scope.wait().await;

        Ok(())
    }

    async fn handle_start_requests(
        self: Arc<Self>,
        mut stream: fstartup::StartupRequestStream,
    ) -> Result<(), Error> {
        while let Some(request) = stream.try_next().await.context("Reading request")? {
            log::debug!(request:?; "");
            match request {
                fstartup::StartupRequest::Start { device, options, responder } => {
                    responder
                        .send(
                            self.start(device.into_proxy(), options.into())
                                .await
                                .map_err(|status| status.into_raw()),
                        )
                        .unwrap_or_else(|e| log::error!(e:?; "Failed to send Start response"));
                }
                fstartup::StartupRequest::Format { responder, .. } => {
                    responder
                        .send(Err(zx::Status::NOT_SUPPORTED.into_raw()))
                        .unwrap_or_else(|e| log::error!(e:?; "Failed to send Check response"));
                }
                fstartup::StartupRequest::Check { responder, .. } => {
                    responder
                        .send(Err(zx::Status::NOT_SUPPORTED.into_raw()))
                        .unwrap_or_else(|e| log::error!(e:?; "Failed to send Check response"));
                }
            }
        }
        Ok(())
    }

    async fn start(
        self: &Arc<Self>,
        device: fblock::BlockProxy,
        config: Config,
    ) -> Result<(), zx::Status> {
        log::info!(config:?; "GPT starting");
        let mut state = self.state.lock().await;
        if !state.is_stopped() {
            log::warn!("Device already bound");
            return Err(zx::Status::ALREADY_BOUND);
        }

        // TODO(https://fxbug.dev/339491886): It would be better if `start` failed on a malformed
        // device, rather than hiding this state from fshost.  However, fs_management isn't well set
        // up to deal with this, because it ties the outgoing directory to a successful return from
        // `start` (see `ServingMultiVolumeFilesystem`), and fshost resolves the queued requests for
        // the filesystem only after `start` is successful.  We should refactor fs_management and
        // fshost to better support this case, which might require changing how queueing works so
        // there's more flexibility to either resolve queueing when the component starts up (what we
        // need here), or when `Start` is successful (what a filesystem like Fxfs needs).
        *state = match GptManager::new_with_config(
            device.clone(),
            self.partitions_dir.clone(),
            config.clone(),
        )
        .await
        {
            Ok(runner) => State::Running(runner),
            Err(err) => {
                // This is a warning because, as described above, we can't return an error from
                // here, so there are normal flows that can print this error message that don't
                // indicate a bug or incorrect behavior.
                log::warn!(err:?; "Failed to load GPT.  Reformatting may be required.");
                State::NeedsFormatting(config, device)
            }
        };
        Ok(())
    }

    async fn handle_partitions_manager_requests(
        self: Arc<Self>,
        mut stream: fpartitions::PartitionsManagerRequestStream,
    ) -> Result<(), Error> {
        while let Some(request) = stream.try_next().await.context("Reading request")? {
            log::debug!(request:?; "");
            match request {
                fpartitions::PartitionsManagerRequest::GetBlockInfo { responder } => {
                    responder
                        .send(self.get_block_info().await.map_err(|status| status.into_raw()))
                        .unwrap_or_else(
                            |e| log::error!(e:?; "Failed to send GetBlockInfo response"),
                        );
                }
                fpartitions::PartitionsManagerRequest::CreateTransaction { responder } => {
                    responder
                        .send(self.create_transaction().await.map_err(|status| status.into_raw()))
                        .unwrap_or_else(
                            |e| log::error!(e:?; "Failed to send CreateTransaction response"),
                        );
                }
                fpartitions::PartitionsManagerRequest::CommitTransaction {
                    transaction,
                    responder,
                } => {
                    responder
                        .send(
                            self.commit_transaction(transaction)
                                .await
                                .map_err(|status| status.into_raw()),
                        )
                        .unwrap_or_else(
                            |e| log::error!(e:?; "Failed to send CommitTransaction response"),
                        );
                }
                fpartitions::PartitionsManagerRequest::AddPartition { payload, responder } => {
                    responder
                        .send(self.add_partition(payload).await.map_err(|status| status.into_raw()))
                        .unwrap_or_else(
                            |e| log::error!(e:?; "Failed to send AddPartition response"),
                        );
                }
            }
        }
        Ok(())
    }

    async fn get_block_info(&self) -> Result<(u64, u32), zx::Status> {
        let state = self.state.lock().await;
        match &*state {
            State::Stopped => return Err(zx::Status::BAD_STATE),
            State::NeedsFormatting(_, block) => {
                let info = block
                    .get_info()
                    .await
                    .map_err(|err| {
                        log::error!(err:?; "get_block_info: failed to query block info");
                        zx::Status::IO
                    })?
                    .map_err(zx::Status::err_from_raw)?;
                Ok((info.block_count, info.block_size))
            }
            State::Running(gpt) => Ok((gpt.block_count(), gpt.block_size())),
        }
    }

    async fn create_transaction(&self) -> Result<zx::EventPair, zx::Status> {
        let gpt_manager = self.gpt_manager().await?;
        gpt_manager.create_transaction().await
    }

    async fn commit_transaction(&self, transaction: zx::EventPair) -> Result<(), zx::Status> {
        let gpt_manager = self.gpt_manager().await?;
        gpt_manager.commit_transaction(transaction).await
    }

    async fn add_partition(
        &self,
        request: fpartitions::PartitionsManagerAddPartitionRequest,
    ) -> Result<(), zx::Status> {
        let gpt_manager = self.gpt_manager().await?;
        gpt_manager.add_partition(request).await
    }

    async fn handle_partitions_admin_requests(
        self: Arc<Self>,
        mut stream: fpartitions::PartitionsAdminRequestStream,
    ) -> Result<(), Error> {
        while let Some(request) = stream.try_next().await.context("Reading request")? {
            log::debug!(request:?; "");
            match request {
                fpartitions::PartitionsAdminRequest::ResetPartitionTable {
                    partitions,
                    responder,
                } => {
                    responder
                        .send(
                            self.reset_partition_table(partitions)
                                .await
                                .map_err(|status| status.into_raw()),
                        )
                        .unwrap_or_else(|e| log::error!(e:?; "Failed to send Start response"));
                }
            }
        }
        Ok(())
    }

    async fn reset_partition_table(
        &self,
        partitions: Vec<fpartitions::PartitionEntry>,
    ) -> Result<(), zx::Status> {
        fn convert_partition_entry(entry: fpartitions::PartitionEntry) -> gpt::PartitionInfo {
            gpt::PartitionInfo {
                label: entry.name,
                type_guid: gpt::Guid::from_bytes(entry.type_guid.value),
                instance_guid: gpt::Guid::from_bytes(entry.instance_guid.value),
                start_block: entry.start_block,
                num_blocks: entry.num_blocks,
                flags: entry.flags,
            }
        }
        let partitions = partitions.into_iter().map(convert_partition_entry).collect::<Vec<_>>();

        let mut state = self.state.lock().await;
        match &mut *state {
            State::Stopped => return Err(zx::Status::BAD_STATE),
            State::NeedsFormatting(config, block) => {
                log::info!("reset_partition_table: Reformatting GPT.");
                let client = Arc::new(RemoteBlockClient::new(block.clone()).await?);

                log::info!("reset_partition_table: Reformatting GPT...");
                gpt::Gpt::format(client, partitions).await.map_err(|err| {
                    log::error!(err:?; "reset_partition_table: failed to init GPT");
                    zx::Status::IO
                })?;
                *state = State::Running(
                    GptManager::new_with_config(
                        block.clone(),
                        self.partitions_dir.clone(),
                        config.clone(),
                    )
                    .await
                    .map_err(|err| {
                        log::error!(err:?; "reset_partition_table: failed to re-launch GPT");
                        zx::Status::BAD_STATE
                    })?,
                );
            }
            State::Running(gpt) => {
                log::info!("reset_partition_table: Updating GPT.");
                gpt.reset_partition_table(partitions).await?;
            }
        }
        Ok(())
    }

    async fn handle_admin_requests(
        &self,
        mut stream: ffs::AdminRequestStream,
    ) -> Result<(), Error> {
        if let Some(request) = stream.try_next().await.context("Reading request")? {
            match request {
                ffs::AdminRequest::Shutdown { responder } => {
                    log::info!("Received Admin::Shutdown request");
                    self.shutdown().await;
                    responder
                        .send()
                        .unwrap_or_else(|e| log::error!(e:?; "Failed to send shutdown response"));
                    log::info!("Admin shutdown complete");
                }
            }
        }
        Ok(())
    }

    async fn handle_lifecycle_requests(&self, lifecycle_channel: zx::Channel) -> Result<(), Error> {
        let mut stream = flifecycle::LifecycleRequestStream::from_channel(
            fasync::Channel::from_channel(lifecycle_channel),
        );
        match stream.try_next().await.context("Reading request")? {
            Some(flifecycle::LifecycleRequest::Stop { .. }) => {
                log::info!("Received Lifecycle::Stop request");
                self.shutdown().await;
                log::info!("Lifecycle shutdown complete");
            }
            None => {}
        }
        Ok(())
    }

    async fn gpt_manager(&self) -> Result<Arc<GptManager>, zx::Status> {
        match &*self.state.lock().await {
            State::Stopped | State::NeedsFormatting(_, _) => Err(zx::Status::BAD_STATE),
            State::Running(gpt) => Ok(gpt.clone()),
        }
    }

    async fn shutdown(&self) {
        let mut state = self.state.lock().await;
        if let State::Running(gpt) = std::mem::take(&mut *state) {
            gpt.shutdown().await;
        }
        self.scope.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::GptService;
    use block_client::RemoteBlockClient;
    use fidl::endpoints::Proxy as _;
    use fidl_fuchsia_fs as ffs;
    use fidl_fuchsia_fs_startup as fstartup;
    use fidl_fuchsia_io as fio;
    use fidl_fuchsia_process_lifecycle::LifecycleMarker;
    use fidl_fuchsia_storage_block as fblock;
    use fidl_fuchsia_storage_partitions as fpartitions;
    use fuchsia_async as fasync;
    use fuchsia_component::client::connect_to_protocol_at_dir_svc;
    use futures::FutureExt as _;
    use gpt::{Gpt, Guid, PartitionInfo};
    use std::sync::Arc;
    use test_vmo_backed_block_server::VmoBackedServer;

    async fn setup_server(
        block_size: u32,
        block_count: u64,
        partitions: Vec<PartitionInfo>,
    ) -> Arc<VmoBackedServer> {
        let vmo = zx::Vmo::create(block_size as u64 * block_count).unwrap();
        let server = Arc::new(
            VmoBackedServer::from_vmo(512, vmo).expect("Failed to create VmoBackedServer"),
        );
        {
            let (block_client, block_server) =
                fidl::endpoints::create_proxy::<fblock::BlockMarker>();
            let volume_stream = fidl::endpoints::ServerEnd::<fblock::BlockMarker>::from(
                block_server.into_channel(),
            )
            .into_stream();
            let server_clone = server.clone();
            let _task = fasync::Task::spawn(async move { server_clone.serve(volume_stream).await });
            let client = Arc::new(RemoteBlockClient::new(block_client).await.unwrap());
            Gpt::format(client, partitions).await.unwrap();
        }
        server
    }

    #[fuchsia::test]
    async fn lifecycle() {
        let (outgoing_dir, outgoing_dir_server) =
            fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        let (lifecycle_client, lifecycle_server) =
            fidl::endpoints::create_proxy::<LifecycleMarker>();
        let (block_client, block_server) =
            fidl::endpoints::create_endpoints::<fblock::BlockMarker>();
        let volume_stream =
            fidl::endpoints::ServerEnd::<fblock::BlockMarker>::from(block_server.into_channel())
                .into_stream();

        futures::join!(
            async {
                // Client
                let client =
                    connect_to_protocol_at_dir_svc::<fstartup::StartupMarker>(&outgoing_dir)
                        .unwrap();
                client
                    .start(block_client, &fstartup::StartOptions::default())
                    .await
                    .expect("FIDL error")
                    .expect("Start failed");
                lifecycle_client.stop().expect("Stop failed");
                fasync::OnSignals::new(
                    &lifecycle_client.into_channel().expect("into_channel failed"),
                    zx::Signals::CHANNEL_PEER_CLOSED,
                )
                .await
                .expect("OnSignals failed");
            },
            async {
                // Server
                let service = GptService::new();
                service
                    .run(outgoing_dir_server.into_channel(), Some(lifecycle_server.into_channel()))
                    .await
                    .expect("Run failed");
            },
            async {
                // Block device
                let server = setup_server(
                    512,
                    8,
                    vec![PartitionInfo {
                        label: "part".to_string(),
                        type_guid: Guid::from_bytes([0xabu8; 16]),
                        instance_guid: Guid::from_bytes([0xcdu8; 16]),
                        start_block: 4,
                        num_blocks: 1,
                        flags: 0,
                    }],
                )
                .await;
                let _ = server.serve(volume_stream).await;
            }
            .fuse(),
        );
    }

    #[fuchsia::test]
    async fn admin_shutdown() {
        let (outgoing_dir, outgoing_dir_server) =
            fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        let (block_client, block_server) =
            fidl::endpoints::create_endpoints::<fblock::BlockMarker>();
        let volume_stream =
            fidl::endpoints::ServerEnd::<fblock::BlockMarker>::from(block_server.into_channel())
                .into_stream();

        futures::join!(
            async {
                // Client
                let client =
                    connect_to_protocol_at_dir_svc::<fstartup::StartupMarker>(&outgoing_dir)
                        .unwrap();
                let admin_client =
                    connect_to_protocol_at_dir_svc::<ffs::AdminMarker>(&outgoing_dir).unwrap();
                client
                    .start(block_client, &fstartup::StartOptions::default())
                    .await
                    .expect("FIDL error")
                    .expect("Start failed");
                admin_client.shutdown().await.expect("admin shutdown failed");
                fasync::OnSignals::new(
                    &admin_client.into_channel().expect("into_channel failed"),
                    zx::Signals::CHANNEL_PEER_CLOSED,
                )
                .await
                .expect("OnSignals failed");
            },
            async {
                // Server
                let service = GptService::new();
                service.run(outgoing_dir_server.into_channel(), None).await.expect("Run failed");
            },
            async {
                // Block device
                let server = setup_server(
                    512,
                    8,
                    vec![PartitionInfo {
                        label: "part".to_string(),
                        type_guid: Guid::from_bytes([0xabu8; 16]),
                        instance_guid: Guid::from_bytes([0xcdu8; 16]),
                        start_block: 4,
                        num_blocks: 1,
                        flags: 0,
                    }],
                )
                .await;
                let _ = server.serve(volume_stream).await;
            }
            .fuse(),
        );
    }

    #[fuchsia::test]
    async fn transaction_lifecycle() {
        let (outgoing_dir, outgoing_dir_server) =
            fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        let (lifecycle_client, lifecycle_server) =
            fidl::endpoints::create_proxy::<LifecycleMarker>();
        let (block_client, block_server) =
            fidl::endpoints::create_endpoints::<fblock::BlockMarker>();
        let volume_stream =
            fidl::endpoints::ServerEnd::<fblock::BlockMarker>::from(block_server.into_channel())
                .into_stream();

        futures::join!(
            async {
                // Client
                connect_to_protocol_at_dir_svc::<fstartup::StartupMarker>(&outgoing_dir)
                    .unwrap()
                    .start(block_client, &fstartup::StartOptions::default())
                    .await
                    .expect("FIDL error")
                    .expect("Start failed");

                let pm_client = connect_to_protocol_at_dir_svc::<
                    fpartitions::PartitionsManagerMarker,
                >(&outgoing_dir)
                .unwrap();
                let transaction = pm_client
                    .create_transaction()
                    .await
                    .expect("FIDL error")
                    .expect("create_transaction failed");

                pm_client
                    .create_transaction()
                    .await
                    .expect("FIDL error")
                    .expect_err("create_transaction should fail while other txn exists");

                pm_client
                    .commit_transaction(transaction)
                    .await
                    .expect("FIDL error")
                    .expect("commit_transaction failed");

                {
                    let _transaction = pm_client
                        .create_transaction()
                        .await
                        .expect("FIDL error")
                        .expect("create_transaction should succeed after committing txn");
                }

                pm_client
                    .create_transaction()
                    .await
                    .expect("FIDL error")
                    .expect("create_transaction should succeed after dropping txn");

                lifecycle_client.stop().expect("Stop failed");
                fasync::OnSignals::new(
                    &lifecycle_client.into_channel().expect("into_channel failed"),
                    zx::Signals::CHANNEL_PEER_CLOSED,
                )
                .await
                .expect("OnSignals failed");
            },
            async {
                // Server
                let service = GptService::new();
                service
                    .run(outgoing_dir_server.into_channel(), Some(lifecycle_server.into_channel()))
                    .await
                    .expect("Run failed");
            },
            async {
                // Block device
                let server = setup_server(
                    512,
                    16,
                    vec![PartitionInfo {
                        label: "part".to_string(),
                        type_guid: Guid::from_bytes([0xabu8; 16]),
                        instance_guid: Guid::from_bytes([0xcdu8; 16]),
                        start_block: 4,
                        num_blocks: 1,
                        flags: 0,
                    }],
                )
                .await;
                let _ = server.serve(volume_stream).await;
            }
            .fuse(),
        );
    }
}
