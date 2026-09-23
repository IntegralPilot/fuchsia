// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod fvm_container;
mod fxfs_container;
mod publisher;
use fuchsia_component::client::connect::{
    connect_to_named_protocol_at_dir_root, connect_to_protocol_at_dir_root,
};
pub use fvm_container::FvmContainer;
pub use fxfs_container::FxfsContainer;
pub use publisher::{DevicePublisher, SinglePublisher};

use crate::crypt::fxfs::CryptService;
use crate::device::constants::DATA_VOLUME_LABEL;
use crate::device::{Device, RegisteredDevices};
use crate::service;
use crate::watcher::{DirSource, Watcher};
use anyhow::{Context, Error, anyhow, bail};
use async_trait::async_trait;
use fidl::endpoints::{
    DiscoverableProtocolMarker, Proxy, ServerEnd, ServiceMarker as _, create_proxy,
};
use fidl_fuchsia_fs_startup::{MountOptions, VolumesProxy};
use fidl_fuchsia_fshost_fxfsprovisioner as ffxfsprovisioner;
use fidl_fuchsia_fxfs as ffxfs;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_storage_block::BlockProxy;
use fidl_fuchsia_storage_partitions as fpartitions;
use fs_management::filesystem::{BlockConnector, ServingMultiVolumeFilesystem, ServingVolume};
use fs_management::format::DiskFormat;
use fs_management::{ComponentType, FSConfig, Fvm, Fxfs};
use fuchsia_async as fasync;
use fuchsia_component::client::connect_to_protocol;
use fuchsia_inspect as finspect;
use futures::channel::mpsc;
use std::sync::Arc;
use vfs::execution_scope::ExecutionScope;

use crate::device::constants::{BLOB_IMAGE_VOLUME_LABEL, BLOB_VOLUME_LABEL};
use crate::recovery::IMAGE_FILE_NAME;

pub struct PartitionInfo {
    pub label: String,
    pub type_guid: [u8; 16],
}

/// Environment is a trait that performs actions when a device is matched.
/// Nb: matcher.rs depend on this interface being used in order to mock tests.
#[async_trait]
pub trait Environment: Send + Sync {
    /// Binds an instance of the GPT component to the given device. Returns a list of the names of
    /// the child partitions.
    async fn launch_and_enumerate_gpt_component(
        &mut self,
        device: &mut dyn Device,
    ) -> Result<(Filesystem, Vec<PartitionInfo>), Error>;

    /// Registers a mounted GPT (bound to `device`) as the system GPT.
    fn register_system_gpt(&mut self, device: &dyn Device, gpt: Filesystem) -> Result<(), Error>;

    /// Returns a proxy for the exposed dir of the partition table manager.  This can be called
    /// before the manager is bound and it will get routed once bound.
    fn partition_manager_exposed_dir(&mut self) -> Result<fio::DirectoryProxy, Error>;

    /// Creates a static instance of Fxfs on `device` and calls serve_multi_volume(). Only creates
    /// the overall Fxfs instance. Mount_blob_volume and mount_data_volume still need to be called.
    async fn mount_fxblob(&mut self, device: &mut dyn Device) -> Result<(), Error>;

    /// Creates a static instance of Fxfs on `device` and calls serve_multi_volume(). Only creates
    /// the overall Fvm instance. Mount_blob_volume and mount_data_volume still need to be called.
    async fn mount_fvm(&mut self, device: &mut dyn Device) -> Result<(), Error>;

    /// Mounts the blob volume on the already mounted container filesystem.
    async fn mount_blob_volume(&mut self) -> Result<(), Error>;

    /// Mounts data volume on the already mounted container filesystem.
    async fn mount_data_volume(&mut self) -> Result<(), Error>;

    /// Called to shred the encryption keys for the data volume.
    async fn shred_data_online(&mut self) -> Result<(), Error>;

    /// Synchronously shut down all associated filesystems.
    async fn shutdown(&mut self) -> Result<(), Error>;

    /// Returns the registered devices.
    fn registered_devices(&self) -> &Arc<RegisteredDevices>;

    /// Returns the container's ServingMultiVolumeFilesystem if the container has been set.
    fn get_container(&mut self) -> Option<&mut ServingMultiVolumeFilesystem>;

