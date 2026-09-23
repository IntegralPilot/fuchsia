// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use blob_writer::BlobWriter;
use block_client::{BlockClient as _, RemoteBlockClient};
use block_matcher::{BlockDeviceMatcher, find_block_device};
use delivery_blob::{CompressionMode, Type1Blob};
use fake_keymint::{FakeKeymint, with_keymint_service};
use fidl_fuchsia_fs_startup::{CreateOptions, MountOptions};
use fidl_fuchsia_fxfs::{
    BlobCreatorProxy, CryptManagementMarker, CryptManagementProxy, CryptMarker, KeyPurpose,
};
use fidl_fuchsia_io as fio;
use fidl_fuchsia_logger as flogger;
use fidl_fuchsia_storage_block as fblock;
use fs_management::filesystem::{
    BlockConnector, DirBasedBlockConnector, Filesystem, ServingMultiVolumeFilesystem,
};
use fs_management::format::constants::{F2FS_MAGIC, FXFS_MAGIC, MINFS_MAGIC};
use fs_management::{BLOBFS_TYPE_GUID, DATA_TYPE_GUID, FVM_TYPE_GUID, Fvm, Fxfs};
use fuchsia_async as fasync;
use fuchsia_component::client::connect_to_protocol_at_dir_svc;
use fuchsia_component_test::{Capability, ChildOptions, RealmBuilder, RealmInstance, Ref, Route};
use fuchsia_hash::Hash;
use gpt_component::gpt::GptManager;
use key_bag::Aes256Key;
use serde_json::json;
use std::collections::HashSet;
use std::ops::Deref;
use std::sync::Arc;
use test_vmo_backed_block_server::{VmoBackedServer, VmoBackedServerConnector};
use uuid::Uuid;
use zerocopy::IntoBytes;

pub const TEST_DISK_BLOCK_SIZE: u32 = 512;
pub const FVM_SLICE_SIZE: u64 = 32 * 1024;
pub const FVM_F2FS_SLICE_SIZE: u64 = 2 * 1024 * 1024;

// The default disk size is about 55MiB, with about 51MiB dedicated to the data volume. This size
// is chosen because the data volume has to be big enough to support f2fs (>= DEFAULT_F2FS_MIN_BYTES
// defined in //src/storage/fshost/device/constants.rs), which has a relatively large minimum size
// requirement to be formatted.
//
// Only the data volume is actually created with a specific size, the other volumes aren't passed
// any sizes. Blobfs can resize itself on the fvm, and the other two potential volumes are only
// used in specific circumstances and are never formatted. The remaining volume size is just used
// for calculation.
pub const DEFAULT_F2FS_MIN_BYTES: u64 = 50 * 1024 * 1024;
pub const DEFAULT_DATA_VOLUME_SIZE: u64 = DEFAULT_F2FS_MIN_BYTES;
pub const BLOBFS_MAX_BYTES: u64 = 8765432;
// For migration tests, we make sure that the default disk size is twice the data volume size to
// allow a second full data partition.
pub const DEFAULT_DISK_SIZE: u64 = DEFAULT_DATA_VOLUME_SIZE * 2 + BLOBFS_MAX_BYTES;

// We use a static key-bag so that the crypt instance can be shared across test executions safely.
// These keys match the DATA_KEY and METADATA_KEY respectively, when wrapped with the "zxcrypt"
// static key used by fshost.
// Note this isn't used in the legacy crypto format.
const KEY_BAG_CONTENTS: &'static str = r#"
{
    "version":1,
    "keys": {
        "0":{
            "Aes128GcmSivWrapped": [
                "7a7c6a718cfde7078f6edec5",
                "7cc31b765c74db3191e269d2666267022639e758fe3370e8f36c166d888586454fd4de8aeb47aadd81c531b0a0a66f27"
            ]
        },
        "1":{
            "Aes128GcmSivWrapped": [
                "b7d7f459cbee4cc536cc4324",
                "9f6a5d894f526b61c5c091e5e02a7ff94d18e6ad36a0aa439c86081b726eca79e6b60bd86ee5d86a20b3df98f5265a99"
            ]
        }
    }
}"#;

