// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{Disk, FatDirectory, FatFs, fatfs_error_to_status};
use anyhow::{Context, Error, bail};
use block_client::RemoteBlockClientSync;
use fidl::endpoints::{ClientEnd, DiscoverableProtocolMarker, RequestStream, ServerEnd};
use fidl_fuchsia_fs::{AdminMarker, AdminRequest, AdminRequestStream};
use fidl_fuchsia_fs_startup::{
    CheckOptions, FormatOptions, StartOptions, StartupMarker, StartupRequest, StartupRequestStream,
};
use fidl_fuchsia_io as fio;
use fidl_fuchsia_process_lifecycle::{LifecycleRequest, LifecycleRequestStream};
use fidl_fuchsia_storage_block::BlockMarker;
use fragile::Fragile;
use fuchsia_async as fasync;
use futures::TryStreamExt;
use log::{error, info, warn};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use vfs::directory::helper::DirectlyMutable;
use vfs::execution_scope::ExecutionScope;
use vfs::node::Node as _;

fn map_to_raw_status(e: Error) -> zx::sys::zx_status_t {
    map_to_status(e).into_raw()
}

fn map_to_status(error: Error) -> zx::Status {
    match error.downcast::<zx::Status>() {
        Ok(status) => status,
        Err(error) => match error.downcast::<std::io::Error>() {
            Ok(io_error) => fatfs_error_to_status(io_error),
            Err(error) => {
                // Print the internal error if we re-map it because we will lose any context after
                // this.
                warn!(error:?; "Internal error");
                zx::Status::INTERNAL
            }
        },
    }
}

enum State {
    ComponentStarted,
    Running(RunningState),
}

struct RunningState {
    // We have to wrap this in an Arc, even though it itself basically just wraps an Arc, so that
    // FsInspectTree can reference `fs` as a Weak<dyn FsInspect>`.
    fs: FatFs,
}

impl State {
    /// Disconnects the running filesystem by removing its "root" entry from the
    /// outgoing directory and calling `close` on it to decrement its reference count.
    /// Returns the `FatFs` instance if it was running.
    fn disconnect(&mut self, outgoing_dir: &vfs::directory::immutable::Simple) -> Option<FatFs> {
        if let State::Running(RunningState { fs }) =
            std::mem::replace(self, State::ComponentStarted)
        {
            if let Ok(Some(entry)) =
                outgoing_dir.remove_entry("root", /* must_be_directory: */ false)
            {
                let _ = entry.into_any().downcast::<FatDirectory>().unwrap().close();
            }
            Some(fs)
        } else {
            None
        }
    }
}

pub struct Component {
    state: RefCell<State>,

    /// The execution scope of the pseudo filesystem (data plane). All VFS connections
    /// and background flush tasks run on this scope.
    scope: ExecutionScope,

    /// The execution scope for admin and lifecycle services (control plane). This is
    /// kept separate from `scope` so that control connections (like Admin or Lifecycle)
    /// can remain active and process requests even while the filesystem is restarting
    /// and `scope` is being shut down.
    admin_scope: ExecutionScope,

    /// The root of the pseudo filesystem for the component.
    outgoing_dir: Arc<vfs::directory::immutable::Simple>,
}

impl Component {
    pub fn new() -> Rc<Self> {
        Rc::new(Self {
            state: RefCell::new(State::ComponentStarted),
            scope: ExecutionScope::new(),
            admin_scope: ExecutionScope::new(),
            outgoing_dir: vfs::directory::immutable::simple(),
        })
    }

    /// Runs Fatfs as a component.
    pub async fn run(
        self: Rc<Self>,
        outgoing_dir: zx::Channel,
        lifecycle_channel: Option<zx::Channel>,
    ) -> Result<(), Error> {
        let svc_dir = vfs::directory::immutable::simple();
        self.outgoing_dir.add_entry("svc", svc_dir.clone()).expect("Unable to create svc dir");
        let weak = Fragile::new(Rc::downgrade(&self));
        let weak_startup = weak.clone();
        svc_dir.add_entry(
            StartupMarker::PROTOCOL_NAME,
            vfs::service::endpoint(move |_scope, channel| {
                let weak = weak_startup.clone();
                let requests = StartupRequestStream::from_channel(channel);
                if let Some(me) = weak.get().upgrade() {
                    let weak_task = weak.clone();
                    me.admin_scope.spawn_local(async move {
                        if let Some(me) = weak_task.get().upgrade() {
                            let _ = me.handle_startup_requests(requests).await;
                        }
                    });
                }
            }),
        )?;

        let weak_admin = weak.clone();
        svc_dir.add_entry(
            AdminMarker::PROTOCOL_NAME,
            vfs::service::endpoint(move |_scope, channel| {
                let weak = weak_admin.clone();
                let requests = AdminRequestStream::from_channel(channel);
                if let Some(me) = weak.get().upgrade() {
                    let weak_task = weak.clone();
                    me.admin_scope.spawn_local(async move {
                        if let Some(me) = weak_task.get().upgrade() {
                            let _ = me.handle_admin_requests(requests).await;
                        }
                    });
                }
            }),
        )?;

        vfs::directory::serve_on(
            self.outgoing_dir.clone(),
            fio::PERM_READABLE | fio::PERM_WRITABLE,
            self.admin_scope.clone(),
            ServerEnd::new(outgoing_dir),
        );

        if let Some(channel) = lifecycle_channel {
            let weak = Fragile::new(Rc::downgrade(&self));
            self.admin_scope.spawn_local(async move {
                if let Some(me) = weak.get().upgrade() {
                    if let Err(error) = me.handle_lifecycle_requests(channel).await {
                        warn!(error:?; "handle_lifecycle_requests");
                    }
                }
            });
        }

        // Wait for the admin scope to finish first. Once the admin scope has finished,
        // no new VFS connections can be created (as the control/startup/admin channels
        // are hosted on it). If we waited for the VFS scope first, new connections
        // could be spawned on it while we were waiting.
        self.admin_scope.wait().await;
        self.scope.wait().await;
        self.stop_filesystem().await;

        Ok(())
    }

