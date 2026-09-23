// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use block_matcher::{Guid, create_random_guid};
use fidl::endpoints::{
    DiscoverableProtocolMarker as _, Proxy, create_proxy, create_request_stream,
};
use fidl_fuchsia_fs_startup::{CreateOptions, MountOptions};
use fidl_fuchsia_io as fio;
use fidl_fuchsia_storage_block::BlockMarker;
use fidl_fuchsia_storage_partitions as fpartitions;
use fs_management::Fvm;
use fs_management::filesystem::{
    BlockConnector, DirBasedBlockConnector, ServingMultiVolumeFilesystem,
};
use fuchsia_async as fasync;
use fuchsia_component::client::{Service, connect_to_protocol, connect_to_protocol_at_dir_root};
use std::sync::Arc;
use storage_benchmarks::block_device::BlockDevice;
use storage_benchmarks::{BlockDeviceConfig, BlockDeviceFactory};

const BENCHMARK_FVM_SIZE_BYTES: u64 = 160 * 1024 * 1024;
// 8MiB is the default slice size; use it so the test FVM partition matches the performance of the
// system FVM partition (so they are interchangeable).
// Note that this only affects the performance of minfs and blobfs, since these two filesystems are
// the only ones that dynamically allocate from FVM.
const BENCHMARK_FVM_SLICE_SIZE_BYTES: u64 = 8 * 1024 * 1024;

// On systems which don't have FVM (i.e. Fxblob), we create an FVM partition the test can use, with
// this GUID.  See connect_to_test_fvm for details.

const BENCHMARK_TYPE_GUID: &Guid = &[
    0x67, 0x45, 0x23, 0x01, 0xab, 0x89, 0xef, 0xcd, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef,
];
const BENCHMARK_VOLUME_NAME: &str = "benchmark";

/// Returns the exposed directory of the volume, as well as the task running Crypt for the volume
/// (if configured in `config`).
pub async fn create_fvm_volume(
    fvm: &fidl_fuchsia_fs_startup::VolumesProxy,
    instance_guid: [u8; 16],
    config: &BlockDeviceConfig,
) -> (fio::DirectoryProxy, Option<fasync::Task<()>>) {
    loop {
        let (crypt, crypt_task) = if config.use_zxcrypt {
            let (crypt, stream) = create_request_stream::<fidl_fuchsia_fxfs::CryptMarker>();
            let task = fasync::Task::spawn(async {
                if let Err(err) =
                    zxcrypt_crypt::run_crypt_service(crypt_policy::Policy::Null, stream).await
                {
                    log::error!(err:?; "Crypt service failure");
                }
            });
            (Some(crypt), Some(task))
        } else {
            (None, None)
        };

        let (volume_dir, server_end) = create_proxy::<fio::DirectoryMarker>();
        let res = fvm
            .create(
                BENCHMARK_VOLUME_NAME,
                server_end,
                CreateOptions {
                    initial_size: config.volume_size,
                    type_guid: Some(BENCHMARK_TYPE_GUID.clone()),
                    guid: Some(instance_guid),
                    ..Default::default()
                },
                MountOptions { crypt, ..Default::default() },
            )
            .await
            .expect("FIDL error")
            .map_err(zx::Status::err_from_raw);

        // If the FVM already exists, remove it and try again.
        if res == Err(zx::Status::ALREADY_EXISTS) {
            fvm.remove(BENCHMARK_VOLUME_NAME)
                .await
                .expect("FIDL error")
                .map_err(zx::Status::err_from_raw)
                .expect("Failed to remove volume");
            continue;
        }

        res.expect("Failed to create volume");
        return (volume_dir, crypt_task);
    }
}

/// A factory for volumes which the benchmarks run in.  If the system has an FVM, benchmarks run in
/// a volume in the system FVM.  Otherwise, they run out of a partition in the system GPT (and might
/// still include a hermetic FVM instance, if the benchmark needs it to run).
pub enum BenchmarkVolumeFactory {
    SystemFvm(
        Box<dyn Send + Sync + Fn() -> fidl_fuchsia_fs_startup::VolumesProxy>,
        Box<dyn Send + Sync + Fn() -> fio::DirectoryProxy>,
    ),
    SystemGpt(Arc<fpartitions::PartitionServiceProxy>),
}

