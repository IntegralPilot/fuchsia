// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::fuchsia::debug::{create_debug_directory, handle_debug_request};
use crate::fuchsia::errors::map_to_status;
use crate::fuchsia::memory_pressure::MemoryPressureMonitor;
use crate::fuchsia::power::FuchsiaPowerManager;
use crate::fuchsia::volume::MemoryPressureConfig;
use crate::fuchsia::volumes_directory::VolumesDirectory;
use anyhow::{Context, Error, bail};
use async_trait::async_trait;
use block_client::{BlockClient as _, RemoteBlockClient};
use fidl::endpoints::{ClientEnd, DiscoverableProtocolMarker, RequestStream};
use fidl_fuchsia_fs::{AdminMarker, AdminRequest, AdminRequestStream};
use fidl_fuchsia_fs_startup::{
    CheckOptions, StartOptions, StartupMarker, StartupRequest, StartupRequestStream, VolumesMarker,
    VolumesRequest, VolumesRequestStream,
};
use fidl_fuchsia_fxfs::{
    DebugMarker, DebugRequestStream, VolumeInstallerMarker, VolumeInstallerRequest,
    VolumeInstallerRequestStream,
};
use fidl_fuchsia_io as fio;
use fidl_fuchsia_memory_attribution as fattribution;
use fidl_fuchsia_process_lifecycle::{LifecycleRequest, LifecycleRequestStream};
use fidl_fuchsia_storage_block::BlockMarker;
use fs_inspect::{FsInspect, FsInspectTree, InfoData, UsageData};
use fuchsia_async as fasync;
use fuchsia_component_client::connect_to_protocol;
use futures::TryStreamExt;
use futures::lock::Mutex;
use fxfs::filesystem::{FxFilesystem, FxFilesystemBuilder, MIN_BLOCK_SIZE, OpenFxFilesystem, mkfs};
use fxfs::log::*;
use fxfs::object_store::volume::root_volume;
use fxfs::serialized_types::LATEST_VERSION;
use fxfs::{fsck, metrics};
use fxfs_trace::{TraceFutureExt, trace_future_args};
use refaults_vmo::PageRefaultCounter;
use std::ops::Deref;
use std::sync::{Arc, Weak};
use storage_device::DeviceHolder;
use storage_device::block_device::BlockDevice;
use storage_units::PAGE_SIZE;
use vfs::directory::helper::DirectlyMutable;
use vfs::execution_scope::ExecutionScope;

const FXFS_INFO_NAME: &'static str = "fxfs";

pub fn map_to_raw_status(e: Error) -> zx::sys::zx_status_t {
    map_to_status(e).into_raw()
}

pub async fn new_block_client(remote: ClientEnd<BlockMarker>) -> Result<RemoteBlockClient, Error> {
    Ok(RemoteBlockClient::new(remote.into_proxy()).await?)
}

/// Runs Fxfs as a component.
pub struct Component {
    state: futures::lock::Mutex<State>,

    // The execution scope of the pseudo filesystem.
    scope: ExecutionScope,

    // The root of the pseudo filesystem for the component.
    export_dir: Arc<vfs::directory::immutable::Simple>,
}

// Wrapper type to add `FsInspect` support to an `OpenFxFilesystem`.
struct InspectedFxFilesystem(OpenFxFilesystem, /*fs_id=*/ u64);

impl From<OpenFxFilesystem> for InspectedFxFilesystem {
    fn from(fs: OpenFxFilesystem) -> Self {
        Self(fs, zx::Event::create().koid().unwrap().raw_koid())
    }
}

impl Deref for InspectedFxFilesystem {
    type Target = Arc<FxFilesystem>;
    fn deref(&self) -> &Self::Target {
        &self.0.deref()
    }
}

#[async_trait]
impl FsInspect for InspectedFxFilesystem {
    fn get_info_data(&self) -> InfoData {
        let earliest_version = self.0.super_block_header().earliest_version;
        InfoData {
            id: self.1,
            fs_type: fidl_fuchsia_fs::VfsType::Fxfs.into_primitive().into(),
            name: FXFS_INFO_NAME.into(),
            version_major: LATEST_VERSION.major.into(),
            version_minor: LATEST_VERSION.minor.into(),
            block_size: self.0.block_size().get(),
            max_filename_length: fio::MAX_NAME_LENGTH,
            oldest_version: Some(format!("{}.{}", earliest_version.major, earliest_version.minor)),
        }
    }

