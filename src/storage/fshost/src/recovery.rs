// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::device::DeviceTag;
use crate::device::constants::{BLOB_IMAGE_VOLUME_LABEL, BLOB_VOLUME_LABEL};
use crate::environment::{Container, Environment, FvmContainer, FxfsContainer};
use anyhow::{Context, Error, anyhow, ensure};
use fidl::endpoints::{Proxy, ServerEnd};
use fidl_fuchsia_fs_startup::{CreateOptions, MountOptions};

use fidl_fuchsia_io::{self as fio, DirectoryMarker};
use fidl_fuchsia_storage_partitions as fpartitions;
use fs_management::filesystem::{BlockConnector, Filesystem as FsFilesystem};
use fs_management::format::{DiskFormat, detect_disk_format};
use fs_management::{Fvm, Fxfs};

use fuchsia_async::{self as fasync, TimeoutExt};
use fuchsia_component::client::connect_to_protocol_at_dir_root;
use futures::TryFutureExt;
use futures::lock::Mutex;
use std::sync::Arc;
use thiserror::Error;

const FIND_PARTITION_DURATION: zx::MonotonicDuration = zx::MonotonicDuration::from_seconds(20);
pub const IMAGE_FILE_NAME: &str = "blob.img";

#[derive(Debug, Error)]
#[error(transparent)]
pub struct FilesystemCorrupt(#[from] anyhow::Error);

pub struct RecoveryOps {
    env: Arc<Mutex<dyn Environment>>,
    registered_devices: Arc<crate::device::RegisteredDevices>,
    system_partition_lock: Arc<Mutex<()>>,
    config: Arc<fshost_config::Config>,
    launcher: Arc<crate::environment::FilesystemLauncher>,
    gpt_exposed_dir: fio::DirectoryProxy,
    scope: vfs::ExecutionScope,
}

impl RecoveryOps {
    pub fn new(
        env: Arc<Mutex<dyn Environment>>,
        registered_devices: Arc<crate::device::RegisteredDevices>,
        config: Arc<fshost_config::Config>,
        launcher: Arc<crate::environment::FilesystemLauncher>,
        gpt_exposed_dir: fio::DirectoryProxy,
        scope: vfs::ExecutionScope,
    ) -> Self {
        Self {
            env,
            registered_devices,
            system_partition_lock: Arc::new(Mutex::new(())),
            config,
            launcher,
            gpt_exposed_dir,
            scope,
        }
    }

    fn ensure_recovery_fxblob(&self, method: &str) -> Result<(), Error> {
        ensure!(self.config.ramdisk_image, "{} called in a non-Recovery build", method);
        ensure!(self.config.fxfs_blob, "{} requires a fxblob-based product", method);
        Ok(())
    }

    async fn get_system_container(
        &self,
    ) -> Result<(Box<dyn BlockConnector>, futures::lock::OwnedMutexGuard<()>), zx::Status> {
        let guard = self.system_partition_lock.clone().lock_owned().await;
        log::info!("Finding system container...");
        let block_connector = self
            .registered_devices
            .get_block_connector(DeviceTag::SystemContainerOnRecovery)
            .map_err(|error| {
                log::error!(error:?; "Unable to get block connector for system container");
                zx::Status::NOT_FOUND
            })
            .on_timeout(FIND_PARTITION_DURATION, || {
                log::warn!("Failed to find system container within timeout");
                Err(zx::Status::NOT_FOUND)
            })
            .await?;
        Ok((block_connector, guard))
    }

    pub async fn write_data_file(&self, filename: &str, payload: zx::Vmo) -> Result<(), Error> {
        ensure!(
            self.config.ramdisk_image,
            "Can't WriteDataFile from a non-recovery build; ramdisk_image must be set."
        );

        let (device, _guard) = self.get_system_container().await?;
        let mut container: Box<dyn Container> = if self.config.fxfs_blob {
            Box::new(FxfsContainer::new(
                self.launcher
                    .serve_fxblob(device, Box::new(Fxfs::dynamic_child()))
                    .await
                    .context("serving Fxblob")?,
            ))
        } else {
            Box::new(FvmContainer::new(
                self.launcher
                    .serve_fvm(device, Box::new(Fvm::dynamic_child()))
                    .await
                    .context("serving Fvm")?,
                false,
            ))
        };
        let mut data = container.serve_data(&self.launcher).await.context("serving data")?;
        let filesystem = container.into_fs();

        let data_root = data.root().context("Failed to get data root")?;
        let (directory_proxy, file_path) = match filename.rsplit_once("/") {
            Some((directory_path, relative_file_path)) => {
                let directory_proxy = fuchsia_fs::directory::create_directory_recursive(
                    &data_root,
                    directory_path,
                    fio::Flags::FLAG_MAYBE_CREATE | fio::PERM_READABLE | fio::PERM_WRITABLE,
                )
                .await
                .context("Failed to create directory")?;
                (directory_proxy, relative_file_path)
            }
            None => (data_root, filename),
        };

        let file_proxy = fuchsia_fs::directory::open_file(
            &directory_proxy,
            file_path,
            fio::Flags::FLAG_MAYBE_CREATE | fio::PERM_READABLE | fio::PERM_WRITABLE,
        )
        .await
        .context("Failed to open file")?;

        let content_size = payload
            .get_content_size()
            .or_else(|_| payload.get_size())
            .context("Failed to get content size")? as usize;
        let mut contents = vec![0; content_size];
        payload.read(&mut contents, 0).context("reading payload vmo")?;
        fuchsia_fs::file::write(&file_proxy, &contents).await.context("writing file contents")?;

        data.shutdown().await.context("shutting down data")?;
        filesystem.shutdown().await.context("shutting down filesystem")?;
        Ok(())
    }

    pub async fn shred_data_offline(&self) -> Result<(), zx::Status> {
        let (device, _guard) = self.get_system_container().await?;

        let format = detect_disk_format(
            &device
                .connect_block()
                .map_err(|error| {
                    log::error!(error:?; "connect_block failed");
                    zx::Status::INTERNAL
                })?
                .into_proxy(),
        )
        .await;

        if self.config.fxfs_blob {
            if format != DiskFormat::Fxfs {
                log::warn!(
                    format:?, expected_format:? = DiskFormat::Fxfs;
                    "found unexpected format - shred_data returning early",
                );
                return Ok(());
            }

            let fxfs = FsFilesystem::from_boxed_config(device, Box::new(Fxfs::dynamic_child()));
            let serving_fxfs = fxfs.serve_multi_volume().await.map_err(|error| {
                log::error!(error:?; "Failed to serve fxfs");
                zx::Status::INTERNAL
            })?;
            let mut fxfs_container = Box::new(FxfsContainer::new(serving_fxfs));
            fxfs_container.shred_data().await.map_err(|error| {
                log::error!(error:?; "Failed to shred fxfs keybag");
                zx::Status::INTERNAL
            })?;
            fxfs_container.into_fs().shutdown().await.map_err(|error| {
                log::error!(error:?; "Failed to unmount fxfs");
                zx::Status::INTERNAL
            })?;
            log::info!("Deleted fxfs-data keybag");
        } else if self.config.data_filesystem_format == "minfs" {
            if format != DiskFormat::Fvm {
                log::warn!(
                    format:?, expected_format:? = DiskFormat::Fvm;
                    "found unexpected format - shred_data returning early",
                );
                return Ok(());
            }

            let fvm = fs_management::filesystem::Filesystem::from_boxed_config(
                device,
                Box::new(Fvm::dynamic_child()),
            );
            let serving_fvm = fvm.serve_multi_volume().await.map_err(|error| {
                log::error!(error:?; "Failed to serve fvm");
                zx::Status::INTERNAL
            })?;
            let mut fvm_container = Box::new(FvmContainer::new(serving_fvm, false));
            fvm_container.shred_data().await.map_err(|error| {
                log::error!(error:?; "Failed to call shred data on fvm component");
                zx::Status::INTERNAL
            })?;
            fvm_container.into_fs().shutdown().await.map_err(|error| {
                log::error!(error:?; "Failed to unmount fvm");
                zx::Status::INTERNAL
            })?;
            log::info!("Shredded zxcrypt instances in fvm");
        } else {
            log::warn!(format:?; "found unexpected format - shred_data returning early");
        }
        Ok(())
    }

    pub async fn shred_data(&self) -> Result<(), zx::Status> {
        let is_online = (self.config.data || self.config.fxfs_blob) && !self.config.ramdisk_image;

        if self.config.data_filesystem_format != "fxfs" && self.config.no_zxcrypt {
            return Err(zx::Status::NOT_SUPPORTED);
        }

        if is_online {
            log::info!("Filesystem is running; shredding online.");
            self.env.lock().await.shred_data_online().await.map_err(|error| {
                log::error!(error:?; "Failed to shred data");
                zx::Status::INTERNAL
            })
        } else {
            log::info!("Filesystem is not running; shredding offline.");
            self.shred_data_offline().await
        }
    }

    pub async fn init_system_partition_table(
        &self,
        partitions: Vec<fpartitions::PartitionEntry>,
    ) -> Result<(), zx::Status> {
        if !self.config.ramdisk_image {
            log::error!("init_system_partition_table only supported in ramdisk-image mode.");
            return Err(zx::Status::NOT_SUPPORTED);
        }
        if !self.config.gpt {
            log::error!("init_system_partition_table called on a non-gpt system.");
            return Err(zx::Status::NOT_SUPPORTED);
        }

        let _guard = self.system_partition_lock.clone().lock_owned().await;
        const TIMEOUT: zx::MonotonicDuration = zx::MonotonicDuration::from_seconds(10);
        let _ = self
            .registered_devices
            .get_block_connector(DeviceTag::SystemPartitionTable)
            .map_err(|error| {
                log::error!(error:?; "init_system_partition_table: unable to get block connector");
                zx::Status::NOT_FOUND
            })
            .on_timeout(TIMEOUT, || {
                log::error!("init_system_partition_table: Failed to find gpt within timeout");
                Err(zx::Status::NOT_FOUND)
            })
            .await?;

        log::info!("init_system_partition_table: Reformatting GPT...");
        let client = connect_to_protocol_at_dir_root::<fpartitions::PartitionsAdminMarker>(
            &self.gpt_exposed_dir,
        )
        .map_err(|e| {
            log::error!(e:?; "failed to connect to partitions admin");
            zx::Status::INTERNAL
        })?;
        client
            .reset_partition_table(&partitions[..])
            .await
            .map_err(|err| {
                log::error!(err:?; "init_system_partition_table: FIDL error");
                zx::Status::PEER_CLOSED
            })?
            .map_err(zx::Status::err_from_raw)
    }

    pub async fn format_system_blob_volume(&self) -> Result<(), Error> {
        self.ensure_recovery_fxblob("format_system_blob_volume")?;

        let (device, _guard) = self.get_system_container().await?;

        let fxfs = FsFilesystem::from_boxed_config(device, Box::new(Fxfs::default()));
        let serving_fxfs = fxfs.serve_multi_volume().await.context("serving fxfs")?;

        if serving_fxfs.has_volume(BLOB_VOLUME_LABEL).await.context("checking for blob volume")? {
            log::info!("Removing existing blob volume.");
            serving_fxfs.remove_volume(BLOB_VOLUME_LABEL).await.context("removing blob volume")?;
        }
        log::info!("Creating new blob volume.");
        let blob_volume = serving_fxfs
            .create_volume(
                BLOB_VOLUME_LABEL,
                CreateOptions::default(),
                MountOptions { as_blob: Some(true), ..Default::default() },
            )
            .await
            .context("creating blob volume")?;

        blob_volume.shutdown().await.context("unmounting blob volume")?;
        serving_fxfs.shutdown().await.context("unmounting fxfs")?;
        Ok(())
    }

    pub async fn mount_system_blob_volume(
        &self,
        blob_exposed_dir: ServerEnd<DirectoryMarker>,
    ) -> Result<(), Error> {
        self.ensure_recovery_fxblob("mount_system_blob_volume")?;
        let (device, guard) = self.get_system_container().await?;

        log::info!("Mounting Fxfs.");
        let fxfs = FsFilesystem::from_boxed_config(device, Box::new(Fxfs::default()));
        let serving_fxfs = fxfs.serve_multi_volume().await.context("serving fxfs")?;

        log::info!("Mounting blob volume.");
        if !serving_fxfs.has_volume(BLOB_VOLUME_LABEL).await.context("checking for blob volume")? {
            log::error!(
                "Blob volume missing! Use fuchsia.fshost/Recovery.FormatSystemBlobVolume \
                to create one."
            );
            return Err(zx::Status::NOT_FOUND).context("missing blob volume");
        }
        let blob_volume = serving_fxfs
            .open_volume(
                BLOB_VOLUME_LABEL,
                MountOptions { as_blob: Some(true), ..Default::default() },
            )
            .await
            .context("mounting blob volume")?;

        let blob_root = fuchsia_fs::directory::clone(blob_volume.root())?;
        let blob_svc = fuchsia_fs::directory::open_directory(
            blob_volume.exposed_dir(),
            "svc",
            fio::PERM_READABLE,
        )
        .await?;

        let exposed_dir = vfs::pseudo_directory! {
            "root" => vfs::remote::remote_dir(blob_root),
            "svc" => vfs::remote::remote_dir(blob_svc),
        };

        let scope = vfs::ExecutionScope::new();
        vfs::directory::serve_on(
            exposed_dir,
            fio::PERM_READABLE | fio::PERM_WRITABLE | fio::PERM_EXECUTABLE,
            scope.clone(),
            blob_exposed_dir,
        );

        self.scope.spawn(async move {
            let _guard = guard;
            scope.wait().await;
            log::info!("All handles to system container closed, unmounting...");
            if let Err(e) = blob_volume.shutdown().await {
                log::error!("Failed to unmount blob volume: {e:?}");
            }
            if let Err(e) = serving_fxfs.shutdown().await {
                log::error!("Failed to shutdown fxfs: {e:?}");
            }
            log::info!("System container unmounted.");
        });

        Ok(())
    }

    pub async fn get_blob_image_handle(
        &self,
    ) -> Result<fidl_fuchsia_fshost::RecoveryGetBlobImageHandleResponse, Error> {
        self.ensure_recovery_fxblob("get_blob_image_handle")?;
        let (device, guard) = self.get_system_container().await?;

        const EXPECTED_FORMAT: DiskFormat = DiskFormat::Fxfs;
        let format = detect_disk_format(
            &device.connect_block().context("connect_block failed")?.into_proxy(),
        )
        .await;
        if format != EXPECTED_FORMAT {
            log::warn!(
                "wrong system container format (expected = {EXPECTED_FORMAT:?}, \
                detected = {format:?})"
            );
            return Ok(fidl_fuchsia_fshost::RecoveryGetBlobImageHandleResponse::Unformatted(
                fidl_fuchsia_fshost::Unformatted {},
            ));
        }

        let fxfs = FsFilesystem::from_boxed_config(device, Box::new(Fxfs::default()));
        let serving_fxfs =
            fxfs.serve_multi_volume().await.context("serving fxfs").map_err(FilesystemCorrupt)?;

        if serving_fxfs
            .has_volume(BLOB_IMAGE_VOLUME_LABEL)
            .await
            .context("checking for blob image volume")?
        {
            serving_fxfs
                .remove_volume(BLOB_IMAGE_VOLUME_LABEL)
                .await
                .context("deleting existing blob image volume")
                .map_err(FilesystemCorrupt)?
        }

        let pending = serving_fxfs
            .create_volume(BLOB_IMAGE_VOLUME_LABEL, Default::default(), Default::default())
            .await
            .context("creating blob image volume")
            .map_err(FilesystemCorrupt)?;

        let image_file = fuchsia_fs::directory::open_file(
            pending.root(),
            IMAGE_FILE_NAME,
            fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_FILE
                | fio::Flags::FLAG_MAYBE_CREATE
                | fio::Flags::FILE_TRUNCATE,
        )
        .await
        .context("opening/creating image file")
        .map_err(FilesystemCorrupt)?
        .into_client_end()
        .map_err(|_| anyhow!("failed to get client end for image handle"))?;

        let (our_mount_token, mount_token) = zx::EventPair::create();

        self.scope.spawn(async move {
            let _guard = guard;
            if let Err(error) =
                fasync::OnSignals::new(our_mount_token, zx::Signals::OBJECT_PEER_CLOSED).await
            {
                log::error!(error:?; "failed to wait for unmount token, unmounting");
            } else {
                log::info!("Unmount token closed, unmounting system container.");
            }
            if let Err(error) = pending.shutdown().await {
                log::error!(error:?; "failed to unmount blob image volume");
            }
            if let Err(error) = serving_fxfs.shutdown().await {
                log::error!(error:?; "failed to shutdown fxfs");
            }
            log::info!("System container unmounted.");
        });

        Ok(fidl_fuchsia_fshost::RecoveryGetBlobImageHandleResponse::MountedSystemContainer(
            fidl_fuchsia_fshost::MountedSystemContainer { image_file, mount_token },
        ))
    }

    pub async fn install_blob_image_offline(&self) -> Result<(), Error> {
        self.ensure_recovery_fxblob("install_blob_image")?;
        let (device, _guard) = self.get_system_container().await?;

        let fxfs = FsFilesystem::from_boxed_config(device, Box::new(Fxfs::default()));
        let serving_fxfs = fxfs.serve_multi_volume().await.context("serving fxfs")?;

        if !serving_fxfs
            .has_volume(BLOB_IMAGE_VOLUME_LABEL)
            .await
            .context("checking for image volume")?
        {
            log::error!(
                "blob image missing, use fuchsia.fshost/Recovery.GetBlobImageHandle to write one"
            );
            return Err(zx::Status::NOT_FOUND).context("missing blob image volume");
        }

        let res = crate::environment::install_blob_image(&serving_fxfs).await;
        serving_fxfs.shutdown().await.context("unmounting fxfs")?;
        res
    }
}