async fn generate_keymint_file_contents(
    old_blob: Option<&[u8]>,
    custom_keymint: Option<Arc<FakeKeymint>>,
) -> Vec<u8> {
    let serve_keymint = |keymint_proxy: fidl_fuchsia_security_keymint::SealingKeysProxy| async move {
        let key_info = b"fuchsia";
        let key_blob = keymint_proxy.create_sealing_key(&key_info[..]).await.unwrap().unwrap();
        assert!(!key_blob.is_empty());

        let data_key_sealed =
            keymint_proxy.seal(&key_info[..], &key_blob, DATA_KEY.deref()).await.unwrap().unwrap();
        let metadata_key_sealed = keymint_proxy
            .seal(&key_info[..], &key_blob, METADATA_KEY.deref())
            .await
            .unwrap()
            .unwrap();

        let mut json = json!({
            "sealing_key_info": key_info,
            "sealing_key_blob": key_blob,
            "sealed_keys": {
                "data.data": data_key_sealed,
                "data.metadata": metadata_key_sealed,
            }
        });

        if let Some(old) = old_blob {
            json.as_object_mut().unwrap().insert(
                "old_blob".to_string(),
                serde_json::Value::Array(
                    old.iter().map(|&b| serde_json::Value::Number(b.into())).collect(),
                ),
            );
        }

        serde_json::to_vec_pretty(&json).unwrap()
    };

    if let Some(fake_keymint) = custom_keymint {
        // When we have a custom injected mock FakeKeymint, we don't need to spin up FIDL proxies
        // and background tasks. The mock natively generates and registers its own bytes.
        let key_info = b"fuchsia".to_vec();

        // We simulate what the hardware would do during an initial "seal" of data and
        // metadata keys.
        let key_blob = fake_keymint.generate_static_sealing_key(&key_info);

        let data_key_sealed =
            fake_keymint.generate_static_sealed_data(&key_info, &key_blob, DATA_KEY.deref());
        let metadata_key_sealed =
            fake_keymint.generate_static_sealed_data(&key_info, &key_blob, METADATA_KEY.deref());

        let mut json = json!({
            "sealing_key_info": key_info,
            "sealing_key_blob": key_blob,
            "sealed_keys": {
                "data.data": data_key_sealed,
                "data.metadata": metadata_key_sealed,
            }
        });

        if let Some(old) = old_blob {
            json.as_object_mut().unwrap().insert(
                "old_blob".to_string(),
                serde_json::Value::Array(
                    old.iter().map(|&b| serde_json::Value::Number(b.into())).collect(),
                ),
            );
        }

        serde_json::to_vec_pretty(&json).unwrap()
    } else {
        with_keymint_service(|proxy, _| async move { Ok(serve_keymint(proxy.into_proxy()).await) })
            .await
            .unwrap()
    }
}

pub const TEST_BLOB_CONTENTS: [u8; 1000] = [1; 1000];

pub fn test_blob_hash() -> fuchsia_merkle::Hash {
    fuchsia_merkle::root_from_slice(&TEST_BLOB_CONTENTS)
}

const DATA_KEY: Aes256Key = Aes256Key::create([
    0xcf, 0x9e, 0x45, 0x2a, 0x22, 0xa5, 0x70, 0x31, 0x33, 0x3b, 0x4d, 0x6b, 0x6f, 0x78, 0x58, 0x29,
    0x04, 0x79, 0xc7, 0xd6, 0xa9, 0x4b, 0xce, 0x82, 0x04, 0x56, 0x5e, 0x82, 0xfc, 0xe7, 0x37, 0xa8,
]);

const METADATA_KEY: Aes256Key = Aes256Key::create([
    0x0f, 0x4d, 0xca, 0x6b, 0x35, 0x0e, 0x85, 0x6a, 0xb3, 0x8c, 0xdd, 0xe9, 0xda, 0x0e, 0xc8, 0x22,
    0x8e, 0xea, 0xd8, 0x05, 0xc4, 0xc9, 0x0b, 0xa8, 0xd8, 0x85, 0x87, 0x50, 0x75, 0x40, 0x1c, 0x4c,
]);

pub const FVM_PART_INSTANCE_GUID: [u8; 16] = [3u8; 16];
pub const DEFAULT_TEST_TYPE_GUID: [u8; 16] = [
    0x66, 0x73, 0x68, 0x6F, 0x73, 0x74, 0x20, 0x69, 0x6E, 0x74, 0x65, 0x67, 0x72, 0x61, 0x74, 0x73,
];