struct RawBlockDeviceInGpt(Arc<fpartitions::PartitionServiceProxy>);

impl BlockDevice for RawBlockDeviceInGpt {
    fn connector(&self) -> Box<dyn BlockConnector> {
        Box::new(self.0.clone())
    }
}

#[async_trait]
impl BlockDeviceFactory for BenchmarkVolumeFactory {
    async fn create_block_device(&self, config: &BlockDeviceConfig) -> Box<dyn BlockDevice> {
        let instance_guid = create_random_guid();
        match self {
            Self::SystemFvm(volumes_connector, _) => {
                let volumes = volumes_connector();
                Box::new(Self::create_fvm_volume(volumes, instance_guid, config).await)
            }
            Self::SystemGpt(partition_service) => {
                if config.requires_fvm {
                    Box::new(
                        Self::create_fvm_instance_and_volume(
                            partition_service.clone(),
                            instance_guid,
                            config,
                        )
                        .await,
                    )
                } else {
                    Box::new(RawBlockDeviceInGpt(partition_service.clone()))
                }
            }
        }
    }
}

impl BenchmarkVolumeFactory {
    /// Creates a factory for volumes in which benchmarks should run in based on the provided
    /// configuration.  Uses various capabilities in the incoming namespace of the process.
    pub async fn from_config(fxfs_blob: bool) -> BenchmarkVolumeFactory {
        if fxfs_blob {
            let partitions = Service::open(fpartitions::PartitionServiceMarker).unwrap();
            let manager = connect_to_protocol::<fpartitions::PartitionsManagerMarker>().unwrap();
            let instance =
                BenchmarkVolumeFactory::connect_to_test_partition(partitions, manager).await;
            assert!(
                instance.is_some(),
                "Failed to open or create testing FVM in GPT.  \
                    Perhaps the system doesn't have a GPT-formatted block device?"
            );
            instance.unwrap()
        } else {
            let volumes_connector = Box::new(move || {
                connect_to_protocol::<fidl_fuchsia_fs_startup::VolumesMarker>().unwrap()
            });
            let volumes_dir_connector = {
                Box::new(move || {
                    fuchsia_fs::directory::open_in_namespace("volumes", fio::PERM_READABLE).unwrap()
                })
            };
            BenchmarkVolumeFactory::connect_to_system_fvm(volumes_connector, volumes_dir_connector)
                .unwrap()
        }
    }

    /// Connects to the system FVM component.
    pub fn connect_to_system_fvm(
        volumes_connector: Box<dyn Send + Sync + Fn() -> fidl_fuchsia_fs_startup::VolumesProxy>,
        volumes_dir_connector: Box<dyn Send + Sync + Fn() -> fio::DirectoryProxy>,
    ) -> Option<BenchmarkVolumeFactory> {
        Some(BenchmarkVolumeFactory::SystemFvm(volumes_connector, volumes_dir_connector))
    }

    // Creates and connects to the partition reserved for benchmarks, or adds it to the GPT if
    // absent.  The partition will be unformatted and should be reformatted explicitly before being
    // used for a benchmark.
    pub async fn connect_to_test_partition(
        service: Service<fpartitions::PartitionServiceMarker>,
        manager: fpartitions::PartitionsManagerProxy,
    ) -> Option<BenchmarkVolumeFactory> {
        let connector = block_matcher::find_or_create_test_partition(
            service,
            manager,
            BENCHMARK_FVM_SIZE_BYTES,
        )
        .await
        .expect("Failed to find or create test partition");

        Some(BenchmarkVolumeFactory::SystemGpt(Arc::new(connector)))
    }

    #[cfg(test)]
    pub async fn contains_fvm_volume(&self, name: &str) -> bool {
        match self {
            Self::SystemFvm(_, volumes_dir_connector) => {
                let dir = volumes_dir_connector();
                fuchsia_fs::directory::dir_contains(&dir, name).await.unwrap()
            }
            // If we're using a system GPT, the FVM instance is created on the fly, so volumes are
            // too.
            _ => false,
        }
    }