    /// Register a running filesystem with the environment. This allows the filesystem to continue
    /// to exist for the lifetime of fshost, and the environment will shut it down cleanly at the
    /// right time.
    fn register_filesystem(&mut self, filesystem: Filesystem);

    /// Publish a device in the block directory.
    fn publish_device(&mut self, device: &mut dyn Device, name: &str) -> Result<(), Error>;

    /// Publish a device in the debug block directory.
    fn publish_device_to_debug_block(
        &mut self,
        device: &dyn Device,
        name: &str,
    ) -> Result<(), Error>;

    /// When called, attempt to provision the device with Fxfs.
    async fn provision_fxfs(&mut self, device: &mut dyn Device) -> Result<(), Error>;

    /// Reports a filesystem corruption for a `format` filesystem with `error`.  Files a crash
    /// report.
    fn report_corruption(&self, format: &str, error: &Error);
}

enum BufferedDirectory {
    Queue(Vec<ServerEnd<fio::DirectoryMarker>>),
    Dir(fio::DirectoryProxy),
    Closed,
}

pub enum Filesystem {
    Queue(Vec<ServerEnd<fio::DirectoryMarker>>),
    ServingVolumeInMultiVolume(
        // We hold onto crypt service here to avoid it prematurely shutting down.
        #[allow(dead_code)] Option<CryptService>,
        ServingVolume,
    ),
    ServingGpt(ServingMultiVolumeFilesystem),
    Shutdown,
}

impl Filesystem {
    fn is_serving(&self) -> bool {
        if let Self::Queue(_) = self { false } else { true }
    }

    pub fn exposed_dir(&mut self) -> Result<fio::DirectoryProxy, Error> {
        let (proxy, server) = create_proxy::<fio::DirectoryMarker>();
        match self {
            Filesystem::Queue(queue) => queue.push(server),
            Filesystem::ServingVolumeInMultiVolume(_, volume) => {
                volume.exposed_dir().clone(server.into_channel().into())?
            }
            Filesystem::ServingGpt(fs) => fs.exposed_dir().clone(server.into_channel().into())?,
            Filesystem::Shutdown => bail!(anyhow!("filesystem is shutting down")),
        }
        Ok(proxy)
    }

    pub fn root(&mut self) -> Result<fio::DirectoryProxy, Error> {
        let root = fuchsia_fs::directory::open_directory_async(
            &self.exposed_dir().context("failed to get exposed dir")?,
            "root",
            fio::PERM_READABLE | fio::Flags::PERM_INHERIT_WRITE | fio::Flags::PERM_INHERIT_EXECUTE,
        )
        .context("failed to open the root directory")?;
        Ok(root)
    }

    fn queue(&mut self) -> Option<&mut Vec<ServerEnd<fio::DirectoryMarker>>> {
        match self {
            Filesystem::Queue(queue) => Some(queue),
            _ => None,
        }
    }

    pub async fn shutdown(&mut self) -> Result<(), Error> {
        let old = std::mem::replace(self, Filesystem::Shutdown);
        match old {
            Filesystem::Queue(_) => Ok(()),
            Filesystem::ServingVolumeInMultiVolume(_, volume) => {
                if volume.root().is_closed() {
                    log::info!("volume is already closed, skipping volume shutdown");
                    Ok(())
                } else {
                    volume.shutdown().await.context("shutdown failed")
                }
            }
            Filesystem::ServingGpt(fs) => fs.shutdown().await.context("shutdown failed"),
            // Getting shut down when we are already shut down is fine. We are already in the
            // desired state!
            Filesystem::Shutdown => Ok(()),
        }
    }
}

/// Trait that captures the common interface between different multi-volume filesystems (there
/// should be concrete implementations for Fxfs and Fvm).
#[async_trait]
pub trait Container: Send + Sync {
    /// Returns fs_management's wrapper around the container filesystem.
    fn fs(&mut self) -> &mut ServingMultiVolumeFilesystem;

    /// Converts into fs_management's wrapper.
    fn into_fs(self: Box<Self>) -> ServingMultiVolumeFilesystem;

    /// Returns the label used for the blob volume.
    fn blobfs_volume_label(&self) -> &'static str;

    /// Called to check the blob volume if required.
    async fn maybe_check_blob_volume(&mut self) -> Result<(), Error> {
        // Default to not checking.
        Ok(())
    }

