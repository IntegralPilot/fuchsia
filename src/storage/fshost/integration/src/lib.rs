// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use assert_matches::assert_matches;
use diagnostics_assertions::assert_data_tree;
use diagnostics_reader::ArchiveReader;
use disk_builder::Disk;
use fake_keymint::FakeKeymint;
use fidl::endpoints::{ServiceMarker as _, create_proxy};
use fidl_fuchsia_boot as fboot;
use fidl_fuchsia_driver_test as fdt;
use fidl_fuchsia_driver_token as ftoken;
use fidl_fuchsia_feedback as ffeedback;
use fidl_fuchsia_fshost_fxfsprovisioner as ffxfsprovisioner;
use fidl_fuchsia_fxfs::{BlobReaderMarker, CryptManagementProxy, CryptProxy, KeyPurpose};
use fidl_fuchsia_hardware_block_volume as fvolume;
use fidl_fuchsia_hardware_inlineencryption as finline;
use fidl_fuchsia_hardware_ramdisk as framdisk;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_security_keymint as fkeymint;
use fidl_fuchsia_storage_block as fblock;
use fidl_fuchsia_storage_partitions as fpartitions;
use fuchsia_async::{self as fasync, TimeoutExt as _};
use fuchsia_component::client::{
    connect_to_named_protocol_at_dir_root, connect_to_protocol_at_dir_root,
};
use fuchsia_component_test::{Capability, ChildOptions, RealmBuilder, RealmInstance, Ref, Route};
use fuchsia_driver_test::{DriverTestRealmBuilder, DriverTestRealmInstance};
use futures::channel::mpsc;
use futures::{FutureExt as _, StreamExt as _};
use ramdevice_client::{RamdiskClient, RamdiskClientBuilder};
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;
use test_vmo_backed_block_server::VmoBackedServer;

pub mod disk_builder;
mod mocks;

pub use disk_builder::write_blob;
pub use fshost_assembly_config::{BlockDeviceConfig, BlockDeviceIdentifiers, BlockDeviceParent};

pub const VFS_TYPE_BLOBFS: u32 = 0x9e694d21;
pub const VFS_TYPE_MINFS: u32 = 0x6e694d21;
pub const VFS_TYPE_MEMFS: u32 = 0x3e694d21;
pub const VFS_TYPE_FXFS: u32 = 0x73667866;
pub const VFS_TYPE_F2FS: u32 = 0xfe694d21;
pub const STARNIX_VOLUME_NAME: &str = "starnix_volume";

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

async fn with_timeout<F: std::future::Future>(fut: F, name: impl Into<String>) -> F::Output {
    let name = name.into();
    fut.on_timeout(DEFAULT_TIMEOUT, move || panic!("{name} timed out after {DEFAULT_TIMEOUT:?}"))
        .await
}

pub fn round_down<
    T: Into<U>,
    U: Copy + std::ops::Rem<U, Output = U> + std::ops::Sub<U, Output = U>,
>(
    offset: U,
    block_size: T,
) -> U {
    let block_size = block_size.into();
    offset - offset % block_size
}

pub struct TestFixtureBuilder {
    no_fuchsia_boot: bool,
    disk: Option<Disk>,
    extra_disks: Vec<Disk>,
    fshost: fshost_testing::FshostBuilder,
    zbi_ramdisk: Option<disk_builder::DiskBuilder>,
    force_fxfs_provisioner_failure: bool,
    keymint: std::sync::Arc<FakeKeymint>,
    crypt_policy: crypt_policy::Policy,
    simulated_gpt: Option<Arc<VmoBackedServer>>,
}

impl TestFixtureBuilder {
    pub fn new(fshost_component_name: &'static str) -> Self {
        Self {
            no_fuchsia_boot: false,
            disk: None,
            extra_disks: Vec::new(),
            fshost: fshost_testing::FshostBuilder::new(fshost_component_name),
            zbi_ramdisk: None,
            force_fxfs_provisioner_failure: false,
            keymint: std::sync::Arc::new(FakeKeymint::default()),
            crypt_policy: crypt_policy::Policy::Null,
            simulated_gpt: None,
        }
    }

