// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::environment::Environment;
use crate::recovery::{FilesystemCorrupt, RecoveryOps};
use anyhow::{Context, Error, anyhow};
use fidl::endpoints::{ClientEnd, RequestStream, ServerEnd};
use fidl_fuchsia_fs_startup::{CheckOptions, CreateOptions, MountOptions};
use fidl_fuchsia_fshost as fshost;
use fidl_fuchsia_fxfs as ffxfs;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_process_lifecycle::{LifecycleRequest, LifecycleRequestStream};

use fshost::{AdminRequest, RecoveryRequest};
use fuchsia_async as fasync;

use fuchsia_runtime::HandleType;
use futures::channel::mpsc;
use futures::lock::Mutex;
use futures::{StreamExt, TryStreamExt};
use std::sync::Arc;
use vfs::service;

pub enum FshostShutdownResponder {
    Lifecycle(
        // TODO(https://fxbug.dev/333319162): Implement me.
        #[allow(dead_code)] LifecycleRequestStream,
    ),
    Crash(&'static str),
}

impl FshostShutdownResponder {
    pub fn close(self) -> Result<(), fidl::Error> {
        match self {
            FshostShutdownResponder::Lifecycle(_) => {}
            FshostShutdownResponder::Crash(_) => {}
        }
        Ok(())
    }
}

const STARNIX_TEST_VOLUME_NAME: &str = "starnix_test_volume";

async fn check_main_starnix_volume(
    environment: &Arc<Mutex<dyn Environment>>,
    volume_name: &str,
    crypt: ClientEnd<ffxfs::CryptMarker>,
) -> Result<(), zx::Status> {
    let mut env = environment.lock().await;
    if let Some(multi_vol_fs) = env.get_container() {
        if multi_vol_fs.has_volume(volume_name).await.map_err(|err| {
            log::error!(err:?; "has_volume failed");
            zx::Status::INTERNAL
        })? {
            multi_vol_fs
                .check_volume(
                    volume_name,
                    CheckOptions { crypt: Some(crypt), ..CheckOptions::default() },
                )
                .await
                .map_err(|err| {
                    env.report_corruption("fxfs", &err);
                    zx::Status::IO_DATA_INTEGRITY
                })
        } else {
            Ok(())
        }
    } else {
        Err(zx::Status::BAD_STATE)
    }
}

async fn check_starnix_volume(
    environment: &Arc<Mutex<dyn Environment>>,
    config: &fshost_config::Config,
    crypt: ClientEnd<ffxfs::CryptMarker>,
) -> Result<(), zx::Status> {
    if config.starnix_volume_name.is_empty() || !config.check_filesystems {
        Ok(())
    } else {
        check_main_starnix_volume(environment, &config.starnix_volume_name, crypt).await
    }
}

async fn open_or_create_starnix_volume(
    environment: &Arc<Mutex<dyn Environment>>,
    volume_name: &str,
    crypt: ClientEnd<ffxfs::CryptMarker>,
    exposed_dir: ServerEnd<fio::DirectoryMarker>,
    recreate: bool,
) -> Result<[u8; 16], Error> {
    let mut env = environment.lock().await;
    let multi_vol_fs = env
        .get_container()
        .ok_or_else(|| anyhow!("Tried to mount starnix volume without container set"))?;

    let mount_options = MountOptions { crypt: Some(crypt), ..MountOptions::default() };
    let mounted_vol = if recreate {
        if multi_vol_fs.has_volume(volume_name).await? {
            log::info!(volume_name:%; "Recreating starnix volume");
            multi_vol_fs.remove_volume(volume_name).await?;
        }
        multi_vol_fs
            .create_volume(
                volume_name,
                CreateOptions { restrict_inode_ids_to_32_bit: Some(true), ..Default::default() },
                mount_options,
            )
            .await?
    } else {
        if multi_vol_fs.has_volume(volume_name).await? {
            multi_vol_fs.open_volume(volume_name, mount_options).await?
        } else {
            multi_vol_fs.create_volume(volume_name, CreateOptions::default(), mount_options).await?
        }
    };

    mounted_vol.exposed_dir().clone(exposed_dir.into_channel().into())?;
    multi_vol_fs
        .get_volume_info(volume_name)
        .await
        .context("get_volume_info")?
        .guid
        .ok_or_else(|| anyhow!("No GUID returned"))
}

pub fn fshost_volume_provider(
    environment: Arc<Mutex<dyn Environment>>,
    config: Arc<fshost_config::Config>,
) -> Arc<service::Service> {
    service::host(move |mut stream: fshost::StarnixVolumeProviderRequestStream| {
        let env = environment.clone();
        let config = config.clone();
        async move {
            while let Some(request) = stream.next().await {
                match request {
                    Ok(fshost::StarnixVolumeProviderRequest::Check { crypt, responder }) => {
                        log::info!("volume provider check");
                        let res = check_starnix_volume(&env, &config, crypt).await;
                        responder.send(res.map_err(|e| e.into_raw())).unwrap_or_else(|error| {
                            log::error!(error:?; "failed to send fidl response");
                        });
                    }
                    Ok(fshost::StarnixVolumeProviderRequest::Mount {
                        crypt,
                        mode,
                        exposed_dir,
                        responder,
                    }) => {
                        log::info!(mode:?; "volume provider mount");
                        let volume_name = if config.starnix_volume_name.is_empty() {
                            STARNIX_TEST_VOLUME_NAME
                        } else {
                            &config.starnix_volume_name
                        };
                        let recreate = mode == fshost::MountMode::AlwaysCreate
                            || config.starnix_volume_name.is_empty();
                        let res = open_or_create_starnix_volume(
                            &env,
                            volume_name,
                            crypt,
                            exposed_dir,
                            recreate,
                        )
                        .await;
                        let res = match res {
                            Ok(guid) => Ok(guid),
                            Err(error) => {
                                log::error!(error:?; "volume provider service: mount failed");
                                Err(zx::Status::INTERNAL.into_raw())
                            }
                        };
                        responder.send(res.as_ref().map_err(|e| *e)).unwrap_or_else(|error| {
                            log::error!(error:?; "failed to send fidl response");
                        });
                    }
                    Err(e) => {
                        log::error!("volume provider server failed: {:?}", e);
                        return;
                    }
                }
            }
        }
    })
}

/// Make a new vfs service node that implements fuchsia.fshost.Admin
pub fn fshost_admin(ops: Arc<RecoveryOps>) -> Arc<service::Service> {
    service::host(move |mut stream: fshost::AdminRequestStream| {
        let ops = ops.clone();
        async move {
            while let Some(request) = stream.next().await {
                match request {
                    Ok(AdminRequest::ShredDataVolume { responder }) => {
                        log::info!("admin shred data volume called");
                        let res = ops.shred_data().await;
                        let res = match res {
                            Ok(()) => Ok(()),
                            Err(status) => {
                                if status == zx::Status::NOT_SUPPORTED {
                                    log::warn!("shred_data_volume not supported");
                                } else {
                                    log::error!(status:?; "shred_data_volume failed");
                                }
                                Err(status.into_raw())
                            }
                        };
                        responder.send(res).unwrap_or_else(|error| {
                            log::error!(error:?; "failed to send fidl response");
                        });
                    }
                    Err(e) => {
                        log::error!("admin service failed: {:?}", e);
                        return;
                    }
                }
            }
        }
    })
}

pub fn fshost_recovery(ops: Arc<RecoveryOps>) -> Arc<service::Service> {
    service::host(move |mut stream: fshost::RecoveryRequestStream| {
        let ops = ops.clone();
        async move {
            while let Some(request) = stream.next().await {
                match request {
                    Ok(RecoveryRequest::WriteDataFile { responder, payload, filename }) => {
                        log::info!(filename:?; "recovery write data file called");
                        let res = ops.write_data_file(&filename, payload).await.map_err(|error| {
                            log::error!(error:?; "write_data_file failed");
                            zx::Status::INTERNAL.into_raw()
                        });
                        responder.send(res).unwrap_or_else(|error| {
                            log::error!(error:?; "failed to send fidl response");
                        });
                    }
                    Ok(RecoveryRequest::InitSystemPartitionTable { partitions, responder }) => {
                        log::info!("recovery init gpt called");
                        let res =
                            ops.init_system_partition_table(partitions).await.map_err(|error| {
                                log::error!(error:?; "init_system_partition_table failed");
                                error.into_raw()
                            });
                        responder.send(res).unwrap_or_else(|error| {
                            log::error!(error:?; "failed to send fidl response");
                        });
                    }
                    Ok(RecoveryRequest::FormatSystemBlobVolume { responder }) => {
                        log::info!("formatting system blob volume");
                        let res = ops.format_system_blob_volume().await.map_err(|error| {
                            log::error!(error:?; "format_system_blob_volume failed");
                            error
                                .downcast::<zx::Status>()
                                .unwrap_or(zx::Status::INTERNAL)
                                .into_raw()
                        });
                        responder.send(res).unwrap_or_else(|error| {
                            log::error!(error:?; "failed to send fidl response");
                        });
                    }
                    Ok(RecoveryRequest::MountSystemBlobVolume { blob_exposed_dir, responder }) => {
                        log::info!("mounting system blob volume");
                        let res =
                            ops.mount_system_blob_volume(blob_exposed_dir).await.map_err(|error| {
                                log::error!(error:?; "mount_system_blob_volume failed");
                                error
                                    .downcast::<zx::Status>()
                                    .unwrap_or(zx::Status::INTERNAL)
                                    .into_raw()
                            });
                        responder.send(res).unwrap_or_else(|error| {
                            log::error!(error:?; "failed to send fidl response");
                        });
                    }
                    Ok(RecoveryRequest::GetBlobImageHandle { responder }) => {
                        log::info!("getting image handle for new system blob volume");
                        let res = ops.get_blob_image_handle().await.map_err(|error| {
                            if error.is::<FilesystemCorrupt>() {
                                log::error!(
                                    error:?;
                                    "Filesystem may be corrupt and requires a full re-flash."
                                );
                                zx::Status::IO_DATA_INTEGRITY.into_raw()
                            } else {
                                log::error!(error:?; "get_blob_image_handle failed");
                                zx::Status::INTERNAL.into_raw()
                            }
                        });
                        responder.send(res).unwrap_or_else(|error| {
                            log::error!(error:?; "failed to send fidl response");
                        });
                    }
                    Ok(RecoveryRequest::InstallBlobImage { responder }) => {
                        log::info!("installing system blob volume");
                        let res = ops.install_blob_image_offline().await.map_err(|error| {
                            log::error!(error:?; "failed to install blob image");
                            error
                                .downcast::<zx::Status>()
                                .unwrap_or(zx::Status::INTERNAL)
                                .into_raw()
                        });
                        responder.send(res).unwrap_or_else(|error| {
                            log::error!(error:?; "failed to send fidl response");
                        });
                    }
                    Err(e) => {
                        log::error!("recovery service failed: {:?}", e);
                        return;
                    }
                }
            }
        }
    })
}

pub fn handle_lifecycle_requests(
    mut shutdown: mpsc::Sender<FshostShutdownResponder>,
) -> Result<(), Error> {
    if let Some(handle) = fuchsia_runtime::take_startup_handle(HandleType::Lifecycle.into()) {
        let mut stream =
            LifecycleRequestStream::from_channel(fasync::Channel::from_channel(handle.into()));
        fasync::Task::spawn(async move {
            if let Ok(Some(LifecycleRequest::Stop { .. })) = stream.try_next().await {
                if let Err(e) = shutdown.try_send(FshostShutdownResponder::Lifecycle(stream)) {
                    log::error!("failed to send shutdown message. error: {:?}", e);
                }
            }
        })
        .detach();
    }
    Ok(())
}