    /// Returns the current set of volumes for this container.
    async fn get_volumes(&mut self) -> Result<Vec<String>, Error> {
        // We expect the startup protocol to work with fxblob. There are no options for
        // reformatting the entire fxfs partition now that blobfs is one of the volumes.
        let volumes_dir = fuchsia_fs::directory::open_directory(
            self.fs().exposed_dir(),
            "volumes",
            fuchsia_fs::PERM_READABLE,
        )
        .await
        .context("opening volumes directory")?;
        Ok(fuchsia_fs::directory::readdir(&volumes_dir)
            .await
            .context("reading volumes directory")?
            .into_iter()
            .map(|e| e.name)
            .collect())
    }

    /// Serves the data volume on this container.
    async fn serve_data(&mut self, launcher: &FilesystemLauncher) -> Result<Filesystem, Error>;

    /// Recreates the data volume.
    async fn format_data(&mut self, launcher: &FilesystemLauncher) -> Result<Filesystem, Error>;

    /// Typically called by `format_data` implementations to remove all the non-blob volumes.  All
    /// volumes must be unmounted.
    async fn remove_all_non_blob_volumes(&mut self) -> Result<(), Error> {
        let blobfs_volume_label = self.blobfs_volume_label();
        let fs = self.fs();
        let volumes_dir = fuchsia_fs::directory::open_directory_async(
            fs.exposed_dir(),
            "volumes",
            fio::PERM_READABLE,
        )?;
        let volumes = fuchsia_fs::directory::readdir(&volumes_dir).await?;
        for volume in volumes {
            if &volume.name != blobfs_volume_label {
                fs.remove_volume(&volume.name)
                    .await
                    .with_context(|| format!("failed to remove volume: {}", volume.name))?;
            }
        }
        Ok(())
    }

    /// Called to shred the encryption keys for the data volume.
    async fn shred_data(&mut self) -> Result<(), Error>;

    /// Determine whether the data volume of this container will be encrypted with zxcrypt.
    fn data_requires_zxcrypt(&self, launcher: &FilesystemLauncher) -> bool;
}

/// This trait exists to make working with `container` easier below and avoid the
/// somewhat hard to read repeated implementation you see below.
trait MaybeFs {
    fn maybe_fs(&mut self) -> Option<&mut ServingMultiVolumeFilesystem>;
}

impl MaybeFs for Option<Box<dyn Container>> {
    fn maybe_fs(&mut self) -> Option<&mut ServingMultiVolumeFilesystem> {
        self.as_mut().map(|c| c.fs())
    }
}

/// Implements the Environment trait and keeps track of mounted filesystems.
pub struct FshostEnvironment {
    config: Arc<fshost_config::Config>,
    // The GPT is run as a component.  `gpt_device_service_instance` will connect to the volume
    // Service instance for the block device that `gpt` is mounted on.
    gpt: Filesystem,
    gpt_device_service_instance: BufferedDirectory,
    // `container` is set inside mount_fxblob() or mount_fvm() and represents the overall
    // Fxfs/Fvm instance which contains both a data and blob volume.
    container: Option<Box<dyn Container>>,
    blobfs: Filesystem,
    data: Filesystem,
    launcher: Arc<FilesystemLauncher>,
    watcher: Watcher,
    registered_devices: Arc<RegisteredDevices>,
    other_filesystems: Vec<Filesystem>,
    device_publisher: DevicePublisher,
    scope: ExecutionScope,
    shutdown_tx: mpsc::Sender<service::FshostShutdownResponder>,
}

impl FshostEnvironment {
    pub fn new(
        config: Arc<fshost_config::Config>,
        inspector: fuchsia_inspect::Inspector,
        watcher: Watcher,
        registered_devices: Arc<RegisteredDevices>,
        device_publisher: DevicePublisher,
        scope: ExecutionScope,
        shutdown_tx: mpsc::Sender<service::FshostShutdownResponder>,
    ) -> Self {
        let corruption_events = inspector.root().create_child("corruption_events");
        let keymint_unseal_failure_events =
            inspector.root().create_child("keymint_unseal_failure_events");
        Self {
            config: config.clone(),
            gpt: Filesystem::Queue(Vec::new()),
            gpt_device_service_instance: BufferedDirectory::Queue(Vec::new()),
            container: None,
            blobfs: Filesystem::Queue(Vec::new()),
            data: Filesystem::Queue(Vec::new()),
            launcher: Arc::new(FilesystemLauncher {
                config,
                corruption_events,
                keymint_unseal_failure_events,
            }),
            watcher,
            registered_devices,
            other_filesystems: Vec::new(),
            device_publisher,
            scope,
            shutdown_tx,
        }
    }

