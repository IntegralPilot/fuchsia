// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, anyhow, format_err};
use fidl::endpoints::{ClientEnd, DiscoverableProtocolMarker, RequestStream, create_proxy};
use fidl_fuchsia_device::ControllerMarker;
use fidl_fuchsia_process_lifecycle::{LifecycleRequest, LifecycleRequestStream};
use fidl_fuchsia_storage_block::BlockMarker;
use fs_management::filesystem::{Filesystem, ServingSingleVolumeFilesystem};
use fs_management::format::{DiskFormat, detect_disk_format};
use fs_management::{Blobfs, F2fs, Fxfs, Minfs};
use fuchsia_runtime::{HandleType, take_startup_handle};
use futures::channel::mpsc;
use futures::lock::Mutex;
use futures::{SinkExt, StreamExt, TryStreamExt};
use std::collections::HashMap;
use std::sync::Arc;
use vfs::directory::helper::DirectlyMutable;
use vfs::directory::immutable::Simple as PseudoDirectory;
use {fidl_fuchsia_fs_realm as fs_realm, fidl_fuchsia_io as fio, fuchsia_async as fasync};

#[derive(Clone)]
pub struct FsRealmState {
    running_filesystems: Arc<Mutex<HashMap<String, ServingSingleVolumeFilesystem>>>,
    mnt: Arc<PseudoDirectory>,
}

impl FsRealmState {
    fn new() -> (Arc<PseudoDirectory>, Self) {
        let running_filesystems = Arc::new(Mutex::new(HashMap::new()));
        let mnt = vfs::pseudo_directory! {};
        (mnt.clone(), FsRealmState { running_filesystems, mnt })
    }

    async fn mount(
        &self,
        mount_name: &str,
        device: ClientEnd<ControllerMarker>,
        options: fs_realm::MountOptions,
    ) -> Result<(), Error> {
        let mut locked_running_filesystems = self.running_filesystems.lock().await;
        // Check that `mount_name` is not already in `running_filesystems`.
        if locked_running_filesystems.contains_key(mount_name) {
            return Err(anyhow!("a filesystem is already mounted at {mount_name}"));
        }

        let device = device.into_proxy();
        let (block_proxy, block_server) = create_proxy::<BlockMarker>();
        device.connect_to_device_fidl(block_server.into_channel())?;
        let format = detect_disk_format(&block_proxy).await;
        let filesystem = match format {
            DiskFormat::Blobfs => {
                let blobfs = Blobfs {
                    verbose: options.verbose.unwrap_or(false),
                    readonly: options.read_only.unwrap_or(false),
                    ..Default::default()
                };
                Filesystem::new(device, blobfs)
            }
            DiskFormat::F2fs => Filesystem::new(device, F2fs::default()),
            DiskFormat::Minfs => {
                let minfs = Minfs {
                    verbose: options.verbose.unwrap_or(false),
                    readonly: options.read_only.unwrap_or(false),
                    ..Default::default()
                };
                Filesystem::new(device, minfs)
            }
            _ => {
                return Err(anyhow!(
                    "Invalid detected disk format: mount only supports blobfs, minfs, and f2fs"
                ));
            }
        };
        let fs = filesystem.serve().await?;
        let node = fuchsia_fs::directory::clone(fs.root())?;
        self.mnt.add_entry(mount_name, vfs::remote::remote_dir(node))?;
        locked_running_filesystems.insert(mount_name.to_string(), fs);
        Ok(())
    }

    async fn unmount(&self, mount_name: &str) -> Result<(), Error> {
        let mut locked_running_filesystems = self.running_filesystems.lock().await;
        if let Some(fs) = locked_running_filesystems.remove(mount_name) {
            self.mnt.remove_entry(mount_name, true)?;
            fs.shutdown().await?;
            Ok(())
        } else {
            Err(anyhow!("Invalid mount path: no filesystem was mounted at {:?}", mount_name))
        }
    }

    async fn shutdown(&self) -> Result<(), Error> {
        let mut locked_running_filesystems = self.running_filesystems.lock().await;
        for (_mount_name, fs) in locked_running_filesystems.drain() {
            fs.shutdown().await?;
        }
        Ok(())
    }
}

async fn format(
    name: &str,
    device: ClientEnd<ControllerMarker>,
    options: fs_realm::FormatOptions,
) -> Result<(), Error> {
    let device = device.into_proxy();
    let mut filesystem = match name.as_ref() {
        "blobfs" => {
            let blobfs = Blobfs { verbose: options.verbose.unwrap_or(false), ..Default::default() };
            Filesystem::new(device, blobfs)
        }
        "fxfs" => Filesystem::new(device, Fxfs::default()),
        "f2fs" => Filesystem::new(device, F2fs::default()),
        "minfs" => {
            let minfs = Minfs {
                verbose: options.verbose.unwrap_or(false),
                fvm_data_slices: options.fvm_data_slices.unwrap_or(0),
                ..Default::default()
            };
            Filesystem::new(device, minfs)
        }
        _ => {
            return Err(anyhow!(
                "Invalid disk format: mkfs only supports blobfs, minfs, fxfs, and f2fs"
            ));
        }
    };
    filesystem.format().await
}