    async fn handle_startup_requests(&self, mut stream: StartupRequestStream) -> Result<(), Error> {
        while let Some(request) = stream.try_next().await? {
            match request {
                StartupRequest::Start { responder, device, options } => {
                    responder.send(self.handle_start(device, options).await.map_err(|e| {
                        error!(e:?; "handle_start failed");
                        map_to_raw_status(e)
                    }))?
                }
                StartupRequest::Format { responder, device, options } => {
                    responder.send(self.handle_format(device, options).await.map_err(|e| {
                        error!(e:?; "handle_format failed");
                        map_to_raw_status(e)
                    }))?
                }
                StartupRequest::Check { responder, device, options } => {
                    responder.send(self.handle_check(device, options).await.map_err(|e| {
                        error!(e:?; "handle_check failed");
                        map_to_raw_status(e)
                    }))?
                }
            }
        }
        Ok(())
    }

    async fn start_with_disk(&self, disk: Box<dyn Disk>) -> Result<(), Error> {
        self.stop_filesystem().await;

        // Resurrect the VFS scope so it can be reused to spawn connections for the new disk.
        self.scope.resurrect();

        let fs = FatFs::new(disk, self.scope.clone()).map_err(|_| zx::Status::IO)?;
        let root = fs.get_root()?;

        self.outgoing_dir.add_entry("root", root)?;

        *self.state.borrow_mut() = State::Running(RunningState { fs });

        Ok(())
    }

    async fn handle_start(
        &self,
        device: ClientEnd<BlockMarker>,
        options: StartOptions,
    ) -> Result<(), Error> {
        info!(options:?; "Received start request");

        let remote_block_client = RemoteBlockClientSync::new(device)?;
        let device = block_client::Cache::new(remote_block_client)?;

        self.start_with_disk(Box::new(device)).await?;

        info!("Mounted");
        Ok(())
    }

    async fn handle_format(
        &self,
        device: ClientEnd<BlockMarker>,
        options: FormatOptions,
    ) -> Result<(), Error> {
        let args: Box<dyn Iterator<Item = _> + Send> =
            if let Some(spc) = options.sectors_per_cluster {
                Box::new(["-c".to_string(), format!("{spc}")].into_iter())
            } else {
                Box::new(std::iter::empty())
            };
        if block_adapter::run(device.into_proxy(), "/pkg/bin/mkfs-msdosfs", args).await? == 0 {
            Ok(())
        } else {
            bail!(zx::Status::IO)
        }
    }