    /// Returns a proxy for the exposed dir of the Blobfs filesystem.  This can be called before
    /// Blobfs is mounted and it will get routed once Blobfs is mounted.
    pub fn blobfs_exposed_dir(&mut self) -> Result<fio::DirectoryProxy, Error> {
        self.blobfs.exposed_dir()
    }

    /// Returns a proxy for the exposed dir of the data filesystem.  This can be called before
    /// "/data" is mounted and it will get routed once the data partition is mounted.
    pub fn data_exposed_dir(&mut self) -> Result<fio::DirectoryProxy, Error> {
        self.data.exposed_dir()
    }

    /// Returns a proxy for the root of the data filesystem.  This can be called before "/data" is
    /// mounted and it will get routed once the data partition is mounted.
    pub fn data_root(&mut self) -> Result<fio::DirectoryProxy, Error> {
        self.data.root()
    }

    /// Returns a proxy for the fuchsia.hardware.block.volume.Service instance for the block device
    /// containing the system GPT.  This can be called before that device is discovered, and the
    /// client will hanging-get until that happens.
    pub fn system_gpt_volume_service_instance(&mut self) -> Result<fio::DirectoryProxy, Error> {
        match &mut self.gpt_device_service_instance {
            BufferedDirectory::Queue(queue) => {
                let (proxy, server_end) = create_proxy::<fio::DirectoryMarker>();
                queue.push(server_end);
                Ok(proxy)
            }
            BufferedDirectory::Dir(dir) => {
                let (proxy, server_end) = create_proxy::<fio::DirectoryMarker>();
                dir.clone(server_end.into_channel().into())?;
                Ok(proxy)
            }
            BufferedDirectory::Closed => bail!("system gpt is closed"),
        }
    }

    pub fn launcher(&self) -> Arc<FilesystemLauncher> {
        self.launcher.clone()
    }

    fn spawn_volume_watcher(
        &self,
        root_dir: fidl_fuchsia_io::DirectoryProxy,
        volume_name: &'static str,
    ) {
        let mut shutdown_tx = self.shutdown_tx.clone();

        self.scope.spawn(async move {
            // Block until the cloned DirectoryProxy connection channel is closed
            // The task will clean itself up when the filesystem eventully shuts down.
            let _ = root_dir.on_closed().await;

            // Only the first shutdown message will get through. It is normal and fine to fail here.
            let _ = shutdown_tx.try_send(service::FshostShutdownResponder::Crash(volume_name));
        });
    }
}

#[async_trait]
impl Environment for FshostEnvironment {
    async fn launch_and_enumerate_gpt_component(
        &mut self,
        device: &mut dyn Device,
    ) -> Result<(Filesystem, Vec<PartitionInfo>), Error> {
        let mut filesystem = fs_management::filesystem::Filesystem::from_boxed_config(
            device.block_connector()?,
            Box::new(fs_management::Gpt {
                merge_super_and_userdata: self.config.merge_super_and_userdata,
                ..fs_management::Gpt::dynamic_child()
            }),
        );
        let moniker = filesystem.get_component_moniker().await.context("Starting GPT component")?;
        let serving = filesystem.serve_multi_volume().await.context("Failed to start GPT")?;
        let exposed_dir = serving.exposed_dir();
        let partitions_dir = fuchsia_fs::directory::open_directory(
            &exposed_dir,
            fpartitions::PartitionServiceMarker::SERVICE_NAME,
            fuchsia_fs::PERM_READABLE,
        )
        .await
        .context("Failed to open partitions dir")?;
        let entries = fuchsia_fs::directory::readdir(&partitions_dir).await?;
        let mut partitions = Vec::new();
        for entry in entries {
            let endpoint_name = format!("{}/volume", entry.name);
            let proxy = connect_to_named_protocol_at_dir_root::<BlockProxy>(
                &partitions_dir,
                &endpoint_name,
            )?;
            let (raw_status, label) = proxy.get_name().await?;
            zx::Status::ok(raw_status)?;
            let (raw_status, type_guid) = proxy.get_type_guid().await?;
            zx::Status::ok(raw_status)?;
            if let (Some(label), Some(type_guid)) = (label, type_guid) {
                partitions.push(PartitionInfo { label, type_guid: type_guid.value });
            }
        }
        self.watcher
            .add_source(Box::new(DirSource::new(
                partitions_dir,
                moniker,
                crate::device::Parent::SystemPartitionTable,
            )))
            .await
            .context("Failed to watch gpt partitions dir")?;
        Ok((Filesystem::ServingGpt(serving), partitions))
    }