    async fn create_fvm_volume(
        volumes: fidl_fuchsia_fs_startup::VolumesProxy,
        instance_guid: [u8; 16],
        config: &BlockDeviceConfig,
    ) -> FvmVolume {
        let (volume_dir, crypt_task) = create_fvm_volume(&volumes, instance_guid, config).await;
        let volumes = volumes.into_client_end().unwrap().into_sync_proxy();
        FvmVolume {
            destroy_fn: Some(Box::new(move || {
                volumes
                    .remove(BENCHMARK_VOLUME_NAME, zx::MonotonicInstant::INFINITE)
                    .unwrap()
                    .map_err(zx::Status::err_from_raw)
            })),
            volume_dir: Some(volume_dir),
            fvm_instance: None,
            block_path: format!("svc/{}", BlockMarker::PROTOCOL_NAME),
            crypt_task,
        }
    }

    async fn create_fvm_instance_and_volume(
        partition: Arc<fpartitions::PartitionServiceProxy>,
        instance_guid: [u8; 16],
        config: &BlockDeviceConfig,
    ) -> FvmVolume {
        let mut fs = fs_management::filesystem::Filesystem::from_boxed_config(
            Box::new(partition),
            Box::new(Fvm { slice_size: BENCHMARK_FVM_SLICE_SIZE_BYTES, ..Fvm::default() }),
        );
        fs.format().await.expect("Failed to format FVM");
        let fvm_instance = fs.serve_multi_volume().await.expect("Failed to serve FVM");
        let volumes = connect_to_protocol_at_dir_root::<fidl_fuchsia_fs_startup::VolumesMarker>(
            fvm_instance.exposed_dir(),
        )
        .unwrap();

        let (volume_dir, crypt_task) = create_fvm_volume(&volumes, instance_guid, config).await;
        FvmVolume {
            destroy_fn: None,
            volume_dir: Some(volume_dir),
            fvm_instance: Some(fvm_instance),
            block_path: format!("svc/{}", BlockMarker::PROTOCOL_NAME),
            crypt_task,
        }
    }
}

/// A block device created on top of an FVM instance.
pub struct FvmVolume {
    destroy_fn: Option<Box<dyn Send + Sync + FnOnce() -> Result<(), zx::Status>>>,
    fvm_instance: Option<ServingMultiVolumeFilesystem>,
    volume_dir: Option<fio::DirectoryProxy>,
    crypt_task: Option<fasync::Task<()>>,
    // The path in `volume_dir` to connect to when opening a new Block connection.
    block_path: String,
}

impl BlockDevice for FvmVolume {
    fn connector(&self) -> Box<dyn BlockConnector> {
        let volume_dir = fuchsia_fs::directory::clone(self.volume_dir.as_ref().unwrap()).unwrap();
        Box::new(DirBasedBlockConnector::new(volume_dir, self.block_path.clone()))
    }
}