    pub fn fshost(&mut self) -> &mut fshost_testing::FshostBuilder {
        &mut self.fshost
    }

    pub fn keymint(&mut self) -> std::sync::Arc<FakeKeymint> {
        self.keymint.clone()
    }

    pub fn with_keymint_instance(mut self, keymint: std::sync::Arc<FakeKeymint>) -> Self {
        self.keymint = keymint.clone();
        if let Some(Disk::Builder(ref mut disk_builder)) = self.disk {
            disk_builder.with_keymint_instance(keymint.clone());
        }
        for disk in &mut self.extra_disks {
            if let Disk::Builder(disk_builder) = disk {
                disk_builder.with_keymint_instance(keymint.clone());
            }
        }
        self
    }

    pub fn with_disk(&mut self) -> &mut disk_builder::DiskBuilder {
        self.disk = Some(Disk::Builder(disk_builder::DiskBuilder::new()));
        self.disk
            .as_mut()
            .unwrap()
            .builder()
            .with_crypt_policy(self.crypt_policy)
            .with_keymint_instance(self.keymint.clone());
        self.disk.as_mut().unwrap().builder()
    }

    pub fn with_extra_disk(&mut self) -> &mut disk_builder::DiskBuilder {
        self.extra_disks.push(Disk::Builder(disk_builder::DiskBuilder::new()));
        self.extra_disks
            .last_mut()
            .unwrap()
            .builder()
            .with_crypt_policy(self.crypt_policy)
            .with_keymint_instance(self.keymint.clone());
        self.extra_disks.last_mut().unwrap().builder()
    }

    pub fn with_uninitialized_disk(mut self) -> Self {
        self.disk = Some(Disk::Builder(disk_builder::DiskBuilder::uninitialized()));
        self
    }

    pub fn with_disk_from(mut self, disk: Disk) -> Self {
        self.disk = Some(disk);
        self
    }

    pub fn with_simulated_gpt(mut self, server: Arc<VmoBackedServer>) -> Self {
        self.simulated_gpt = Some(server);
        self
    }

    pub fn with_zbi_ramdisk(&mut self) -> &mut disk_builder::DiskBuilder {
        self.zbi_ramdisk = Some(disk_builder::DiskBuilder::new());
        self.zbi_ramdisk.as_mut().unwrap()
    }

    pub fn no_fuchsia_boot(mut self) -> Self {
        self.no_fuchsia_boot = true;
        self
    }

    pub fn with_device_config(mut self, device_config: Vec<BlockDeviceConfig>) -> Self {
        self.fshost.set_device_config(device_config);
        self
    }

    pub fn with_crypt_policy(mut self, policy: crypt_policy::Policy) -> Self {
        self.fshost.set_crypt_policy(policy);
        self.crypt_policy = policy;
        if let Some(Disk::Builder(ref mut disk_builder)) = self.disk {
            disk_builder.with_crypt_policy(policy);
        }
        for disk in &mut self.extra_disks {
            if let Disk::Builder(disk_builder) = disk {
                disk_builder.with_crypt_policy(policy);
            }
        }
        self
    }

    pub fn force_fxfs_provisioner_failure(mut self) -> Self {
        self.force_fxfs_provisioner_failure = true;
        self
    }