    fn register_system_gpt(
        &mut self,
        device: &dyn Device,
        mut gpt: Filesystem,
    ) -> Result<(), Error> {
        if self.gpt.is_serving() {
            // If we want to support multiple GPT devices, we'll need to change Environment to
            // separate the system GPT and other GPTs.
            bail!("GPT already bound");
        }
        let queue = self.gpt.queue().unwrap();
        let exposed_dir = gpt.exposed_dir()?;
        for server in queue.drain(..) {
            exposed_dir.clone(server.into_channel().into())?;
        }
        self.gpt = gpt;
        match device.service_instance_directory() {
            Ok(dir) => {
                match &mut self.gpt_device_service_instance {
                    BufferedDirectory::Queue(queue) => {
                        for server in queue.drain(..) {
                            dir.clone(server.into_channel().into())?;
                        }
                        self.gpt_device_service_instance = BufferedDirectory::Dir(dir);
                    }
                    // gpt and gpt_device_service_instance should be set together, and we checked
                    // for repeated calls above
                    BufferedDirectory::Dir(_) => unreachable!(),
                    BufferedDirectory::Closed => {}
                }
            }
            Err(err) => {
                log::warn!(
                    err:?;
                    "Failed to get service instance directory; system_gpt service instance will \
                     not be available"
                );
                self.gpt_device_service_instance = BufferedDirectory::Closed;
            }
        }
        Ok(())
    }

    fn partition_manager_exposed_dir(&mut self) -> Result<fio::DirectoryProxy, Error> {
        self.gpt.exposed_dir()
    }

    async fn mount_fxblob(&mut self, device: &mut dyn Device) -> Result<(), Error> {
        log::info!(
            path:% = device.path(),
            expected_format = "fxfs";
            "Mounting fxblob"
        );
        // In production, barriers should only be enabled when inline encryption is enabled.
        let config = Fxfs {
            component_type: ComponentType::StaticChild,
            startup_profiling_seconds: Some(60),
            inline_crypto_enabled: self.config.inline_crypto,
            barriers_enabled: self.config.inline_crypto,
            ..Default::default()
        };
        let serving_fs =
            self.launcher.serve_fxblob(device.block_connector()?, Box::new(config)).await?;
        self.container = Some(Box::new(FxfsContainer::new(serving_fs)));
        if !self.gpt.is_serving() && !device.is_fshost_ramdisk() {
            // NB: If we've already found the system container, but not the GPT, we are probably
            // dealing with a system like QEMU where the block device is directly Fxfs-formatted,
            // rather than having a GPT.  Since clients might be blocking on the GPT appearing,
            // explicitly error them out.
            // This call won't fail because we just checked that the gpt is not serving.
            self.gpt.shutdown().await.unwrap();
        }
        Ok(())
    }

    async fn mount_fvm(&mut self, device: &mut dyn Device) -> Result<(), Error> {
        log::info!(
            path:% = device.path(),
            expected_format = "fvm";
            "Mounting fvm"
        );
        let config = Fvm { component_type: ComponentType::StaticChild, ..Default::default() };
        let serving_fs =
            self.launcher.serve_fvm(device.block_connector()?, Box::new(config)).await?;
        self.container = Some(Box::new(FvmContainer::new(serving_fs, device.is_fshost_ramdisk())));
        if !self.gpt.is_serving() && !device.is_fshost_ramdisk() {
            // See comment in `mount_fxblob`.
            self.gpt.shutdown().await.unwrap();
        }
        Ok(())
    }