    async fn get_usage_data(&self) -> UsageData {
        let info = self.0.get_info();
        UsageData {
            total_bytes: info.total_bytes,
            used_bytes: info.used_bytes,
            // TODO(https://fxbug.dev/42175930): Should these be moved to per-volume nodes?
            total_nodes: 0,
            used_nodes: 0,
        }
    }
}

struct RunningState {
    // We have to wrap this in an Arc, even though it itself basically just wraps an Arc, so that
    // FsInspectTree can reference `fs` as a Weak<dyn FsInspect>`.
    fs: Arc<InspectedFxFilesystem>,
    volumes: Arc<VolumesDirectory>,
    _inspect_tree: Arc<FsInspectTree>,
}

enum State {
    ComponentStarted,
    Running(RunningState),
}

impl State {
    async fn stop(&mut self, outgoing_dir: &vfs::directory::immutable::Simple) {
        if let State::Running(RunningState { fs, volumes, .. }) =
            std::mem::replace(self, State::ComponentStarted)
        {
            info!("Stopping Fxfs runtime; remaining connections will be forcibly closed");
            let _ = outgoing_dir.remove_entry("volumes", /* must_be_directory: */ false);
            let _ = outgoing_dir.remove_entry("debug", /* must_be_directory: */ false);
            volumes.terminate().await;
            let _ = fs.deref().close().await;
        }
    }
}

