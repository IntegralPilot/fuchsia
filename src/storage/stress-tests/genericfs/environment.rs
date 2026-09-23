// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::Args;
use crate::deletion_actor::DeletionActor;
use crate::file_actor::FileActor;
use crate::instance_actor::InstanceActor;
use anyhow::{anyhow, format_err};
use async_trait::async_trait;
use diagnostics_reader::ArchiveReader;
use either::Either;
use fidl_fuchsia_fs_startup::{CheckOptions, CreateOptions, MountOptions};
use fidl_fuchsia_fxfs::{CryptManagementMarker, CryptManagementProxy, CryptMarker, KeyPurpose};
use fidl_fuchsia_io as fio;
use fidl_fuchsia_logger as flogger;
use fs_management::FSConfig;
use fs_management::filesystem::Filesystem;
use fuchsia_async as fasync;
use fuchsia_component_test::{Capability, ChildOptions, RealmBuilder, RealmInstance, Ref, Route};
use fuchsia_inspect::hierarchy::DiagnosticsHierarchy;
use fuchsia_sync::Mutex;
use futures::StreamExt as _;
use futures::lock::Mutex as FuturesMutex;
use key_bag::Aes256Key;
use rand::rngs::SmallRng;
use rand::{RngExt as _, SeedableRng as _};
use std::ops::Deref;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use storage_stress_test_utils::data::{Compressibility, FileFactory, UncompressedSize};
use storage_stress_test_utils::fvm::{FvmInstance, Guid};
use storage_stress_test_utils::io::Directory;
use stress_test::actor::ActorRunner;
use stress_test::environment::Environment;
use stress_test::random_seed;
use zx::Vmo;

// All partitions in this test have their type set to this arbitrary GUID.
const TYPE_GUID: Guid =
    [0x0, 0x1, 0x2, 0x3, 0x4, 0x5, 0x6, 0x7, 0x8, 0x9, 0xa, 0xb, 0xc, 0xd, 0xe, 0xf];

const ONE_MIB: u64 = 1048576;
const FOUR_MIB: u64 = 4 * ONE_MIB;

const MOUNT_PATH: &str = "/fs";

const DATA_KEY: Aes256Key = Aes256Key::create([
    0xcf, 0x9e, 0x45, 0x2a, 0x22, 0xa5, 0x70, 0x31, 0x33, 0x3b, 0x4d, 0x6b, 0x6f, 0x78, 0x58, 0x29,
    0x04, 0x79, 0xc7, 0xd6, 0xa9, 0x4b, 0xce, 0x82, 0x04, 0x56, 0x5e, 0x82, 0xfc, 0xe7, 0x37, 0xa8,
]);

const METADATA_KEY: Aes256Key = Aes256Key::create([
    0x0f, 0x4d, 0xca, 0x6b, 0x35, 0x0e, 0x85, 0x6a, 0xb3, 0x8c, 0xdd, 0xe9, 0xda, 0x0e, 0xc8, 0x22,
    0x8e, 0xea, 0xd8, 0x05, 0xc4, 0xc9, 0x0b, 0xa8, 0xd8, 0x85, 0x87, 0x50, 0x75, 0x40, 0x1c, 0x4c,
]);

const INSPECT_POLL_INTERVAL: zx::MonotonicDuration = zx::MonotonicDuration::from_seconds(1);

fn print_inspect_data(data: &DiagnosticsHierarchy) {
    match serde_json::to_string_pretty(&data) {
        Ok(data) => {
            println!("=== START Inspect Data ===");
            println!("{}", data);
            println!("=== END Inspect Data ===");
        }
        Err(e) => {
            eprintln!("Failed to deserialize inspect data: {:?}", e);
        }
    }
}