    async fn mount_blob_volume(&mut self) -> Result<(), Error> {
        let _ = self.blobfs.queue().ok_or_else(|| anyhow!("blobfs partition already mounted"))?;

        let container = self.container.as_mut().ok_or_else(|| anyhow!("Missing container!"))?;

        container.maybe_check_blob_volume().await?;

        let label = container.blobfs_volume_label();
        let blobfs = container
            .fs()
            .open_volume(
                label,
                MountOptions {
                    as_blob: Some(true),
                    uri: Some(format!("#meta/blobfs.cm")),
                    ..MountOptions::default()
                },
            )
            .await
            .context("Failed to open the blob volume")?;
        let exposed_dir = blobfs.exposed_dir();
        let queue = self.blobfs.queue().ok_or_else(|| anyhow!("blobfs already mounted"))?;
        for server in queue.drain(..) {
            exposed_dir.clone(server.into_channel().into())?;
        }

        // For the death watcher.
        let root_dir_clone = match fuchsia_fs::directory::clone(blobfs.root()) {
            Ok(root_dir_clone) => Some(root_dir_clone),
            Err(e) => {
                log::warn!(
                    "Failed to clone blobfs root directory for crash watching: {:?}. \
                    Crash watching disabled for this volume.",
                    e
                );
                None
            }
        };

        self.blobfs = Filesystem::ServingVolumeInMultiVolume(None, blobfs);
        if let Err(e) = container.fs().set_byte_limit(label, self.config.blob_max_bytes).await {
            log::warn!("Failed to set byte limit for the blob volume: {:?}", e);
        }
        if let Some(root_dir_clone) = root_dir_clone {
            self.spawn_volume_watcher(root_dir_clone, "blob");
        }

        Ok(())
    }

    async fn mount_data_volume(&mut self) -> Result<(), Error> {
        let _ = self.data.queue().ok_or_else(|| anyhow!("data partition already mounted"))?;

        let container = self.container.as_mut().ok_or_else(|| anyhow!("Missing container!"))?;
        let mut filesystem = container.serve_data(&self.launcher).await?;

        if self.config.data_max_bytes != 0 {
            let extra_bytes = if container.data_requires_zxcrypt(&self.launcher) {
                connect_to_protocol_at_dir_root::<VolumesProxy>(container.fs().exposed_dir())
                    .context("failed to connect to volumes proxy")?
                    .get_info()
                    .await
                    .context("getting volume info failed (fidl error)")?
                    .map_err(zx::Status::err_from_raw)
                    .context("getting volume info failed (returned error)")?
                    .ok_or_else(|| anyhow!("getting volume info returned nothing"))?
                    .slice_size
            } else {
                0
            };
            if let Err(e) = container
                .fs()
                .set_byte_limit(DATA_VOLUME_LABEL, self.config.data_max_bytes + extra_bytes)
                .await
            {
                log::warn!("Failed to set byte limit for the data volume: {:?}", e);
            }
        }

        let queue = self.data.queue().unwrap();
        let exposed_dir = filesystem.exposed_dir()?;
        for server in queue.drain(..) {
            exposed_dir.clone(server.into_channel().into())?;
        }

        // Watcher Spawn on Data Volume
        if let Filesystem::ServingVolumeInMultiVolume(_, volume) = &filesystem {
            let root_dir = volume.root();
            match fuchsia_fs::directory::clone(root_dir) {
                Ok(root_clone) => {
                    self.spawn_volume_watcher(root_clone, "data");
                }
                Err(e) => {
                    log::warn!(
                        "Failed to clone data root directory for crash watching: {:?}. \
                        Crash watching disabled for this volume.",
                        e
                    );
                }
            }
        } else {
            log::warn!(
                "Data filesystem is not a multi-volume serving volume. Crash watching disabled."
            );
        }

        self.data = filesystem;
        Ok(())
    }

    async fn shred_data_online(&mut self) -> Result<(), Error> {
        if !self.data.is_serving() {
            return Err(anyhow!("Can't shred data; not already mounted"));
        }
        if let Some(container) = self.container.as_mut() {
            container.shred_data().await
        } else {
            Err(anyhow!("can't shred data; no container"))
        }
    }