async fn create_hermetic_crypt_service(
    data_key: Aes256Key,
    metadata_key: Aes256Key,
) -> RealmInstance {
    let builder = RealmBuilder::new().await.unwrap();
    let url = "#meta/fxfs-crypt.cm";
    let crypt = builder.add_child("fxfs-crypt", url, ChildOptions::new().eager()).await.unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<CryptMarker>())
                .capability(Capability::protocol::<CryptManagementMarker>())
                .from(&crypt)
                .to(Ref::parent()),
        )
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<flogger::LogSinkMarker>())
                .from(Ref::parent())
                .to(&crypt),
        )
        .await
        .unwrap();
    let realm = builder.build().await.expect("realm build failed");
    let crypt_management: CryptManagementProxy =
        realm.root.connect_to_protocol_at_exposed_dir().unwrap();
    let wrapping_key_id_0 = [0; 16];
    let mut wrapping_key_id_1 = [0; 16];
    wrapping_key_id_1[0] = 1;
    crypt_management
        .add_wrapping_key(&wrapping_key_id_0, data_key.deref())
        .await
        .unwrap()
        .expect("add_wrapping_key failed");
    crypt_management
        .add_wrapping_key(&wrapping_key_id_1, metadata_key.deref())
        .await
        .unwrap()
        .expect("add_wrapping_key failed");
    crypt_management
        .set_active_key(KeyPurpose::Data, &wrapping_key_id_0)
        .await
        .unwrap()
        .expect("set_active_key failed");
    crypt_management
        .set_active_key(KeyPurpose::Metadata, &wrapping_key_id_1)
        .await
        .unwrap()
        .expect("set_active_key failed");
    realm
}

/// Write a blob to the blob volume to ensure that on format, the blob volume does not get wiped.
pub async fn write_blob(blob_creator: BlobCreatorProxy, data: &[u8]) -> Hash {
    let hash = fuchsia_merkle::root_from_slice(data);
    let compressed_data = Type1Blob::generate(&data, CompressionMode::Always);

    let blob_writer_client_end = blob_creator
        .create(&hash.into(), false)
        .await
        .expect("transport error on create")
        .expect("failed to create blob");

    let writer = blob_writer_client_end.into_proxy();
    let mut blob_writer = BlobWriter::create(writer, compressed_data.len() as u64)
        .await
        .expect("failed to create BlobWriter");
    blob_writer.write(&compressed_data).await.unwrap();
    hash
}

#[allow(clippy::large_enum_variant)]
pub enum Disk {
    Prebuilt(zx::Vmo, Option<[u8; 16]>),
    Builder(DiskBuilder),
}

impl Disk {
    pub async fn into_vmo_and_type_guid(self) -> (zx::Vmo, Option<[u8; 16]>) {
        match self {
            Disk::Prebuilt(vmo, guid) => (vmo, guid),
            Disk::Builder(builder) => builder.build().await,
        }
    }

    pub fn builder(&mut self) -> &mut DiskBuilder {
        match self {
            Disk::Prebuilt(..) => panic!("attempted to get builder for prebuilt disk"),
            Disk::Builder(builder) => builder,
        }
    }
}

#[derive(Debug)]
pub struct DataSpec {
    pub format: Option<&'static str>,
    pub zxcrypt: bool,
    pub crypt_policy: crypt_policy::Policy,
}

impl Default for DataSpec {
    fn default() -> Self {
        Self {
            format: Default::default(),
            zxcrypt: Default::default(),
            crypt_policy: crypt_policy::Policy::Null,
        }
    }
}

#[derive(Debug)]
pub struct VolumesSpec {
    pub fxfs_blob: bool,
    pub create_data_partition: bool,
}

enum FxfsType {
    Fxfs(Box<dyn BlockConnector>),
    FxBlob(ServingMultiVolumeFilesystem, RealmInstance),
}

pub struct DiskBuilder {
    size: u64,
    // Overrides all other options.  The disk will be unformatted.
    uninitialized: bool,
    blob_hash: Option<Hash>,
    data_volume_size: u64,
    fvm_slice_size: u64,
    data_spec: DataSpec,
    volumes_spec: VolumesSpec,
    // Only used if `format` is Some.
    corrupt_data: bool,
    gpt: bool,
    extra_volumes: Vec<&'static str>,
    extra_gpt_partitions: Vec<(&'static str, u64)>,
    // Note: fvm also means fxfs acting as the volume manager when using fxblob.
    format_volume_manager: bool,
    legacy_data_label: bool,
    // Only used if 'fs_switch' set.
    fs_switch: Option<String>,
    // The type guid of the ramdisk when it's created for the test fshost.
    type_guid: Option<[u8; 16]>,
    system_partition_label: &'static str,
    /// If set, pre-populates the keymint sealing key metadata on disk with
    /// this content as the previous sealing key. Our crypt policy
    /// code should attempt to delete this key when unseal is called.
    keymint_old_blob: Option<Vec<u8>>,
    /// Allows a keymint instance to be provided instead of the default one.
    keymint: Option<std::sync::Arc<FakeKeymint>>,
}

impl std::fmt::Debug for DiskBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiskBuilder")
            .field("size", &self.size)
            .field("uninitialized", &self.uninitialized)
            .field("blob_hash", &self.blob_hash)
            .field("data_volume_size", &self.data_volume_size)
            .field("fvm_slice_size", &self.fvm_slice_size)
            .field("data_spec", &self.data_spec)
            .field("volumes_spec", &self.volumes_spec)
            .field("corrupt_data", &self.corrupt_data)
            .field("gpt", &self.gpt)
            .field("format_volume_manager", &self.format_volume_manager)
            .field("legacy_data_label", &self.legacy_data_label)
            .field("fs_switch", &self.fs_switch)
            .field("type_guid", &self.type_guid)
            .field("system_partition_label", &self.system_partition_label)
            .field("keymint_old_blob", &self.keymint_old_blob)
            .finish_non_exhaustive()
    }
}