pub async fn create_hermetic_crypt_service(
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

pub fn open_dir_at_root(subdir: &str) -> Directory {
    let path = PathBuf::from(MOUNT_PATH).join(subdir);
    Directory::from_namespace(path, fio::PERM_WRITABLE | fio::PERM_READABLE).unwrap()
}

/// Describes the environment that this stress test will run under.
pub struct FsEnvironment<FSC: FSConfig> {
    seed: u64,
    args: Args,
    vmo: Vmo,
    config: FSC,
    crypt_realm: Option<RealmInstance>,
    instance_actor: Arc<FuturesMutex<InstanceActor>>,
    file_actor: Arc<FuturesMutex<FileActor>>,
    deletion_actor: Arc<FuturesMutex<DeletionActor>>,
    _inspect_poll_task: fasync::Task<()>,
    inspect: Arc<Mutex<Option<DiagnosticsHierarchy>>>,
}

impl<FSC: Clone + FSConfig> FsEnvironment<FSC> {
    pub async fn new(config: FSC, args: Args) -> Self {
        let crypt_realm = if config.is_multi_volume() {
            Some(create_hermetic_crypt_service(DATA_KEY, METADATA_KEY).await)
        } else {
            None
        };
        // Create the VMO that the ramdisk is backed by
        let vmo_size = args.ramdisk_block_count * args.ramdisk_block_size;
        let vmo = Vmo::create(vmo_size).unwrap();

        // Initialize the VMO with FVM partition style and a single filesystem partition

        // Create a ramdisk and setup FVM.
        let mut fvm =
            FvmInstance::new(&vmo, args.ramdisk_block_size, Some(args.fvm_slice_size)).await;

        // Initialize the filesystem on a new volume
        let volume = fvm.new_volume("default", &TYPE_GUID, Some(fvm.free_space().await)).await;
        let mut fs = Filesystem::new(volume.block_connector(), config.clone());
        fs.format().await.unwrap();
        let moniker = fs.get_component_moniker().await.unwrap();

        let instance = if fs.config().is_multi_volume() {
            let crypt = Some(
                crypt_realm.as_ref().unwrap().root.connect_to_protocol_at_exposed_dir().unwrap(),
            );
            let instance = fs.serve_multi_volume().await.unwrap();
            let mut vol = instance
                .create_volume(
                    "default",
                    CreateOptions::default(),
                    MountOptions { crypt, ..MountOptions::default() },
                )
                .await
                .unwrap();
            vol.bind_to_path(MOUNT_PATH).unwrap();
            Either::Right((instance, vol))
        } else {
            let mut instance = fs.serve().await.unwrap();
            instance.bind_to_path(MOUNT_PATH).unwrap();
            Either::Left(instance)
        };

        let seed = match args.seed {
            Some(seed) => seed,
            None => random_seed(),
        };

        let mut rng = SmallRng::seed_from_u64(seed);

        // Make a home directory for file actor and deletion actor
        let root_dir =
            Directory::from_namespace(MOUNT_PATH, fio::PERM_WRITABLE | fio::PERM_READABLE).unwrap();
        root_dir.create_directory("home1", fio::PERM_WRITABLE | fio::PERM_READABLE).await.unwrap();
        // Home directory must be recovered because SPO occurs in a crash test.
        // Syncronize the home directory to ensure consistency.
        open_dir_at_root("home1").sync_directory().await.unwrap();

        let file_actor = {
            let rng = SmallRng::from_seed(rng.random());
            let uncompressed_size = UncompressedSize::InRange(ONE_MIB, FOUR_MIB);
            let compressibility = Compressibility::Compressible;
            let factory = FileFactory::new(rng, uncompressed_size, compressibility);
            let home_dir = open_dir_at_root("home1");
            let file_actor = FileActor::new(factory, home_dir);
            file_actor.set_progress_timer(std::time::Duration::from_secs(60));
            Arc::new(FuturesMutex::new(file_actor))
        };
        let deletion_actor = {
            let rng = SmallRng::from_seed(rng.random());
            let home_dir = open_dir_at_root("home1");
            Arc::new(FuturesMutex::new(DeletionActor::new(rng, home_dir)))
        };

        let instance_actor = Arc::new(FuturesMutex::new(InstanceActor::new(fvm, volume, instance)));

        let inspect = Arc::new(Mutex::new(None));
        let inspect_cloned = inspect.clone();
        Self {
            seed,
            args,
            vmo,
            crypt_realm,
            file_actor,
            deletion_actor,
            instance_actor,
            config,
            _inspect_poll_task: fasync::Task::spawn(async move {
                Self::inspect_poll_task(moniker, inspect_cloned).await;
            }),
            inspect,
        }
    }

    async fn inspect_poll_task(moniker: String, inspect: Arc<Mutex<Option<DiagnosticsHierarchy>>>) {
        let mut timer = fuchsia_async::Interval::new(INSPECT_POLL_INTERVAL);
        loop {
            timer.next().await;
            match ArchiveReader::inspect()
                .select_all_for_component(moniker.to_string())
                .snapshot()
                .await
                .map_err(|e| anyhow!(e))
                .and_then(|d| {
                    d.into_iter()
                        .next()
                        .and_then(|res| res.payload)
                        .ok_or_else(|| format_err!("expected one inspect hierarchy"))
                }) {
                Ok(data) => {
                    let mut inspect = inspect.lock();
                    if inspect.replace(data).is_none() {
                        // Whenever we first receive data for a new instance, dump it out.
                        print_inspect_data(inspect.as_ref().unwrap());
                    }
                }
                Err(e) => {
                    eprintln!("Failed to read inspect: {:?}", e);
                    continue;
                }
            };
        }
    }
}

impl<FSC: FSConfig> std::fmt::Debug for FsEnvironment<FSC> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Environment").field("seed", &self.seed).field("args", &self.args).finish()
    }
}