    async fn shutdown(&mut self) -> Result<(), Error> {
        // If we encounter an error, log it, but continue trying to shut down the remaining
        // filesystems.
        self.blobfs.shutdown().await.unwrap_or_else(|error| {
            log::error!(error:?; "failed to shut down blobfs");
        });
        self.data.shutdown().await.unwrap_or_else(|error| {
            log::error!(error:?; "failed to shut down data");
        });
        if let Some(container) = self.container.take() {
            container.into_fs().shutdown().await.unwrap_or_else(|error| {
                log::error!(error:?; "failed to shut down container");
            })
        }
        // Shut down any other dynamic filesystems we happen to know about before we shut down
        // anything that could potentially be hosting them.
        for mut fs in self.other_filesystems.drain(..) {
            fs.shutdown().await.unwrap_or_else(|error| {
                log::error!(error:?; "failed to shut down other filesystem");
            })
        }
        self.gpt.shutdown().await.unwrap_or_else(|error| {
            log::error!(error:?; "failed to shut down gpt");
        });
        Ok(())
    }

    fn registered_devices(&self) -> &Arc<RegisteredDevices> {
        &self.registered_devices
    }

    fn get_container(&mut self) -> Option<&mut ServingMultiVolumeFilesystem> {
        self.container.maybe_fs()
    }

    fn register_filesystem(&mut self, filesystem: Filesystem) {
        self.other_filesystems.push(filesystem);
    }

    fn publish_device(&mut self, device: &mut dyn Device, name: &str) -> Result<(), Error> {
        self.device_publisher.publish(device.block_connector()?, name)
    }

    fn publish_device_to_debug_block(
        &mut self,
        device: &dyn Device,
        name: &str,
    ) -> Result<(), Error> {
        self.device_publisher.publish_to_debug_block_dir(device, name)
    }

    async fn provision_fxfs(&mut self, device: &mut dyn Device) -> Result<(), Error> {
        debug_assert!(
            self.config.provision_fxfs,
            "fshost was not configured to provision Fxfs yet `provision_fxfs(..)` was called"
        );

        let partition_service = device
            .service_instance_directory()
            .with_context(|| anyhow!("Failed to open service instance for {}", device.path()))?;

        let fxfs_provisioner = connect_to_protocol::<ffxfsprovisioner::FxfsProvisionerMarker>()
            .context("failed to connect to fxfs provisioner protocol")?;

        // If Fxfs provisioner fails, panic to force a reboot. This is okay as we only enable the
        // `provision_fxfs` config on specific builds - and in those case we would like to force a
        // reboot on failure.
        fxfs_provisioner
            .provision(partition_service.into_client_end().unwrap())
            .await
            .map_err(|err| panic!("Failed FIDL request to provision Fxfs: {err:?}."))?
            .map_err(|err| {
                panic!("Failed to provision Fxfs: {:?}.", zx::Status::err_from_raw(err))
            })?;

        Ok(())
    }

    fn report_corruption(&self, format: &str, error: &Error) {
        self.launcher.report_corruption(format, error);
    }
}

pub struct FilesystemLauncher {
    config: Arc<fshost_config::Config>,
    corruption_events: finspect::Node,
    keymint_unseal_failure_events: finspect::Node,
}

impl FilesystemLauncher {
    pub fn requires_zxcrypt(&self, format: DiskFormat, is_ramdisk: bool) -> bool {
        match format {
            // Fxfs never has zxcrypt underneath
            DiskFormat::Fxfs => false,
            _ if self.config.no_zxcrypt => false,
            // No point using zxcrypt for ramdisk devices.
            _ if is_ramdisk => false,
            _ => true,
        }
    }

    /// Starts serving Fxblob without opening any volumes.
    pub async fn serve_fxblob(
        &self,
        block_connector: Box<dyn BlockConnector>,
        config: Box<dyn FSConfig>,
    ) -> Result<ServingMultiVolumeFilesystem, Error> {
        let mut fs =
            fs_management::filesystem::Filesystem::from_boxed_config(block_connector, config);
        if self.config.check_filesystems {
            log::info!("fsck started for fxblob");
            if let Err(error) = fs.fsck().await {
                self.report_corruption("fxfs", &error);
                return Err(error);
            } else {
                log::info!("fsck completed OK for fxblob");
            }
        }
        let fs = fs.serve_multi_volume().await?;
        // Before we return the serving filesystem, handle installing any new blob volumes which
        // may have been flashed to the device.
        if fs.has_volume(BLOB_IMAGE_VOLUME_LABEL).await.context("checking for image volume")? {
            if let Err(error) = install_blob_image(&fs).await {
                // If we fail to install the new blob volume, all we can do here is log a warning
                // here and continue mounting the existing blob volume. Typically this happens if
                // flashing a new blob volume was incomplete, in which case we probably have
                // booted back into the old slot, and this warning can be ignored. We don't want
                // to file a crash report in this case, otherwise the slot may erroneously be
                // marked as unhealthy.
                log::warn!(error:?; "could not install new blob volume");
            }
        }
        Ok(fs)
    }