impl DiskBuilder {
    pub fn uninitialized() -> DiskBuilder {
        Self { uninitialized: true, type_guid: None, ..Self::new() }
    }

    pub fn new() -> DiskBuilder {
        DiskBuilder {
            size: DEFAULT_DISK_SIZE,
            uninitialized: false,
            blob_hash: None,
            data_volume_size: DEFAULT_DATA_VOLUME_SIZE,
            fvm_slice_size: FVM_SLICE_SIZE,
            data_spec: DataSpec::default(),
            volumes_spec: VolumesSpec { fxfs_blob: false, create_data_partition: true },
            corrupt_data: false,
            gpt: false,
            extra_volumes: Vec::new(),
            extra_gpt_partitions: Vec::new(),
            format_volume_manager: true,
            legacy_data_label: false,
            fs_switch: None,
            type_guid: Some(DEFAULT_TEST_TYPE_GUID),
            system_partition_label: "fvm",
            keymint_old_blob: None,
            keymint: None,
        }
    }

    pub fn with_crypt_policy(&mut self, policy: crypt_policy::Policy) -> &mut Self {
        self.data_spec.crypt_policy = policy;
        self
    }

    pub fn with_keymint_instance(&mut self, keymint: std::sync::Arc<FakeKeymint>) -> &mut Self {
        self.keymint = Some(keymint);
        self
    }

    pub fn set_uninitialized(&mut self) -> &mut Self {
        self.uninitialized = true;
        self
    }

    pub fn with_keymint_old_blob(&mut self, blob: Vec<u8>) -> &mut Self {
        self.keymint_old_blob = Some(blob);
        self
    }

    pub fn size(&mut self, size: u64) -> &mut Self {
        self.size = size;
        self
    }

    pub fn data_volume_size(&mut self, data_volume_size: u64) -> &mut Self {
        self.data_volume_size = data_volume_size;
        // Increase the size of the disk if required. NB: We don't decrease the size of the disk
        // because some tests set a lower initial size and expect to be able to resize to a larger
        // one.
        self.size = self.size.max(self.data_volume_size + BLOBFS_MAX_BYTES);
        self
    }

    pub fn format_volumes(&mut self, volumes_spec: VolumesSpec) -> &mut Self {
        self.volumes_spec = volumes_spec;
        self
    }

    pub fn format_data(&mut self, data_spec: DataSpec) -> &mut Self {
        log::info!(data_spec:?; "formatting data volume");
        if !self.volumes_spec.fxfs_blob {
            assert!(self.format_volume_manager);
        } else {
            if let Some(format) = data_spec.format {
                assert_eq!(format, "fxfs");
            }
        }
        if data_spec.format == Some("f2fs") {
            self.fvm_slice_size = FVM_F2FS_SLICE_SIZE;
        }
        self.data_spec = data_spec;
        self
    }

    pub fn set_fs_switch(&mut self, content: &str) -> &mut Self {
        self.fs_switch = Some(content.to_string());
        self
    }

    pub fn corrupt_data(&mut self) -> &mut Self {
        self.corrupt_data = true;
        self
    }

    pub fn with_gpt(&mut self) -> &mut Self {
        self.gpt = true;
        // The system partition matcher expects the type guid to be either None or completely zero,
        // so if we are formatting with gpt we clear any type guid.
        self.type_guid = None;
        self
    }

    pub fn with_system_partition_label(&mut self, label: &'static str) -> &mut Self {
        self.system_partition_label = label;
        self
    }

    /// Appends an additional GPT partition.  The partitions are contiguous in the inserted order,
    /// and the first one is immediately after the system partition.
    pub fn with_extra_gpt_partition(
        &mut self,
        volume_name: &'static str,
        num_blocks: u64,
    ) -> &mut Self {
        self.extra_gpt_partitions.push((volume_name, num_blocks));
        self
    }

    pub fn with_extra_volume(&mut self, volume_name: &'static str) -> &mut Self {
        self.extra_volumes.push(volume_name);
        self
    }

    pub fn with_unformatted_volume_manager(&mut self) -> &mut Self {
        assert!(self.data_spec.format.is_none());
        self.format_volume_manager = false;
        self
    }

    pub fn with_legacy_data_label(&mut self) -> &mut Self {
        self.legacy_data_label = true;
        self
    }