impl Component {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State::ComponentStarted),
            scope: ExecutionScope::new(),
            export_dir: vfs::directory::immutable::simple(),
        })
    }

    /// Runs Fxfs as a component.
    pub async fn run(
        self: Arc<Self>,
        outgoing_dir: zx::Channel,
        lifecycle_channel: Option<zx::Channel>,
    ) -> Result<(), Error> {
        let svc_dir = vfs::directory::immutable::simple();
        self.export_dir.add_entry("svc", svc_dir.clone()).expect("Unable to create svc dir");

        let weak = Arc::downgrade(&self);
        svc_dir.add_entry(
            StartupMarker::PROTOCOL_NAME,
            vfs::service::host(move |requests| {
                let weak = weak.clone();
                async move {
                    if let Some(me) = weak.upgrade() {
                        let _ = me.handle_startup_requests(requests).await;
                    }
                }
            }),
        )?;
        let weak = Arc::downgrade(&self);
        svc_dir.add_entry(
            VolumesMarker::PROTOCOL_NAME,
            vfs::service::host(move |requests| {
                let weak = weak.clone();
                async move {
                    if let Some(me) = weak.upgrade() {
                        me.handle_volumes_requests(requests).await;
                    }
                }
            }),
        )?;

        let weak = Arc::downgrade(&self);
        svc_dir.add_entry(
            AdminMarker::PROTOCOL_NAME,
            vfs::service::host(move |requests| {
                let weak = weak.clone();
                async move {
                    if let Some(me) = weak.upgrade() {
                        let _ = me.handle_admin_requests(requests).await;
                    }
                }
            }),
        )?;

        let weak = Arc::downgrade(&self);
        svc_dir.add_entry(
            VolumeInstallerMarker::PROTOCOL_NAME,
            vfs::service::host(move |requests| {
                let weak = weak.clone();
                async move {
                    if let Some(me) = weak.upgrade() {
                        if let Err(error) = me.handle_volume_installer_requests(requests).await {
                            error!(error:?; "failed to handle VolumeInstaller requests");
                        }
                    }
                }
            }),
        )?;

        // TODO(b/315704445): Only enable in debug builds?
        let weak = Arc::downgrade(&self);
        svc_dir.add_entry(
            DebugMarker::PROTOCOL_NAME,
            vfs::service::host(move |requests| {
                let weak = weak.clone();
                async move {
                    if let Some(me) = weak.upgrade() {
                        let _ = me.handle_debug_requests(requests).await;
                    }
                }
            }),
        )?;

        vfs::directory::serve_on(
            self.export_dir.clone(),
            fio::PERM_READABLE | fio::PERM_WRITABLE | fio::PERM_EXECUTABLE,
            self.scope.clone(),
            fidl::endpoints::ServerEnd::new(outgoing_dir),
        );

        if let Some(channel) = lifecycle_channel {
            let me = self.clone();
            self.scope.spawn(async move {
                if let Err(error) = me.handle_lifecycle_requests(channel).await {
                    warn!(error:?; "handle_lifecycle_requests");
                }
            });
        }

        self.scope.wait().await;

        Ok(())
    }

    async fn handle_startup_requests(&self, mut stream: StartupRequestStream) -> Result<(), Error> {
        while let Some(request) = stream.try_next().await? {
            match request {
                StartupRequest::Start { responder, device, options } => {
                    async move {
                        responder.send(self.handle_start(device, options).await.map_err(|error| {
                            error!(error:?; "handle_start failed");
                            map_to_raw_status(error)
                        }))
                    }
                    .trace(trace_future_args!("Startup::Start"))
                    .await?
                }
                StartupRequest::Format { responder, device, .. } => {
                    async move {
                        responder.send(self.handle_format(device).await.map_err(|error| {
                            error!(error:?; "handle_format failed");
                            map_to_raw_status(error)
                        }))
                    }
                    .trace(trace_future_args!("Startup::Format"))
                    .await?
                }
                StartupRequest::Check { responder, device, options } => {
                    async move {
                        responder.send(self.handle_check(device, options).await.map_err(|error| {
                            error!(error:?; "handle_check failed");
                            map_to_raw_status(error)
                        }))
                    }
                    .trace(trace_future_args!("Startup::Check"))
                    .await?
                }
            }
        }
        Ok(())
    }

    async fn handle_start(
        &self,
        device: ClientEnd<BlockMarker>,
        options: StartOptions,
    ) -> Result<(), Error> {
        info!(options:?; "Received start request");
        let mut state = self.state.lock().await;
        // TODO(https://fxbug.dev/42174810): This is not very graceful.  It would be better for the client to
        // explicitly shut down all volumes first, and make this fail if there are remaining active
        // connections.  Fix the bug in fs_test which requires this.
        state.stop(&self.export_dir).await;
        let client = new_block_client(device).await?;

        // TODO(https://fxbug.dev/42063349) Add support for block sizes greater than the page size.
        assert!(client.block_size() <= PAGE_SIZE.get() as u32);
        assert!(PAGE_SIZE == MIN_BLOCK_SIZE);

        let fs = FxFilesystemBuilder::new()
            .fsck_after_every_transaction(options.fsck_after_every_transaction.unwrap_or(false))
            .read_only(options.read_only.unwrap_or(false))
            .inline_crypto_enabled(options.inline_crypto_enabled.unwrap_or(false))
            .barriers_enabled(options.barriers_enabled.unwrap_or(false))
            .allow_type3_blobs(options.allow_type3_blobs.unwrap_or(false))
            .power_manager(FuchsiaPowerManager::new())
            .open(DeviceHolder::new(
                BlockDevice::new(client, options.read_only.unwrap_or(false)).await?,
            ))
            .await?;
        let root_volume = root_volume(fs.clone()).await?;
        let fs: Arc<InspectedFxFilesystem> = Arc::new(fs.into());
        let weak_fs = Arc::downgrade(&fs) as Weak<dyn FsInspect + Send + Sync>;
        let inspect_tree =
            Arc::new(FsInspectTree::new(weak_fs, fuchsia_inspect::component::inspector().root()));

        let blob_resupplied_count = Arc::new(PageRefaultCounter::new()?);
        connect_to_protocol::<fattribution::PageRefaultSinkMarker>()?
            .send_page_refault_count(blob_resupplied_count.readonly_vmo()?)?;

        let mem_monitor = match MemoryPressureMonitor::start() {
            Ok(v) => Some(v),
            Err(error) => {
                warn!(
                    error:?;
                    "Failed to connect to memory pressure monitor. Running \
                     without pressure awareness."
                );
                None
            }
        };

        let volumes = VolumesDirectory::new(
            root_volume,
            Arc::downgrade(&inspect_tree),
            mem_monitor,
            blob_resupplied_count,
            MemoryPressureConfig::default(),
        )
        .await?;

        self.export_dir.add_entry_may_overwrite(
            "volumes",
            volumes.directory_node().clone(),
            /* overwrite: */ true,
        )?;

        let debug = create_debug_directory(&**fs, &volumes);
        self.export_dir.add_entry_may_overwrite("debug", debug, true)?;

        fs.allocator().track_statistics(&metrics::detail(), "allocator");
        fs.journal().track_statistics(&metrics::detail(), "journal");
        fs.object_manager().track_statistics(&metrics::detail(), "object_manager");

        let info = fs.get_info();
        info!(
            device_size = info.total_bytes,
            used = info.used_bytes,
            free = info.total_bytes - info.used_bytes;
            "Mounted"
        );

        if let Some(profile_time) = options.startup_profiling_seconds {
            // Unwrap ok, shouldn't have anything else recording or replaying this early in startup.
            volumes
                .record_and_replay_profile(None, ".boot".to_owned(), profile_time)
                .await
                .unwrap();
        }

        *state = State::Running(RunningState { fs, volumes, _inspect_tree: inspect_tree });

        Ok(())
    }

    async fn handle_format(&self, device: ClientEnd<BlockMarker>) -> Result<(), Error> {
        let device = DeviceHolder::new(
            BlockDevice::new(new_block_client(device).await?, /* read_only: */ false).await?,
        );
        mkfs(device).await?;
        info!("Formatted filesystem");
        Ok(())
    }

    async fn handle_check(
        &self,
        device: ClientEnd<BlockMarker>,
        _options: CheckOptions,
    ) -> Result<(), Error> {
        let state = self.state.lock().await;
        let (fs_container, fs) = match *state {
            State::ComponentStarted => {
                let client = new_block_client(device).await?;
                let fs_container = FxFilesystemBuilder::new()
                    .read_only(true)
                    .open(DeviceHolder::new(BlockDevice::new(client, /* read_only: */ true).await?))
                    .await?;
                let fs = fs_container.clone();
                (Some(fs_container), fs)
            }
            State::Running(RunningState { ref fs, .. }) => (None, fs.deref().deref().clone()),
        };
        let res = fsck::fsck(fs.clone()).await;
        if let Some(fs_container) = fs_container {
            let _ = fs_container.close().await;
        }
        // TODO(b/311550633): Stash ok res in inspect.
        info!("handle_check for fs: {:?}", res?);
        Ok(())
    }

    async fn handle_admin_requests(&self, mut stream: AdminRequestStream) -> Result<(), Error> {
        while let Some(request) = stream.try_next().await.context("Reading request")? {
            if self.handle_admin(request).await? {
                break;
            }
        }
        Ok(())
    }

    // Returns true if we should close the connection.
    async fn handle_admin(&self, req: AdminRequest) -> Result<bool, Error> {
        match req {
            AdminRequest::Shutdown { responder } => {
                async move {
                    info!("Received shutdown request");
                    self.shutdown().await;
                    responder
                        .send()
                        .unwrap_or_else(|error| warn!(error:?; "Failed to send shutdown response"));
                }
                .trace(trace_future_args!("Admin::Shutdown"))
                .await;
                return Ok(true);
            }
        }
    }

    /// Handles fuchsia.fxfs.Debug requests, providing live debugging internals of the running
    /// filesystem.
    async fn handle_debug_requests(&self, mut stream: DebugRequestStream) -> Result<(), Error> {
        while let Some(request) = stream.try_next().await.context("Reading request")? {
            let state = self.state.lock().await;
            let (fs, volumes) = match &*state {
                State::ComponentStarted => {
                    info!("Debug commands are not valid unless component is started.");
                    bail!("Component not started");
                }
                State::Running(RunningState { fs, volumes, .. }) => {
                    (fs.deref().deref().clone(), volumes.clone())
                }
            };
            handle_debug_request(fs, volumes, request).await?;
        }
        Ok(())
    }

    async fn shutdown(&self) {
        self.state.lock().await.stop(&self.export_dir).await;
        info!("Filesystem terminated");
    }

    async fn handle_volumes_requests(&self, mut stream: VolumesRequestStream) {
        let volumes =
            if let State::Running(RunningState { volumes, .. }) = &*self.state.lock().await {
                volumes.clone()
            } else {
                let _ = stream.into_inner().0.shutdown_with_epitaph(zx::Status::BAD_STATE);
                return;
            };
        while let Ok(Some(request)) = stream.try_next().await {
            match request {
                VolumesRequest::Create {
                    name,
                    outgoing_directory,
                    create_options,
                    mount_options,
                    responder,
                } => {
                    async {
                        info!(
                            name = name.as_str();
                            "Create {}volume",
                            if mount_options.crypt.is_some() { "encrypted " } else { "" }
                        );
                        responder
                            .send(
                                volumes
                                    .create_and_serve_volume(
                                        &name,
                                        outgoing_directory.into_channel().into(),
                                        mount_options,
                                        create_options,
                                    )
                                    .await
                                    .map_err(map_to_raw_status),
                            )
                            .unwrap_or_else(
                                |error| warn!(error:?; "Failed to send volume creation response"),
                            );
                    }
                    .trace(trace_future_args!("Volumes::Create"))
                    .await;
                }
                VolumesRequest::Remove { name, responder } => {
                    async {
                        info!(name = name.as_str(); "Remove volume");
                        responder
                            .send(volumes.remove_volume(&name).await.map_err(map_to_raw_status))
                            .unwrap_or_else(
                                |error| warn!(error:?; "Failed to send volume removal response"),
                            );
                    }
                    .trace(trace_future_args!("Volumes::Remove"))
                    .await;
                }
                VolumesRequest::GetInfo { responder } => {
                    async move {
                        responder.send(Err(zx::Status::NOT_SUPPORTED.into_raw())).unwrap_or_else(
                            |error| warn!(error:?; "Failed to send volume removal response"),
                        )
                    }
                    .trace(trace_future_args!("Volumes::GetInfo"))
                    .await;
                }
            }
        }
    }

    async fn handle_lifecycle_requests(&self, lifecycle_channel: zx::Channel) -> Result<(), Error> {
        let mut stream =
            LifecycleRequestStream::from_channel(fasync::Channel::from_channel(lifecycle_channel));
        match stream.try_next().await.context("Reading request")? {
            Some(LifecycleRequest::Stop { .. }) => {
                info!("Received Lifecycle::Stop request");
                self.shutdown().await;
            }
            None => {}
        }
        Ok(())
    }

    async fn handle_volume_installer_requests(
        &self,
        mut stream: VolumeInstallerRequestStream,
    ) -> Result<(), Error> {
        let volumes =
            if let State::Running(RunningState { volumes, .. }) = &*self.state.lock().await {
                volumes.clone()
            } else {
                let _ = stream.into_inner().0.shutdown_with_epitaph(zx::Status::BAD_STATE);
                return Err(zx::Status::BAD_STATE).context("fxfs component not running");
            };
        while let Some(request) = stream.try_next().await.context("reading request")? {
            match request {
                VolumeInstallerRequest::Install { src, image_file, dst, responder } => {
                    async {
                        let response = volumes.install_volume(&src, &image_file, &dst).await;
                        responder.send(response.map_err(|error| {
                            error!(error:?; "install failed");
                            map_to_raw_status(error)
                        }))
                    }
                    .trace(trace_future_args!("VolumeInstaller::Install"))
                    .await?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Component, new_block_client};
    use fidl::endpoints::Proxy;
    use fidl_fuchsia_fs::AdminMarker;
    use fidl_fuchsia_fs_startup::{
        CreateOptions, MountOptions, StartOptions, StartupMarker, VolumesMarker,
    };
    use fidl_fuchsia_fxfs::DebugMarker;
    use fidl_fuchsia_io as fio;
    use fidl_fuchsia_process_lifecycle::{LifecycleMarker, LifecycleProxy};
    use fuchsia_async as fasync;
    use fuchsia_component_client::connect_to_protocol_at_dir_svc;
    use fuchsia_fs::directory::readdir;
    use futures::future::{BoxFuture, FusedFuture, FutureExt};
    use futures::{pin_mut, select};
    use fxfs::filesystem::FxFilesystem;
    use fxfs::object_store::volume::root_volume;
    use fxfs::object_store::{NewChildStoreOptions, StoreOptions};
    use std::pin::Pin;
    use std::sync::Arc;
    use storage_device::DeviceHolder;
    use storage_device::block_device::BlockDevice;
    use test_vmo_backed_block_server::VmoBackedServer;

    async fn run_test(
        callback: impl Fn(&fio::DirectoryProxy, LifecycleProxy) -> BoxFuture<'static, ()>,
    ) -> (Pin<Box<impl FusedFuture>>, Arc<VmoBackedServer>) {
        const BLOCK_SIZE: u32 = 512;
        let block_server = Arc::new(
            VmoBackedServer::new(16384, BLOCK_SIZE, &[]).expect("Failed to create VmoBackedServer"),
        );

        {
            let fs = FxFilesystem::new_empty(DeviceHolder::new(
                BlockDevice::new(
                    new_block_client(block_server.connect())
                        .await
                        .expect("Unable to create block client"),
                    false,
                )
                .await
                .unwrap(),
            ))
            .await
            .expect("FxFilesystem::new_empty failed");
            {
                let root_volume = root_volume(fs.clone()).await.expect("Open root_volume failed");
                root_volume
                    .new_volume("default", NewChildStoreOptions::default())
                    .await
                    .expect("Create volume failed");
            }
            fs.close().await.expect("close failed");
        }

        let (client_end, server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();

        let (lifecycle_client, lifecycle_server) =
            fidl::endpoints::create_proxy::<LifecycleMarker>();

        let mut component_task = Box::pin(
            async {
                Component::new()
                    .run(server_end.into_channel(), Some(lifecycle_server.into_channel()))
                    .await
                    .expect("Failed to run component");
            }
            .fuse(),
        );

        let startup_proxy = connect_to_protocol_at_dir_svc::<StartupMarker>(&client_end)
            .expect("Unable to connect to Startup protocol");
        let block_server_connection = block_server.connect();
        let task = async {
            startup_proxy
                .start(block_server_connection, &StartOptions::default())
                .await
                .expect("Start failed (FIDL)")
                .expect("Start failed");
            callback(&client_end, lifecycle_client).await;
        }
        .fuse();

        pin_mut!(task);

        loop {
            select! {
                () = component_task => {},
                () = task => break,
            }
        }

        (component_task, block_server)
    }

    #[fuchsia::test(threads = 2)]
    async fn test_clear_caches() {
        let (component_task, _block_server) = run_test(|client, _| {
            let debug_proxy = connect_to_protocol_at_dir_svc::<DebugMarker>(client)
                .expect("Unable to connect to Debug protocol");
            let admin_proxy = connect_to_protocol_at_dir_svc::<AdminMarker>(client)
                .expect("Unable to connect to Admin protocol");
            async move {
                debug_proxy
                    .clear_caches()
                    .await
                    .expect("clear_caches failed (FIDL)")
                    .expect("clear_caches failed");
                admin_proxy.shutdown().await.expect("shutdown failed");
            }
            .boxed()
        })
        .await;
        component_task.await;
    }

    #[fuchsia::test(threads = 2)]
    async fn test_shutdown() {
        let (component_task, _block_server) = run_test(|client, _| {
            let admin_proxy = connect_to_protocol_at_dir_svc::<AdminMarker>(client)
                .expect("Unable to connect to Admin protocol");
            async move {
                admin_proxy.shutdown().await.expect("shutdown failed");
            }
            .boxed()
        })
        .await;
        assert!(!component_task.is_terminated());
    }

    #[fuchsia::test(threads = 2)]
    async fn test_lifecycle_stop() {
        let (component_task, _block_server) = run_test(|_, lifecycle_client| {
            lifecycle_client.stop().expect("Stop failed");
            async move {
                fasync::OnSignals::new(
                    &lifecycle_client.into_channel().expect("into_channel failed"),
                    zx::Signals::CHANNEL_PEER_CLOSED,
                )
                .await
                .expect("OnSignals failed");
            }
            .boxed()
        })
        .await;
        component_task.await;
    }

    #[fuchsia::test(threads = 2)]
    async fn test_create_and_remove() {
        let (component_task, _block_server) = run_test(|client, _| {
            let volumes_proxy = connect_to_protocol_at_dir_svc::<VolumesMarker>(client)
                .expect("Unable to connect to Volumes protocol");

            let fs_admin_proxy = connect_to_protocol_at_dir_svc::<AdminMarker>(client)
                .expect("Unable to connect to Admin protocol");

            async move {
                let (dir_proxy, server_end) =
                    fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
                volumes_proxy
                    .create("test", server_end, CreateOptions::default(), MountOptions::default())
                    .await
                    .expect("fidl failed")
                    .expect("create failed");

                // This should fail whilst the volume is mounted.
                volumes_proxy
                    .remove("test")
                    .await
                    .expect("fidl failed")
                    .expect_err("remove succeeded");

                let volume_admin_proxy = connect_to_protocol_at_dir_svc::<AdminMarker>(&dir_proxy)
                    .expect("Unable to connect to Admin protocol");
                volume_admin_proxy.shutdown().await.expect("shutdown failed");

                // Creating another volume with the same name should fail.
                let (_dir_proxy, server_end) =
                    fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
                volumes_proxy
                    .create("test", server_end, CreateOptions::default(), MountOptions::default())
                    .await
                    .expect("fidl failed")
                    .expect_err("create succeeded");

                volumes_proxy.remove("test").await.expect("fidl failed").expect("remove failed");

                // Removing a non-existent volume should fail.
                volumes_proxy
                    .remove("test")
                    .await
                    .expect("fidl failed")
                    .expect_err("remove failed");

                // Create the same volume again and it should now succeed.
                let (_dir_proxy, server_end) =
                    fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
                volumes_proxy
                    .create("test", server_end, CreateOptions::default(), MountOptions::default())
                    .await
                    .expect("fidl failed")
                    .expect("create failed");

                fs_admin_proxy.shutdown().await.expect("shutdown failed");
            }
            .boxed()
        })
        .await;
        component_task.await;
    }

    #[fuchsia::test(threads = 2)]
    async fn test_volumes_enumeration() {
        let (component_task, _block_server) = run_test(|client, _| {
            let volumes_proxy = connect_to_protocol_at_dir_svc::<VolumesMarker>(client)
                .expect("Unable to connect to Volumes protocol");

            let (volumes_dir_proxy, server_end) =
                fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
            client
                .open("volumes", fio::PERM_READABLE, &Default::default(), server_end.into_channel())
                .expect("open failed");

            let fs_admin_proxy = connect_to_protocol_at_dir_svc::<AdminMarker>(client)
                .expect("Unable to connect to Admin protocol");

            async move {
                let (_dir_proxy, server_end) =
                    fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
                volumes_proxy
                    .create("test", server_end, CreateOptions::default(), MountOptions::default())
                    .await
                    .expect("fidl failed")
                    .expect("create failed");

                let entries = readdir(&volumes_dir_proxy).await.expect("readdir failed");
                let mut entry_names = entries.iter().map(|d| d.name.as_str()).collect::<Vec<_>>();
                entry_names.sort();
                assert_eq!(entry_names, ["default", "test"]);

                fs_admin_proxy.shutdown().await.expect("shutdown failed");
            }
            .boxed()
        })
        .await;
        component_task.await;
    }

    #[fuchsia::test(threads = 2)]
    async fn test_create_with_guid() {
        let (component_task, block_server) = run_test(|client, _| {
            let volumes_proxy = connect_to_protocol_at_dir_svc::<VolumesMarker>(client)
                .expect("Unable to connect to Volumes protocol");
            let admin_proxy = connect_to_protocol_at_dir_svc::<AdminMarker>(client)
                .expect("Unable to connect to Admin protocol");
            async move {
                let (_dir_proxy, server_end) =
                    fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
                let guid = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
                volumes_proxy
                    .create(
                        "test_guid",
                        server_end,
                        CreateOptions { guid: Some(guid), ..Default::default() },
                        MountOptions::default(),
                    )
                    .await
                    .expect("fidl failed")
                    .expect("create failed");
                admin_proxy.shutdown().await.expect("shutdown failed");
            }
            .boxed()
        })
        .await;
        component_task.await;

        // Verify the GUID
        let fs = FxFilesystem::open(DeviceHolder::new(
            BlockDevice::new(
                new_block_client(block_server.connect())
                    .await
                    .expect("Unable to create block client"),
                false,
            )
            .await
            .unwrap(),
        ))
        .await
        .expect("FxFilesystem::open failed");
        {
            let root_volume = root_volume(fs.clone()).await.expect("Open root_volume failed");
            let vol = root_volume
                .volume("test_guid", StoreOptions::default())
                .await
                .expect("Open volume failed");
            assert_eq!(vol.guid(), [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]);
        }
        fs.close().await.expect("close failed");
    }
}