    /// Starts serving Fvm without opening any volumes.
    pub async fn serve_fvm(
        &self,
        block_connector: Box<dyn BlockConnector>,
        config: Box<dyn FSConfig>,
    ) -> Result<ServingMultiVolumeFilesystem, Error> {
        let fs = fs_management::filesystem::Filesystem::from_boxed_config(block_connector, config);
        fs.serve_multi_volume().await
    }

    fn report_corruption(&self, format: &str, error: &Error) {
        log::error!(format, error:?; "FILESYSTEM CORRUPTION DETECTED!");
        log::error!(
            "Please file a bug to the Storage component in http://fxbug.dev, including a \
            device snapshot collected with `ffx target snapshot` if possible.",
        );

        // If a keymint unseal error occurred, the problem is likely key invalidation
        // as part of FDR (see shred_keys.rs). We want to be able to differentiate these
        // from filesystem or device level corruption so we report them as
        // `fuchsia-fxfs-unseal-error`.
        let is_unseal_error =
            error.root_cause().downcast_ref::<kms_stateless::SealingKeysError>().is_some();

        let crash_signature = Some(if is_unseal_error {
            format!("fuchsia-{format}-unseal-error")
        } else {
            format!("fuchsia-{format}-corruption")
        });

        let report = fidl_fuchsia_feedback::CrashReport {
            program_name: Some(format.to_string()),
            crash_signature,
            is_fatal: Some(false),
            ..Default::default()
        };

        fasync::Task::spawn(async move {
            let proxy = if let Ok(proxy) =
                connect_to_protocol::<fidl_fuchsia_feedback::CrashReporterMarker>()
            {
                proxy
            } else {
                log::error!("Failed to connect to crash report service");
                return;
            };
            if let Err(e) = proxy.file_report(report).await {
                log::error!(e:?; "Failed to file crash report");
            }
        })
        .detach();

        // NOTE: If an event is recorded multiple times, Inspect does not deduplicate them.
        // Sampler (which converts Inspect -> Cobalt) will arbitrarily select one of the samples,
        // rather than aggregating them.  Since we only ever expect one of these events per boot,
        // this should not cause any issues.
        if is_unseal_error {
            self.keymint_unseal_failure_events.record_uint(format, 1);
        } else {
            self.corruption_events.record_uint(format, 1);
        }
    }
}

/// Searches for a new blob volume ready for installation on the system container and attempts to
/// install it. On failure, the installation file will be cleaned up so we don't attempt the
/// installation again on subsequent boots.
pub async fn install_blob_image(fs: &ServingMultiVolumeFilesystem) -> Result<(), Error> {
    log::info!("Installing system blob volume from image...");

    let installer: ffxfs::VolumeInstallerProxy = connect_to_named_protocol_at_dir_root(
        fs.exposed_dir(),
        ffxfs::VolumeInstallerMarker::PROTOCOL_NAME,
    )?;
    if let Err(error) = installer
        .install(BLOB_IMAGE_VOLUME_LABEL, IMAGE_FILE_NAME, BLOB_VOLUME_LABEL)
        .await
        .context("FIDL call to fuchsia.fxfs/VolumeInstaller.Install")?
        .map_err(zx::Status::err_from_raw)
    {
        log::error!(error:?; "failed to install blob volume, cleaning up...");
        if let Err(error) = fs.remove_volume(BLOB_IMAGE_VOLUME_LABEL).await {
            log::error!(error:?; "could not remove blob image after failed installation");
        }
        // Return the original installation error.
        return Err(error).context("failed to install blob volume");
    } else {
        log::info!("Successfully installed system blob volume from image.");
    }

    Ok(())
}