    pub async fn build(mut self) -> (zx::Vmo, Option<[u8; 16]>) {
        log::info!("building disk: {:?}", self);
        assert_eq!(
            self.data_volume_size % self.fvm_slice_size,
            0,
            "data_volume_size {} needs to be a multiple of fvm slice size {}",
            self.data_volume_size,
            self.fvm_slice_size
        );
        if self.data_spec.format == Some("f2fs") {
            assert!(self.data_volume_size >= DEFAULT_F2FS_MIN_BYTES);
        }

        let vmo = zx::Vmo::create(self.size).unwrap();

        if self.uninitialized {
            return (vmo, self.type_guid);
        }

        let server = Arc::new(
            VmoBackedServer::from_vmo(
                TEST_DISK_BLOCK_SIZE,
                vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
            )
            .unwrap(),
        );

        if self.gpt {
            // Format the disk with gpt, with a single empty partition named "fvm".
            let client = Arc::new(
                RemoteBlockClient::new(server.connect::<fblock::BlockProxy>()).await.unwrap(),
            );
            assert!(self.extra_gpt_partitions.len() < 10);
            let fvm_num_blocks = self.size / TEST_DISK_BLOCK_SIZE as u64 - 138;
            let mut start_block = 64;
            let mut partitions = vec![gpt::PartitionInfo {
                label: self.system_partition_label.to_string(),
                type_guid: gpt::Guid::from_bytes(FVM_TYPE_GUID),
                instance_guid: gpt::Guid::from_bytes(FVM_PART_INSTANCE_GUID),
                start_block,
                num_blocks: fvm_num_blocks,
                flags: 0,
            }];
            start_block = start_block + fvm_num_blocks;
            for (extra_partition, num_blocks) in &self.extra_gpt_partitions {
                partitions.push(gpt::PartitionInfo {
                    label: extra_partition.to_string(),
                    type_guid: gpt::Guid::from_bytes(DEFAULT_TEST_TYPE_GUID),
                    instance_guid: gpt::Guid::from_bytes(FVM_PART_INSTANCE_GUID),
                    start_block,
                    num_blocks: *num_blocks,
                    flags: 0,
                });
                start_block += num_blocks;
            }
            let _ = gpt::Gpt::format(client, partitions).await.expect("gpt format failed");
        }

        if !self.format_volume_manager {
            return (vmo, self.type_guid);
        }

        let mut gpt = None;
        let connector: Box<dyn BlockConnector> = if self.gpt {
            // Format the volume manager in the gpt partition named "fvm".
            let partitions_dir = vfs::directory::immutable::simple();
            let manager = GptManager::new(server.connect(), partitions_dir.clone()).await.unwrap();
            let dir = vfs::directory::serve(
                partitions_dir,
                vfs::execution_scope::ExecutionScope::new(),
                fio::PERM_READABLE | fio::PERM_WRITABLE,
            );
            gpt = Some((manager, fuchsia_fs::directory::clone(&dir).unwrap()));
            Box::new(DirBasedBlockConnector::new(dir, "part-000/volume".to_string()))
        } else {
            // Format the volume manager onto the disk directly.
            Box::new(VmoBackedServerConnector::new(server))
        };

        if self.volumes_spec.fxfs_blob {
            self.build_fxfs_as_volume_manager(connector).await;
        } else {
            self.build_fvm_as_volume_manager(connector).await;
        }
        if let Some((gpt, partitions_dir)) = gpt {
            partitions_dir.close().await.unwrap().unwrap();
            gpt.shutdown().await;
        }
        (vmo, self.type_guid)
    }

    pub(crate) async fn build_fxfs_as_volume_manager(
        &mut self,
        connector: Box<dyn BlockConnector>,
    ) {
        let crypt_realm = create_hermetic_crypt_service(DATA_KEY, METADATA_KEY).await;
        let mut fxfs = Filesystem::from_boxed_config(connector, Box::new(Fxfs::default()));
        // Wipes the device
        fxfs.format().await.expect("format failed");
        let fs = fxfs.serve_multi_volume().await.expect("serve_multi_volume failed");
        let blob_volume = fs
            .create_volume(
                "blob",
                CreateOptions::default(),
                MountOptions { as_blob: Some(true), ..MountOptions::default() },
            )
            .await
            .expect("failed to create blob volume");
        let blob_creator = connect_to_protocol_at_dir_svc::<fidl_fuchsia_fxfs::BlobCreatorMarker>(
            blob_volume.exposed_dir(),
        )
        .expect("failed to connect to the Blob service");
        self.blob_hash = Some(write_blob(blob_creator, &TEST_BLOB_CONTENTS).await);

        for volume in &self.extra_volumes {
            fs.create_volume(volume, CreateOptions::default(), MountOptions::default())
                .await
                .expect("failed to make extra fxfs volume");
        }

        if self.data_spec.format.is_some() {
            self.init_data_fxfs(FxfsType::FxBlob(fs, crypt_realm), self.data_spec.crypt_policy)
                .await;
        } else {
            fs.shutdown().await.expect("shutdown failed");
        }
    }