    pub async fn build(self) -> TestFixture {
        let builder = RealmBuilder::new().await.unwrap();
        let fshost = self.fshost.build(&builder).await;
        // Route fshost's inline encryption Device capability to the parent.
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<finline::DeviceMarker>())
                    .from(&fshost)
                    .to(Ref::parent()),
            )
            .await
            .unwrap();

        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<fidl_fuchsia_component::RealmMarker>())
                    .from(Ref::framework())
                    .to(Ref::parent()),
            )
            .await
            .unwrap();

        let maybe_zbi_vmo = match self.zbi_ramdisk {
            Some(disk_builder) => Some(disk_builder.build_as_zbi_ramdisk().await),
            None => None,
        };
        let (tx, crash_reports) = mpsc::channel(32);
        let mocks = mocks::new_mocks(
            maybe_zbi_vmo,
            tx,
            self.force_fxfs_provisioner_failure,
            self.keymint.clone(),
            self.simulated_gpt.clone(),
        );

        let mocks = builder
            .add_local_child("mocks", move |h| mocks(h).boxed(), ChildOptions::new())
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<fkeymint::SealingKeysMarker>())
                    .capability(Capability::protocol::<fkeymint::AdminMarker>())
                    .from(&mocks)
                    .to(Ref::parent()),
            )
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<ffeedback::CrashReporterMarker>())
                    .capability(Capability::protocol::<ffxfsprovisioner::FxfsProvisionerMarker>())
                    .capability(Capability::protocol::<fkeymint::SealingKeysMarker>())
                    .capability(Capability::protocol::<fkeymint::AdminMarker>())
                    .capability(Capability::protocol::<ftoken::NodeBusTopologyMarker>())
                    .from(&mocks)
                    .to(&fshost),
            )
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(Capability::service::<fvolume::ServiceMarker>())
                    .from(&mocks)
                    .to(&fshost),
            )
            .await
            .unwrap();
        if !self.no_fuchsia_boot {
            builder
                .add_route(
                    Route::new()
                        .capability(Capability::protocol::<fboot::ArgumentsMarker>())
                        .capability(Capability::protocol::<fboot::ItemsMarker>())
                        .from(&mocks)
                        .to(&fshost),
                )
                .await
                .unwrap();
        }

        builder
            .add_route(
                Route::new()
                    .capability(Capability::dictionary("diagnostics"))
                    .from(Ref::parent())
                    .to(&fshost),
            )
            .await
            .unwrap();

        let dtr_exposes = vec![
            fidl_fuchsia_component_test::Capability::Service(
                fidl_fuchsia_component_test::Service {
                    name: Some("fuchsia.hardware.ramdisk.Service".to_owned()),
                    ..Default::default()
                },
            ),
            fidl_fuchsia_component_test::Capability::Service(
                fidl_fuchsia_component_test::Service {
                    name: Some("fuchsia.hardware.block.volume.Service".to_owned()),
                    ..Default::default()
                },
            ),
        ];
        builder.driver_test_realm_setup().await.unwrap();
        builder.driver_test_realm_add_dtr_exposes(&dtr_exposes).await.unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(Capability::directory("dev-topological").rights(fio::R_STAR_DIR))
                    .capability(Capability::service::<fvolume::ServiceMarker>())
                    .from(Ref::child(fuchsia_driver_test::COMPONENT_NAME))
                    .to(&fshost),
            )
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(
                        Capability::directory("dev-class")
                            .rights(fio::R_STAR_DIR)
                            .subdir("block")
                            .as_("dev-class-block"),
                    )
                    .from(Ref::child(fuchsia_driver_test::COMPONENT_NAME))
                    .to(Ref::parent()),
            )
            .await
            .unwrap();

        let realm = builder.build().await.unwrap();
        let realm_proxy = connect_to_protocol_at_dir_root::<fidl_fuchsia_component::RealmMarker>(
            realm.root.get_exposed_dir(),
        )
        .expect("failed to connect to Realm");
        let (controller, controller_server_end) =
            create_proxy::<fidl_fuchsia_component::ControllerMarker>();
        realm_proxy
            .open_controller(
                &fidl_fuchsia_component_decl::ChildRef {
                    name: "test-fshost".to_string(),
                    collection: None,
                },
                controller_server_end,
            )
            .await
            .expect("failed to open controller")
            .expect("open controller error");
        let (execution_controller, execution_controller_server_end) =
            create_proxy::<fidl_fuchsia_component::ExecutionControllerMarker>();
        controller
            .start(
                fidl_fuchsia_component::StartChildArgs::default(),
                execution_controller_server_end,
            )
            .await
            .expect("failed to start fshost")
            .expect("start fshost error");

        let mut fixture = TestFixture {
            realm,
            ramdisks: Vec::new(),
            main_disk: None,
            crash_reports,
            execution_controller,
            torn_down: TornDown(false),
        };

        log::info!(
            realm_name:? = fixture.realm.root.child_name();
            "built new test realm",
        );

        fixture
            .realm
            .driver_test_realm_start(fdt::RealmArgs {
                root_driver: Some("fuchsia-boot:///platform-bus#meta/platform-bus.cm".to_owned()),
                dtr_exposes: Some(dtr_exposes),
                software_devices: Some(vec![
                    fdt::SoftwareDevice {
                        device_name: "ram-disk".to_string(),
                        device_id: bind_fuchsia_platform::BIND_PLATFORM_DEV_DID_RAM_DISK,
                    },
                    fdt::SoftwareDevice {
                        device_name: "ram-nand".to_string(),
                        device_id: bind_fuchsia_platform::BIND_PLATFORM_DEV_DID_RAM_NAND,
                    },
                ]),
                ..Default::default()
            })
            .await
            .unwrap();

        // The order of adding disks matters here, unfortunately. fshost should not change behavior
        // based on the order disks appear, but because we take the first available that matches
        // whatever relevant criteria, it's useful to test that matchers don't get clogged up by
        // previous disks.
        // TODO(https://fxbug.dev/380353856): This type of testing should be irrelevant once the
        // block devices are determined by configuration options instead of heuristically.
        for disk in self.extra_disks.into_iter() {
            fixture.add_disk(disk).await;
        }
        if let Some(disk) = self.disk {
            fixture.add_main_disk(disk).await;
        }

        fixture
    }
}