#[async_trait]
impl<FSC: 'static + FSConfig + Clone + Send + Sync> Environment for FsEnvironment<FSC> {
    fn target_operations(&self) -> Option<u64> {
        self.args.num_operations
    }

    fn timeout_seconds(&self) -> Option<u64> {
        self.args.time_limit_secs
    }

    async fn actor_runners(&mut self) -> Vec<ActorRunner> {
        let mut runners = vec![
            ActorRunner::new("file_actor", None, self.file_actor.clone()),
            ActorRunner::new(
                "deletion_actor",
                Some(Duration::from_secs(5)),
                self.deletion_actor.clone(),
            ),
        ];

        if let Some(secs) = self.args.disconnect_secs {
            if secs > 0 {
                let runner = ActorRunner::new(
                    "instance_actor",
                    Some(Duration::from_secs(secs)),
                    self.instance_actor.clone(),
                );
                runners.push(runner);
            }
        }

        runners
    }

    async fn reset(&mut self) {
        {
            let mut actor = self.instance_actor.lock().await;

            // The environment is only reset when the instance is killed.
            // TODO(72385): Pass the actor error here, so it can be printed out on assert failure.
            assert!(actor.instance.is_none());

            // Create a ramdisk and setup FVM.
            let fvm = FvmInstance::new(&self.vmo, self.args.ramdisk_block_size, None).await;
            let volume = fvm.open_volume("default").await;

            let mut fs = Filesystem::new(volume.block_connector(), self.config.clone());
            fs.fsck().await.unwrap();
            let instance = if fs.config().is_multi_volume() {
                let instance = fs.serve_multi_volume().await.unwrap();
                let crypt = Some(
                    self.crypt_realm
                        .as_ref()
                        .unwrap()
                        .root
                        .connect_to_protocol_at_exposed_dir()
                        .unwrap(),
                );
                instance
                    .check_volume("default", CheckOptions { crypt, ..Default::default() })
                    .await
                    .unwrap();
                let crypt = Some(
                    self.crypt_realm
                        .as_ref()
                        .unwrap()
                        .root
                        .connect_to_protocol_at_exposed_dir()
                        .unwrap(),
                );
                let mut vol = instance
                    .open_volume("default", MountOptions { crypt, ..MountOptions::default() })
                    .await
                    .unwrap();
                vol.bind_to_path(MOUNT_PATH).unwrap();
                Either::Right((instance, vol))
            } else {
                let mut instance = fs.serve().await.unwrap();
                instance.bind_to_path(MOUNT_PATH).unwrap();
                Either::Left(instance)
            };

            *self.inspect.lock() = None;

            // Replace the fvm and fs instances
            actor.instance = Some((fvm, volume, instance));
        }

        // Replace the root directory with a new one
        {
            let mut actor = self.file_actor.lock().await;
            actor.home_dir = open_dir_at_root("home1");
        }

        {
            let mut actor = self.deletion_actor.lock().await;
            actor.home_dir = open_dir_at_root("home1");
        }
    }

    fn panic_hook(&self) -> Option<Box<dyn Fn() + 'static + Sync + Send>> {
        let inspect = self.inspect.clone();
        Some(Box::new(move || {
            eprintln!("Printing inspect data for test due to panic.");
            let mut inspect = match inspect.try_lock() {
                None => {
                    eprintln!("No inspect data was collected; can't print additional debug info");
                    return;
                }
                Some(inspect) => inspect,
            };
            let inspect = inspect.as_mut().unwrap();
            inspect.sort();
            print_inspect_data(&*inspect);
        }))
    }
}