    async fn build_fvm_as_volume_manager(&mut self, connector: Box<dyn BlockConnector>) {
        let fvm_slice_size = self.fvm_slice_size;
        let mut fvm_fs = Filesystem::from_boxed_config(
            connector,
            Box::new(Fvm { slice_size: fvm_slice_size, ..Fvm::dynamic_child() }),
        );
        fvm_fs.format().await.unwrap();
        let fvm = fvm_fs.serve_multi_volume().await.unwrap();

        {
            let blob_volume = fvm
                .create_volume(
                    "blobfs",
                    CreateOptions {
                        type_guid: Some(BLOBFS_TYPE_GUID),
                        guid: Some(Uuid::new_v4().into_bytes()),
                        ..Default::default()
                    },
                    MountOptions {
                        uri: Some(String::from("#meta/blobfs.cm")),
                        ..Default::default()
                    },
                )
                .await
                .expect("failed to make fvm blobfs volume");
            let blob_creator =
                connect_to_protocol_at_dir_svc::<fidl_fuchsia_fxfs::BlobCreatorMarker>(
                    blob_volume.exposed_dir(),
                )
                .expect("failed to connect to the Blob service");
            self.blob_hash = Some(write_blob(blob_creator, &TEST_BLOB_CONTENTS).await);

            blob_volume.shutdown().await.unwrap();
        }

        if self.volumes_spec.create_data_partition {
            let data_label = if self.legacy_data_label { "minfs" } else { "data" };

            let _crypt_service;
            let crypt = if self.data_spec.format != Some("fxfs") && self.data_spec.zxcrypt {
                let (crypt, stream) = fidl::endpoints::create_request_stream();
                _crypt_service = fasync::Task::spawn(zxcrypt_crypt::run_crypt_service(
                    crypt_policy::Policy::Null,
                    stream,
                ));
                Some(crypt)
            } else {
                None
            };
            let uri = match (&self.data_spec.format, self.corrupt_data) {
                (None, _) => None,
                (_, true) => None,
                (Some("fxfs"), false) => None,
                (Some("minfs"), false) => Some(String::from("#meta/minfs.cm")),
                (Some("f2fs"), false) => Some(String::from("#meta/f2fs.cm")),
                (Some(format), _) => panic!("unsupported data volume format '{}'", format),
            };

            let data_volume = fvm
                .create_volume(
                    data_label,
                    CreateOptions {
                        initial_size: Some(self.data_volume_size),
                        type_guid: Some(DATA_TYPE_GUID),
                        guid: Some(Uuid::new_v4().into_bytes()),
                        ..Default::default()
                    },
                    MountOptions { crypt, uri, ..Default::default() },
                )
                .await
                .unwrap();

            if self.corrupt_data {
                let volume_proxy = connect_to_protocol_at_dir_svc::<
                    fidl_fuchsia_storage_block::BlockMarker,
                >(data_volume.exposed_dir())
                .unwrap();
                match self.data_spec.format {
                    Some("fxfs") => self.write_magic(volume_proxy, FXFS_MAGIC, 0).await,
                    Some("minfs") => self.write_magic(volume_proxy, MINFS_MAGIC, 0).await,
                    Some("f2fs") => self.write_magic(volume_proxy, F2FS_MAGIC, 1024).await,
                    _ => (),
                }
            } else if self.data_spec.format == Some("fxfs") {
                let dir = fuchsia_fs::directory::clone(data_volume.exposed_dir()).unwrap();
                self.init_data_fxfs(
                    FxfsType::Fxfs(Box::new(DirBasedBlockConnector::new(
                        dir,
                        String::from("svc/fuchsia.storage.block.Block"),
                    ))),
                    self.data_spec.crypt_policy,
                )
                .await
            } else if self.data_spec.format.is_some() {
                self.write_test_data(data_volume.root()).await;
                data_volume.shutdown().await.unwrap();
            }
        }

        for volume in &self.extra_volumes {
            fvm.create_volume(
                volume,
                CreateOptions {
                    type_guid: Some(DATA_TYPE_GUID),
                    guid: Some(Uuid::new_v4().into_bytes()),
                    ..Default::default()
                },
                MountOptions::default(),
            )
            .await
            .expect("failed to make extra fvm volume");
        }

        fvm.shutdown().await.expect("fvm shutdown failed");
    }