/// Create a separate struct that does the drop-assert because fixture.tear_down can't call
/// realm.destroy if it has the drop impl itself.
struct TornDown(bool);

impl Drop for TornDown {
    fn drop(&mut self) {
        // Because tear_down is async, it needs to be called by the test in an async context. It
        // checks some properties so for correctness it must be called.
        assert!(self.0, "fixture.tear_down() must be called");
    }
}

pub struct TestFixture {
    pub realm: RealmInstance,
    pub ramdisks: Vec<RamdiskClient>,
    pub main_disk: Option<Disk>,
    pub crash_reports: mpsc::Receiver<ffeedback::CrashReport>,
    pub execution_controller: fidl_fuchsia_component::ExecutionControllerProxy,
    torn_down: TornDown,
}

impl TestFixture {
    pub async fn tear_down(mut self) -> Option<Disk> {
        log::info!(realm_name:? = self.realm.root.child_name(); "tearing down");
        let disk = self.main_disk.take();
        // Check the crash reports before destroying the realm because tearing down the realm can
        // cause mounting errors that trigger a crash report.
        assert_matches!(self.crash_reports.try_next(), Ok(None) | Err(_));
        self.realm.destroy().await.unwrap();
        self.torn_down.0 = true;
        disk
    }

    pub fn exposed_dir(&self) -> &fio::DirectoryProxy {
        self.realm.root.get_exposed_dir()
    }

    pub fn dir(&self, dir: &str, flags: fio::Flags) -> fio::DirectoryProxy {
        let (dev, server) = create_proxy::<fio::DirectoryMarker>();
        let flags = flags | fio::Flags::PROTOCOL_DIRECTORY;
        self.realm
            .root
            .get_exposed_dir()
            .open(dir, flags, &fio::Options::default(), server.into_channel())
            .expect("open failed");
        dev
    }

    pub async fn check_fs_type(&self, dir: &str, fs_type: u32) {
        let (status, info) = with_timeout(
            self.dir(dir, fio::PERM_READABLE).query_filesystem(),
            format!("check_fs_type({dir})"),
        )
        .await
        .expect("query failed");
        assert_eq!(status, zx::sys::ZX_OK);
        assert!(info.is_some());
        let info_type = info.unwrap().fs_type;
        assert_eq!(info_type, fs_type, "{:#08x} != {:#08x}", info_type, fs_type);
    }

    pub async fn check_test_blob(&self) {
        with_timeout(
            async {
                let expected_blob_hash = disk_builder::test_blob_hash();
                let reader = connect_to_protocol_at_dir_root::<BlobReaderMarker>(
                    self.realm.root.get_exposed_dir(),
                )
                .expect("failed to connect to the BlobReader");
                let _vmo = reader
                    .get_vmo(&expected_blob_hash.into())
                    .await
                    .expect("blob get_vmo fidl error")
                    .unwrap_or_else(|e| match zx::Status::err_from_raw(e) {
                        zx::Status::NOT_FOUND => panic!("Test blob not found - blobfs lost data!"),
                        s => panic!("Error while opening test blob vmo: {s}"),
                    });
            },
            "check_test_blob",
        )
        .await
    }