async fn check(name: &str, device: ClientEnd<ControllerMarker>) -> Result<(), Error> {
    let device = device.into_proxy();
    let mut filesystem = match name.as_ref() {
        "blobfs" => Filesystem::new(device, Blobfs::default()),
        "fxfs" => Filesystem::new(device, Fxfs::default()),
        "f2fs" => Filesystem::new(device, F2fs::default()),
        "minfs" => Filesystem::new(device, Minfs::default()),
        _ => {
            return Err(anyhow!(
                "Invalid disk format: fsck only supports blobfs, fxfs, f2fs, and minfs"
            ));
        }
    };
    filesystem.fsck().await
}

pub fn fs_realm_service(fs_realm_state: FsRealmState) -> Arc<vfs::service::Service> {
    vfs::service::host(move |mut stream: fs_realm::ControllerRequestStream| {
        let fs_realm_state = fs_realm_state.clone();
        async move {
            while let Some(request) = stream.next().await {
                match request {
                    Ok(fs_realm::ControllerRequest::Mount { responder, name, device, options }) => {
                        let res = match fs_realm_state.mount(&name, device, options).await {
                            Ok(()) => Ok(()),
                            Err(error) => {
                                log::error!(error:?; "mount failed");
                                Err(zx::Status::INTERNAL.into_raw())
                            }
                        };
                        responder.send(res).unwrap_or_else(|error| {
                            log::error!(error:?; "failed to send Mount response");
                        });
                    }
                    Ok(fs_realm::ControllerRequest::Unmount { responder, name }) => {
                        let res = match fs_realm_state.unmount(&name).await {
                            Ok(()) => Ok(()),
                            Err(error) => {
                                log::error!(error:?; "unmount failed");
                                Err(zx::Status::INTERNAL.into_raw())
                            }
                        };
                        responder.send(res).unwrap_or_else(|error| {
                            log::error!(error:?; "failed to send Unmount response");
                        });
                    }
                    Ok(fs_realm::ControllerRequest::Format {
                        responder,
                        name,
                        device,
                        options,
                    }) => {
                        let res = match format(&name, device, options).await {
                            Ok(()) => Ok(()),
                            Err(error) => {
                                log::error!(error:?; "format failed");
                                Err(zx::Status::INTERNAL.into_raw())
                            }
                        };
                        responder.send(res).unwrap_or_else(|error| {
                            log::error!(error:?; "failed to send Format response");
                        });
                    }
                    Ok(fs_realm::ControllerRequest::Check { responder, name, device }) => {
                        let res = match check(&name, device).await {
                            Ok(()) => Ok(()),
                            Err(error) => {
                                log::error!(error:?; "check failed");
                                Err(zx::Status::INTERNAL.into_raw())
                            }
                        };
                        responder.send(res).unwrap_or_else(|error| {
                            log::error!(error:?; "failed to send Check response");
                        });
                    }
                    Err(error) => {
                        log::error!(error:?; "fs_realm server failed");
                        return;
                    }
                }
            }
        }
    })
}

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    let (mnt, fs_realm_state) = FsRealmState::new();
    let directory_request =
        take_startup_handle(HandleType::DirectoryRequest.into()).ok_or_else(|| {
            format_err!("missing DirectoryRequest startup handle - not launched as a component?")
        })?;

    let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<LifecycleRequestStream>(1);
    let svc = vfs::pseudo_directory! {
        fs_realm::ControllerMarker::PROTOCOL_NAME =>
            fs_realm_service(fs_realm_state.clone())
    };

    let export = vfs::pseudo_directory! {
        "svc" => svc,
        "mnt" => mnt,
    };
    log::info!("Serving lifecycle requests");
    let _ = handle_lifecycle_requests(shutdown_tx)?;

    let scope = vfs::execution_scope::ExecutionScope::new();
    vfs::directory::serve_on(
        export,
        fio::PERM_READABLE | fio::PERM_WRITABLE,
        scope.clone(),
        fidl::endpoints::ServerEnd::new(directory_request.into()),
    );
    log::info!("Waiting for shutdown request");
    // Once _lifecycle_request_stream goes out of scope, the LifecycleRequestStream will
    // automatically get dropped, thereby closing the lifecycle channel and signaling
    // that shutdown is complete.
    let _lifecycle_request_stream = shutdown_rx
        .next()
        .await
        .ok_or_else(|| format_err!("shutdown signal stream ended unexpectedly"))?;

    log::info!("shutdown signal received");
    scope.shutdown();
    fs_realm_state.shutdown().await?;
    Ok(())
}

pub fn handle_lifecycle_requests(
    mut shutdown: mpsc::Sender<LifecycleRequestStream>,
) -> Result<(), Error> {
    if let Some(handle) = fuchsia_runtime::take_startup_handle(HandleType::Lifecycle.into()) {
        let mut stream =
            LifecycleRequestStream::from_channel(fasync::Channel::from_channel(handle.into()));
        fasync::Task::spawn(async move {
            match stream.try_next().await {
                Ok(Some(LifecycleRequest::Stop { .. })) => {
                    shutdown.send(stream).await.unwrap_or_else(
                        |error| log::error!(error:?; "failed to send shutdown message"),
                    );
                }
                Ok(None) => {
                    log::info!("next item on the LifecycleRequestStream is None");
                }
                Err(error) => {
                    log::error!(error:?; "failed to get the next lifecycle request");
                }
            }
        })
        .detach();
    }
    Ok(())
}