    async fn init_data_fxfs(&self, fxfs: FxfsType, crypt_policy: crypt_policy::Policy) {
        let mut fxblob = false;
        let (fs, crypt_realm) = match fxfs {
            FxfsType::Fxfs(connector) => {
                let crypt_realm = create_hermetic_crypt_service(DATA_KEY, METADATA_KEY).await;
                let mut fxfs =
                    Filesystem::from_boxed_config(connector, Box::new(Fxfs::dynamic_child()));
                fxfs.format().await.expect("format failed");
                (fxfs.serve_multi_volume().await.expect("serve_multi_volume failed"), crypt_realm)
            }
            FxfsType::FxBlob(fs, crypt_realm) => {
                fxblob = true;
                (fs, crypt_realm)
            }
        };

        let vol = {
            let vol = fs
                .create_volume("unencrypted", CreateOptions::default(), MountOptions::default())
                .await
                .expect("create_volume failed");
            let keys_dir = fuchsia_fs::directory::create_directory(
                vol.root(),
                "keys",
                fio::PERM_READABLE | fio::PERM_WRITABLE,
            )
            .await
            .unwrap();
            if let crypt_policy::Policy::Keymint = crypt_policy {
                let keymint_file = fuchsia_fs::directory::open_file(
                    &keys_dir,
                    "keymint.0",
                    fio::Flags::FLAG_MAYBE_CREATE
                        | fio::Flags::PROTOCOL_FILE
                        | fio::PERM_READABLE
                        | fio::PERM_WRITABLE,
                )
                .await
                .unwrap();
                let contents = generate_keymint_file_contents(
                    self.keymint_old_blob.as_deref(),
                    self.keymint.clone(),
                )
                .await;
                let mut contents_ref = contents.as_slice();
                if self.corrupt_data && fxblob {
                    contents_ref = &TEST_BLOB_CONTENTS;
                }
                fuchsia_fs::file::write(&keymint_file, contents_ref).await.unwrap();
                fuchsia_fs::file::close(keymint_file).await.unwrap();
            } else {
                let keys_file = fuchsia_fs::directory::open_file(
                    &keys_dir,
                    "fxfs-data",
                    fio::Flags::FLAG_MAYBE_CREATE
                        | fio::Flags::PROTOCOL_FILE
                        | fio::PERM_READABLE
                        | fio::PERM_WRITABLE,
                )
                .await
                .unwrap();
                let mut key_bag = KEY_BAG_CONTENTS.as_bytes();
                if self.corrupt_data && fxblob {
                    key_bag = &TEST_BLOB_CONTENTS;
                }
                fuchsia_fs::file::write(&keys_file, key_bag).await.unwrap();
                fuchsia_fs::file::close(keys_file).await.unwrap();
            }
            fuchsia_fs::directory::close(keys_dir).await.unwrap();

            let crypt = Some(
                crypt_realm
                    .root
                    .connect_to_protocol_at_exposed_dir()
                    .expect("Unable to connect to Crypt service"),
            );
            fs.create_volume(
                "data",
                CreateOptions::default(),
                MountOptions { crypt, ..MountOptions::default() },
            )
            .await
            .expect("create_volume failed")
        };
        self.write_test_data(&vol.root()).await;
        fs.shutdown().await.expect("shutdown failed");
    }

    /// Create a small set of known files to test for presence. The test tree is
    ///  root
    ///   |- .testdata (file, empty)
    ///   |- ssh (directory, non-empty)
    ///   |   |- authorized_keys (file, non-empty)
    ///   |   |- config (directory, empty)
    ///   |- problems (directory, empty (no problems))
    async fn write_test_data(&self, root: &fio::DirectoryProxy) {
        fuchsia_fs::directory::open_file(
            root,
            ".testdata",
            fio::Flags::FLAG_MAYBE_CREATE | fio::PERM_READABLE,
        )
        .await
        .unwrap();

        let ssh_dir = fuchsia_fs::directory::create_directory(
            root,
            "ssh",
            fio::PERM_READABLE | fio::PERM_WRITABLE,
        )
        .await
        .unwrap();
        let authorized_keys = fuchsia_fs::directory::open_file(
            &ssh_dir,
            "authorized_keys",
            fio::Flags::FLAG_MAYBE_CREATE | fio::PERM_READABLE | fio::PERM_WRITABLE,
        )
        .await
        .unwrap();
        fuchsia_fs::file::write(&authorized_keys, "public key!").await.unwrap();
        fuchsia_fs::directory::create_directory(&ssh_dir, "config", fio::PERM_READABLE)
            .await
            .unwrap();

        fuchsia_fs::directory::create_directory(&root, "problems", fio::PERM_READABLE)
            .await
            .unwrap();

        if let Some(content) = &self.fs_switch {
            let fs_switch = fuchsia_fs::directory::open_file(
                &root,
                "fs_switch",
                fio::Flags::FLAG_MAYBE_CREATE | fio::PERM_READABLE | fio::PERM_WRITABLE,
            )
            .await
            .unwrap();
            fuchsia_fs::file::write(&fs_switch, content).await.unwrap();
        }
    }