impl Drop for FvmVolume {
    fn drop(&mut self) {
        self.volume_dir = None;
        self.fvm_instance = None;
        self.crypt_task = None;
        if let Some(destroy_fn) = self.destroy_fn.take() {
            destroy_fn().expect("Failed to destroy FVM volume");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{RAMDISK_FVM_SLICE_SIZE, RamdiskFactory};
    use block_client::RemoteBlockClient;
    use fidl_fuchsia_fs_startup::VolumesMarker;
    use fs_management::Gpt;
    use ramdevice_client::{RamdiskClient, RamdiskClientBuilder};
    use std::sync::Arc;
    use vmo_backed_block_server::VmoBackedServer;

    const BLOCK_SIZE: u64 = 4 * 1024;
    const BLOCK_COUNT: u64 = 1024;
    // We need more blocks for the GPT version of the test, since the library will by default
    // allocate 128MiB for the embedded FVM.  This is big enough for a 192MiB device.
    const GPT_BLOCK_COUNT: u64 = 49152;

    #[fuchsia::test]
    async fn ramdisk_create_block_device_with_zxcrypt() {
        let ramdisk_factory = RamdiskFactory::new(BLOCK_SIZE, BLOCK_COUNT);
        let _ = ramdisk_factory
            .create_block_device(&BlockDeviceConfig {
                requires_fvm: true,
                use_zxcrypt: true,
                volume_size: None,
            })
            .await;
    }

    #[fuchsia::test]
    async fn ramdisk_create_block_device_without_zxcrypt() {
        let ramdisk_factory = RamdiskFactory::new(BLOCK_SIZE, BLOCK_COUNT);
        let _ = ramdisk_factory
            .create_block_device(&BlockDeviceConfig {
                requires_fvm: true,
                use_zxcrypt: false,
                volume_size: None,
            })
            .await;
    }

    #[fuchsia::test]
    async fn ramdisk_create_block_device_without_volume_size() {
        let ramdisk_factory = RamdiskFactory::new(BLOCK_SIZE, BLOCK_COUNT);
        let ramdisk = ramdisk_factory
            .create_block_device(&BlockDeviceConfig {
                requires_fvm: true,
                use_zxcrypt: false,
                volume_size: None,
            })
            .await;
        let volume_info = ramdisk
            .connector()
            .connect_block()
            .unwrap()
            .into_proxy()
            .get_volume_info()
            .await
            .unwrap();
        zx::ok(volume_info.0).unwrap();
        let volume_info = volume_info.2.unwrap();
        assert_eq!(volume_info.partition_slice_count, 1);
    }

    #[fuchsia::test]
    async fn ramdisk_create_block_device_with_volume_size() {
        let ramdisk_factory = RamdiskFactory::new(BLOCK_SIZE, BLOCK_COUNT);
        let ramdisk = ramdisk_factory
            .create_block_device(&BlockDeviceConfig {
                requires_fvm: false,
                use_zxcrypt: false,
                volume_size: Some(RAMDISK_FVM_SLICE_SIZE as u64 * 3),
            })
            .await;
        let volume_info = ramdisk
            .connector()
            .connect_block()
            .unwrap()
            .into_proxy()
            .get_volume_info()
            .await
            .unwrap();
        zx::ok(volume_info.0).unwrap();
        let volume_info = volume_info.2.unwrap();
        assert_eq!(volume_info.partition_slice_count, 3);
    }

    async fn init_gpt(block_size: u32, block_count: u64) -> zx::Vmo {
        let vmo = zx::Vmo::create(block_size as u64 * block_count).unwrap();
        let server = VmoBackedServer::from_vmo(
            block_size,
            vmo.create_child(zx::VmoChildOptions::REFERENCE, 0, 0).unwrap(),
        )
        .expect("Failed to create VmoBackedServer");
        let (client, server_end) =
            fidl::endpoints::create_proxy::<fidl_fuchsia_storage_block::BlockMarker>();

        let _task =
            fasync::Task::spawn(async move { server.serve(server_end.into_stream()).await });
        let client = Arc::new(RemoteBlockClient::new(client).await.unwrap());
        gpt::Gpt::format(client.clone(), vec![gpt::PartitionInfo::nil(); 128])
            .await
            .expect("format failed");
        vmo
    }

    struct FvmTestConfig {
        fxblob_enabled: bool,
    }

    /// Retains test state.
    ///
    /// The ramdisk is running in an isolated devmgr instance, but the volume managers are
    /// running in child components.
    struct TestState(
        #[allow(dead_code)] RamdiskClient,
        #[allow(dead_code)] ServingMultiVolumeFilesystem,
    );

    async fn initialize(config: FvmTestConfig) -> (TestState, BenchmarkVolumeFactory) {
        if config.fxblob_enabled {
            // Initialize a new GPT.
            let vmo = init_gpt(BLOCK_SIZE as u32, GPT_BLOCK_COUNT).await;
            let ramdisk = RamdiskClientBuilder::new_with_vmo(vmo, Some(BLOCK_SIZE))
                .build()
                .await
                .expect("Failed to create ramdisk");

            let gpt = fs_management::filesystem::Filesystem::from_boxed_config(
                ramdisk.connector().unwrap(),
                Box::new(Gpt::dynamic_child()),
            )
            .serve_multi_volume()
            .await
            .expect("Failed to serve GPT");
            let partitions =
                Service::open_from_dir(gpt.exposed_dir(), fpartitions::PartitionServiceMarker)
                    .unwrap();
            let manager = connect_to_protocol_at_dir_root::<fpartitions::PartitionsManagerMarker>(
                gpt.exposed_dir(),
            )
            .unwrap();
            let fvm = BenchmarkVolumeFactory::connect_to_test_partition(partitions, manager)
                .await
                .expect("Failed to connect to FVM");
            (TestState(ramdisk, gpt), fvm)
        } else {
            // Initialize a new FVM.
            let ramdisk = RamdiskClientBuilder::new(BLOCK_SIZE, BLOCK_COUNT)
                .build()
                .await
                .expect("Failed to create ramdisk");
            let mut fs = fs_management::filesystem::Filesystem::from_boxed_config(
                ramdisk.connector().unwrap(),
                Box::new(Fvm { slice_size: RAMDISK_FVM_SLICE_SIZE, ..Fvm::dynamic_child() }),
            );
            fs.format().await.expect("Failed to format FVM");
            let fvm_component = match fs.serve_multi_volume().await {
                Ok(fvm_component) => fvm_component,
                Err(_) => loop {},
            };
            let volumes_connector = {
                let exposed_dir =
                    fuchsia_fs::directory::clone(fvm_component.exposed_dir()).unwrap();
                Box::new(move || {
                    connect_to_protocol_at_dir_root::<VolumesMarker>(&exposed_dir).unwrap()
                })
            };
            let volumes_dir_connector = {
                let exposed_dir =
                    fuchsia_fs::directory::clone(fvm_component.exposed_dir()).unwrap();
                Box::new(move || {
                    fuchsia_fs::directory::open_directory_async(
                        &exposed_dir,
                        "volumes",
                        fio::PERM_READABLE,
                    )
                    .unwrap()
                })
            };
            let fvm = BenchmarkVolumeFactory::connect_to_system_fvm(
                volumes_connector,
                volumes_dir_connector,
            );
            (TestState(ramdisk, fvm_component), fvm.unwrap())
        }
    }

    async fn benchmark_volume_factory_can_find_fvm_instance(config: FvmTestConfig) {
        let (_state, volume_factory) = initialize(config).await;

        // Verify that a volume can be created.
        volume_factory
            .create_block_device(&BlockDeviceConfig {
                requires_fvm: true,
                use_zxcrypt: false,
                volume_size: None,
            })
            .await;
    }

    #[fuchsia::test]
    async fn benchmark_volume_factory_can_find_fvm_instance_fvm() {
        benchmark_volume_factory_can_find_fvm_instance(FvmTestConfig { fxblob_enabled: false })
            .await;
    }

    #[fuchsia::test]
    async fn benchmark_volume_factory_can_find_fvm_instance_gpt() {
        benchmark_volume_factory_can_find_fvm_instance(FvmTestConfig { fxblob_enabled: true })
            .await;
    }

    async fn dropping_an_fvm_volume_removes_the_volume(config: FvmTestConfig) {
        let (_state, volume_factory) = initialize(config).await;
        {
            let _volume = volume_factory
                .create_block_device(&BlockDeviceConfig {
                    requires_fvm: true,
                    use_zxcrypt: false,
                    volume_size: None,
                })
                .await;
            assert!(volume_factory.contains_fvm_volume(BENCHMARK_VOLUME_NAME).await);
        };
        assert!(!volume_factory.contains_fvm_volume(BENCHMARK_VOLUME_NAME).await);
    }

    #[fuchsia::test]
    async fn dropping_an_fvm_volume_removes_the_volume_fvm() {
        dropping_an_fvm_volume_removes_the_volume(FvmTestConfig { fxblob_enabled: false }).await;
    }

    async fn benchmark_volume_factory_create_block_device_with_zxcrypt(config: FvmTestConfig) {
        let (_state, volume_factory) = initialize(config).await;
        let _ = volume_factory
            .create_block_device(&BlockDeviceConfig {
                requires_fvm: true,
                use_zxcrypt: true,
                volume_size: None,
            })
            .await;
    }

    #[fuchsia::test]
    async fn benchmark_volume_factory_create_block_device_with_zxcrypt_fvm() {
        benchmark_volume_factory_create_block_device_with_zxcrypt(FvmTestConfig {
            fxblob_enabled: false,
        })
        .await;
    }

    #[fuchsia::test]
    async fn benchmark_volume_factory_create_block_device_with_zxcrypt_gpt() {
        benchmark_volume_factory_create_block_device_with_zxcrypt(FvmTestConfig {
            fxblob_enabled: true,
        })
        .await;
    }
}