    async fn handle_check(
        &self,
        device: ClientEnd<BlockMarker>,
        _options: CheckOptions,
    ) -> Result<(), Error> {
        // Pass the '-n' flag so that it never modifies which remains consistent with other
        // filesystems.
        if block_adapter::run(
            device.into_proxy(),
            "/pkg/bin/fsck-msdosfs",
            ["-n".to_string()].into_iter(),
        )
        .await?
            == 0
        {
            Ok(())
        } else {
            bail!(zx::Status::IO)
        }
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
                info!("Received shutdown request");
                self.stop_filesystem().await;
                responder
                    .send()
                    .unwrap_or_else(|e| warn!("Failed to send shutdown response: {}", e));
                return Ok(true);
            }
        }
    }

    async fn stop_filesystem(&self) {
        info!("Stopping fatfs runtime; remaining connections will be forcibly closed");

        // Disconnect the filesystem's root directory from the outgoing directory first.
        // This prevents new connections from being established via the outgoing directory
        // while we are waiting for existing ones to drain.
        let maybe_fs = self.state.borrow_mut().disconnect(&self.outgoing_dir);

        self.scope.shutdown();
        self.scope.wait().await;

        // Cleanly shut down the filesystem. This is guaranteed to succeed (meaning
        // Rc::into_inner will succeed) because all connection references to it
        // have been dropped during scope wait.
        if let Some(fs) = maybe_fs {
            if let Err(error) = fs.shut_down() {
                error!(error:?; "Failed to shutdown fatfs");
            } else {
                info!("Filesystem terminated");
            }
        }
    }

    async fn handle_lifecycle_requests(&self, lifecycle_channel: zx::Channel) -> Result<(), Error> {
        let mut stream =
            LifecycleRequestStream::from_channel(fasync::Channel::from_channel(lifecycle_channel));
        match stream.try_next().await.context("Reading request")? {
            Some(LifecycleRequest::Stop { .. }) => {
                info!("Received Lifecycle::Stop request");
                self.stop_filesystem().await;
            }
            None => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl::endpoints::{Proxy, create_proxy};
    use fuchsia_async as fasync;

    #[fuchsia::test]
    async fn test_component_lifecycle_with_open_connections() {
        let component = Component::new();

        // Create a 2MB fatfs formatted in-memory disk.
        let mut buffer = vec![0u8; 2048 << 10];
        let cursor = std::io::Cursor::new(buffer.as_mut_slice());
        fatfs::format_volume(cursor, fatfs::FormatVolumeOptions::new()).unwrap();
        let disk: Box<dyn Disk> = Box::new(std::io::Cursor::new(buffer));

        component.start_with_disk(disk).await.unwrap();

        let (outgoing_dir_client, outgoing_dir_server) = create_proxy::<fio::DirectoryMarker>();
        let component_clone = component.clone();
        let run_task = fasync::Task::local(async move {
            component_clone.run(outgoing_dir_server.into_channel(), None).await.unwrap();
        });

        // Open the root directory via outgoing_dir.
        let (root_client, root_server) = create_proxy::<fio::NodeMarker>();
        outgoing_dir_client
            .open(
                "root",
                fio::PERM_READABLE | fio::PERM_WRITABLE,
                &Default::default(),
                root_server.into_channel(),
            )
            .unwrap();
        let root_client = fio::DirectoryProxy::new(root_client.into_channel().unwrap());

        // Guarantee "open" has been processed by making a round-trip call.
        let _ = root_client.query().await.unwrap();

        // Close outgoing_dir_client. admin_scope should become empty, but VFS scope still has root_client.
        drop(outgoing_dir_client);

        // Wait a bit and ensure the component is still running.
        use futures::FutureExt;
        let mut run_task = run_task.fuse();
        let mut delay = Box::pin(fasync::Timer::new(fasync::MonotonicInstant::after(
            zx::MonotonicDuration::from_millis(100),
        )))
        .fuse();
        futures::select! {
            _ = run_task => panic!("Component exited prematurely!"),
            _ = delay => {},
        }

        // Now close the root connection. The component should exit.
        drop(root_client);

        // Await run_task to ensure it exits.
        run_task.await;
    }

    #[fuchsia::test]
    async fn test_component_restart_with_open_connections() {
        let component = Component::new();

        // Helper to create a formatted disk.
        let create_disk = || {
            let mut buffer = vec![0u8; 2048 << 10];
            let cursor = std::io::Cursor::new(buffer.as_mut_slice());
            fatfs::format_volume(cursor, fatfs::FormatVolumeOptions::new()).unwrap();
            Box::new(std::io::Cursor::new(buffer)) as Box<dyn Disk>
        };

        // Start with disk 1.
        component.start_with_disk(create_disk()).await.unwrap();

        let (outgoing_dir_client, outgoing_dir_server) = create_proxy::<fio::DirectoryMarker>();
        let component_clone = component.clone();
        let _run_task = fasync::Task::local(async move {
            component_clone.run(outgoing_dir_server.into_channel(), None).await.unwrap();
        });

        // Open the root directory.
        let (root_client1, root_server1) = create_proxy::<fio::NodeMarker>();
        outgoing_dir_client
            .open(
                "root",
                fio::PERM_READABLE | fio::PERM_WRITABLE,
                &Default::default(),
                root_server1.into_channel(),
            )
            .unwrap();
        let root_client1 = fio::DirectoryProxy::new(root_client1.into_channel().unwrap());

        // Guarantee "open" has been processed.
        let _ = root_client1.query().await.unwrap();

        // Restart with disk 2. This should shut down the scope, closing root_client1.
        component.start_with_disk(create_disk()).await.unwrap();

        // Verify root_client1 is closed (calls to it should fail).
        assert!(root_client1.query().await.is_err());

        // We should be able to open root again, and it should work (talking to disk 2).
        let (root_client2, root_server2) = create_proxy::<fio::NodeMarker>();
        outgoing_dir_client
            .open(
                "root",
                fio::PERM_READABLE | fio::PERM_WRITABLE,
                &Default::default(),
                root_server2.into_channel(),
            )
            .unwrap();
        let root_client2 = fio::DirectoryProxy::new(root_client2.into_channel().unwrap());

        // This should succeed.
        let _ = root_client2.query().await.unwrap();

        // Cleanup: close connections and wait for run task to exit so disk 2 is shut down cleanly.
        drop(root_client2);
        drop(outgoing_dir_client);
        _run_task.await;
    }
}