    async fn write_magic<const N: usize>(
        &self,
        volume_proxy: fblock::BlockProxy,
        value: [u8; N],
        offset: u64,
    ) {
        let client = block_client::RemoteBlockClient::new(volume_proxy)
            .await
            .expect("Failed to create client");
        let block_size = client.block_size() as usize;
        assert!(value.len() <= block_size);
        let mut data = vec![0xffu8; block_size];
        data[..value.len()].copy_from_slice(&value);
        let buffer = block_client::BufferSlice::Memory(&data[..]);
        client.write_at(buffer, offset).await.expect("write failed");
    }

    /// Create a vmo artifact with the format of a compressed zbi boot item containing this
    /// filesystem.
    pub(crate) async fn build_as_zbi_ramdisk(self) -> zx::Vmo {
        // Defined in //sdk/lib/zbi-format/include/lib/zbi-format/internal/storage.h.
        const ZBI_FLAGS_STORAGE_COMPRESSED: zbi::Flags = zbi::Flags::from_bits_retain(1);

        let (ramdisk_vmo, _) = self.build().await;
        let extra = ramdisk_vmo.get_size().unwrap() as u32;
        let mut decompressed_buf = vec![0u8; extra as usize];
        ramdisk_vmo.read(&mut decompressed_buf, 0).unwrap();
        let compressed_buf = zstd::encode_all(decompressed_buf.as_slice(), 0).unwrap();
        let length = compressed_buf.len() as u32;

        let header = zbi::Header {
            r#type: zbi::Type::StorageRamdisk,
            length,
            extra,
            flags: zbi::Flags::VERSION | ZBI_FLAGS_STORAGE_COMPRESSED,
            reserved0: 0,
            reserved1: 0,
            magic: zbi::ITEM_MAGIC,
            crc32: 0,
        };

        let header_size = std::mem::size_of::<zbi::Header>() as u64;
        let zbi_vmo = zx::Vmo::create(header_size + length as u64).unwrap();
        zbi_vmo.write(header.as_bytes(), 0).unwrap();
        zbi_vmo.write(&compressed_buf, header_size).unwrap();

        zbi_vmo
    }
}

/// Helper function to return a set of the volumes found within a given `disk`, which must have a
/// valid GPT containing an fxfs partition in the first entry.
pub async fn list_all_fxfs_volumes(disk: &Disk) -> HashSet<String> {
    let Disk::Prebuilt(vmo, _) = disk else {
        panic!("list_all_fxfs_volumes only supports prebuilt disks");
    };
    let server = Arc::new(
        VmoBackedServer::from_vmo(
            TEST_DISK_BLOCK_SIZE,
            vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
        )
        .unwrap(),
    );

    let partitions_dir = vfs::directory::immutable::simple();
    let manager = GptManager::new(server.connect(), partitions_dir.clone()).await.unwrap();
    let dir = vfs::directory::serve(
        partitions_dir.clone(),
        vfs::execution_scope::ExecutionScope::new(),
        fio::PERM_READABLE | fio::PERM_WRITABLE,
    );

    let partitions = fuchsia_fs::directory::readdir(&dir).await.unwrap().into_iter().map(|entry| {
        let dir = fuchsia_fs::directory::clone(&dir).unwrap();
        let name = entry.name;
        DirBasedBlockConnector::new(dir, format!("{name}/volume"))
    });
    let connector = find_block_device(
        &[
            BlockDeviceMatcher::TypeGuid(&FVM_TYPE_GUID),
            BlockDeviceMatcher::InstanceGuid(&FVM_PART_INSTANCE_GUID),
        ],
        partitions,
    )
    .await
    .expect("failed to match partition")
    .expect("did not match fxfs partition");

    let fxfs = Filesystem::from_boxed_config(Box::new(connector), Box::new(Fxfs::default()));
    let fs = fxfs.serve_multi_volume().await.expect("serve_multi_volume failed");
    let volumes = fs.list_volumes().await.expect("list_volumes failed");
    fs.shutdown().await.expect("shutdown failed");
    manager.shutdown().await;
    volumes.into_iter().collect()
}

/// Returns the set of volume names expected for an fxblob-based system.
pub fn expected_fxblob_volumes() -> HashSet<String> {
    ["blob", "data", "unencrypted"].into_iter().map(str::to_owned).collect()
}