    /// Check for the existence of a well-known set of test files in the data volume. These files
    /// are placed by the disk builder if it formats the filesystem beforehand.
    pub async fn check_test_data_file(&self) {
        with_timeout(
            async {
                let (file, server) = create_proxy::<fio::NodeMarker>();
                self.dir("data", fio::PERM_READABLE)
                    .open(
                        ".testdata",
                        fio::PERM_READABLE,
                        &fio::Options::default(),
                        server.into_channel(),
                    )
                    .expect("open failed");
                file.get_attributes(fio::NodeAttributesQuery::empty())
                    .await
                    .expect("Fidl transport error on get_attributes()")
                    .expect("get_attr failed - data was probably deleted!");

                let data = self.dir("data", fio::PERM_READABLE);
                fuchsia_fs::directory::open_file(&data, ".testdata", fio::PERM_READABLE)
                    .await
                    .unwrap();

                fuchsia_fs::directory::open_directory(&data, "ssh", fio::PERM_READABLE)
                    .await
                    .unwrap();
                fuchsia_fs::directory::open_directory(&data, "ssh/config", fio::PERM_READABLE)
                    .await
                    .unwrap();
                fuchsia_fs::directory::open_directory(&data, "problems", fio::PERM_READABLE)
                    .await
                    .unwrap();

                let authorized_keys = fuchsia_fs::directory::open_file(
                    &data,
                    "ssh/authorized_keys",
                    fio::PERM_READABLE,
                )
                .await
                .unwrap();
                assert_eq!(
                    &fuchsia_fs::file::read_to_string(&authorized_keys).await.unwrap(),
                    "public key!"
                );
            },
            "check_test_data_file",
        )
        .await
    }

    /// Checks for the absence of the .testdata marker file, indicating the data filesystem was
    /// reformatted.
    pub async fn check_test_data_file_absent(&self) {
        let err = with_timeout(
            fuchsia_fs::directory::open_file(
                &self.dir("data", fio::PERM_READABLE),
                ".testdata",
                fio::PERM_READABLE,
            ),
            "check_test_data_file_absent",
        )
        .await
        .expect_err("open_file failed");
        assert!(err.is_not_found_error());
    }

    pub async fn add_main_disk(&mut self, disk: Disk) {
        assert!(self.main_disk.is_none());
        let (vmo, type_guid) = disk.into_vmo_and_type_guid().await;
        let vmo_clone =
            vmo.create_child(zx::VmoChildOptions::SLICE, 0, vmo.get_size().unwrap()).unwrap();

        self.add_ramdisk(vmo, type_guid).await;
        self.main_disk = Some(Disk::Prebuilt(vmo_clone, type_guid));
    }

    pub async fn add_disk(&mut self, disk: Disk) {
        let (vmo, type_guid) = disk.into_vmo_and_type_guid().await;
        self.add_ramdisk(vmo, type_guid).await;
    }

    async fn add_ramdisk(&mut self, vmo: zx::Vmo, type_guid: Option<[u8; 16]>) {
        let mut ramdisk_builder = RamdiskClientBuilder::new_with_vmo(vmo, Some(512))
            .publish()
            .ramdisk_service(self.dir(framdisk::ServiceMarker::SERVICE_NAME, fio::PERM_READABLE));
        if let Some(guid) = type_guid {
            ramdisk_builder = ramdisk_builder.guid(guid);
        }
        let mut ramdisk = pin!(ramdisk_builder.build().fuse());

        let ramdisk = futures::select_biased!(
            res = ramdisk => res,
            _ = fasync::Timer::new(Duration::from_secs(120))
                .fuse() => panic!("Timed out waiting for RamdiskClient"),
        )
        .unwrap();
        self.ramdisks.push(ramdisk);
    }

    pub fn connect_to_crypt(&self) -> CryptProxy {
        self.realm
            .root
            .connect_to_protocol_at_exposed_dir()
            .expect("connect_to_protocol_at_exposed_dir failed for the Crypt protocol")
    }

    pub async fn setup_starnix_crypt(&self) -> (CryptProxy, CryptManagementProxy) {
        let crypt_management: CryptManagementProxy =
            self.realm.root.connect_to_protocol_at_exposed_dir().expect(
                "connect_to_protocol_at_exposed_dir failed for the CryptManagement protocol",
            );
        let crypt = self
            .realm
            .root
            .connect_to_protocol_at_exposed_dir()
            .expect("connect_to_protocol_at_exposed_dir failed for the Crypt protocol");
        let key = vec![0xABu8; 32];
        crypt_management
            .add_wrapping_key(&u128::to_le_bytes(0), key.as_slice())
            .await
            .expect("fidl transport error")
            .expect("add wrapping key failed");
        crypt_management
            .add_wrapping_key(&u128::to_le_bytes(1), key.as_slice())
            .await
            .expect("fidl transport error")
            .expect("add wrapping key failed");
        crypt_management
            .set_active_key(KeyPurpose::Data, &u128::to_le_bytes(0))
            .await
            .expect("fidl transport error")
            .expect("set metadata key failed");
        crypt_management
            .set_active_key(KeyPurpose::Metadata, &u128::to_le_bytes(1))
            .await
            .expect("fidl transport error")
            .expect("set metadata key failed");
        (crypt, crypt_management)
    }

    /// This must be called if any crash reports are expected, since spurious reports will cause a
    /// failure in TestFixture::tear_down.
    pub async fn wait_for_crash_reports(
        &mut self,
        count: usize,
        expected_program: &'_ str,
        expected_signature: &'_ str,
    ) {
        log::info!("Waiting for {count} crash reports");
        for _ in 0..count {
            let report = self.crash_reports.next().await.expect("Sender closed");
            assert_eq!(report.program_name.as_deref(), Some(expected_program));
            assert_eq!(report.crash_signature.as_deref(), Some(expected_signature));
        }
        if count > 0 {
            let selector =
                format!("realm_builder\\:{}/test-fshost:root", self.realm.root.child_name());
            log::info!("Checking inspect for corruption event, selector={selector}");
            let tree = ArchiveReader::inspect()
                .add_selector(selector)
                .snapshot()
                .await
                .unwrap()
                .into_iter()
                .next()
                .and_then(|result| result.payload)
                .expect("expected one inspect hierarchy");

            let format = || expected_program.to_string();
            if expected_signature.contains("unseal-error") {
                assert_data_tree!(tree, root: contains {
                    keymint_unseal_failure_events: contains {
                        format() => 1u64,
                    }
                });
            } else {
                assert_data_tree!(tree, root: contains {
                    corruption_events: contains {
                        format() => 1u64,
                    }
                });
            }
        }
    }

    // Check that the system partition table contains partitions with labels found in `expected`.
    pub async fn check_system_partitions(&self, mut expected: Vec<&str>) {
        with_timeout(
            async {
                let partitions =
                    self.dir(fpartitions::PartitionServiceMarker::SERVICE_NAME, fio::PERM_READABLE);
                let entries = fuchsia_fs::directory::readdir(&partitions)
                    .await
                    .expect("Failed to read partitions");

                assert_eq!(entries.len(), expected.len());

                let mut found_partition_labels = Vec::new();
                for entry in entries {
                    let endpoint_name = format!("{}/volume", entry.name);
                    let volume = connect_to_named_protocol_at_dir_root::<fblock::BlockMarker>(
                        &partitions,
                        &endpoint_name,
                    )
                    .expect("failed to connect to named protocol at dir root");
                    let (raw_status, label) =
                        volume.get_name().await.expect("failed to call get_name");
                    zx::Status::ok(raw_status).expect("get_name status failed");
                    found_partition_labels
                        .push(label.expect("partition label expected to be some value"));
                }
                found_partition_labels.sort();
                expected.sort();
                assert_eq!(found_partition_labels, expected);
            },
            "check_system_partitions",
        )
        .await
    }
}
