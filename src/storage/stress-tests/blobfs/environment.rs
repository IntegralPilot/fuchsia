// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::blob_actor::BlobActor;
use crate::deletion_actor::DeletionActor;
use crate::instance_actor::InstanceActor;
use crate::read_actor::ReadActor;
use crate::{Args, BLOBFS_MOUNT_PATH};
use async_trait::async_trait;
use fidl_fuchsia_fxfs::{BlobCreatorMarker, BlobCreatorProxy, BlobReaderMarker, BlobReaderProxy};
use fidl_fuchsia_io as fio;
use fs_management::Blobfs;
use futures::lock::Mutex;
use rand::rngs::SmallRng;
use rand::{RngExt as _, SeedableRng as _};
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

const EIGHT_KIB: u64 = 8192;
const ONE_MIB: u64 = 1048576;
const FOUR_MIB: u64 = 4 * ONE_MIB;

/// Describes the environment that this blobfs stress test will run under.
pub struct BlobfsEnvironment {
    seed: u64,
    args: Args,
    vmo: Vmo,
    small_blob_actor: Arc<Mutex<BlobActor>>,
    medium_blob_actor: Arc<Mutex<BlobActor>>,
    large_blob_actor: Arc<Mutex<BlobActor>>,
    deletion_actor: Arc<Mutex<DeletionActor>>,
    instance_actor: Arc<Mutex<InstanceActor>>,
    read_actor: Arc<Mutex<ReadActor>>,
}

pub fn open_blobfs_root() -> Directory {
    Directory::from_namespace(BLOBFS_MOUNT_PATH, fio::PERM_WRITABLE | fio::PERM_READABLE).unwrap()
}

pub fn connect_to_blob_reader(exposed_dir: &fio::DirectoryProxy) -> BlobReaderProxy {
    fuchsia_component::client::connect_to_protocol_at_dir_root::<BlobReaderMarker>(exposed_dir)
        .unwrap()
}

pub fn connect_to_blob_creator(exposed_dir: &fio::DirectoryProxy) -> BlobCreatorProxy {
    fuchsia_component::client::connect_to_protocol_at_dir_root::<BlobCreatorMarker>(exposed_dir)
        .unwrap()
}

impl BlobfsEnvironment {
    pub async fn new(args: Args) -> Self {
        // Create the VMO that the ramdisk is backed by
        let disk_size = args.ramdisk_block_count * args.ramdisk_block_size;
        let vmo = Vmo::create(disk_size).unwrap();

        // Initialize the VMO with FVM partition style and a single blobfs partition

        // Create a ramdisk and setup FVM.
        let mut fvm =
            FvmInstance::new(&vmo, args.ramdisk_block_size, Some(args.fvm_slice_size)).await;

        // Create a blobfs volume
        let volume = fvm.new_volume("blobfs", &TYPE_GUID, None).await;

        let connector = volume.block_connector();
        let mut blobfs = fs_management::filesystem::Filesystem::new(connector, Blobfs::default());
        blobfs.format().await.unwrap();

        let seed = match args.seed {
            Some(seed) => seed,
            None => random_seed(),
        };

        // Mount the blobfs volume
        blobfs.fsck().await.unwrap();
        let mut blobfs = blobfs.serve().await.unwrap();
        blobfs.bind_to_path(BLOBFS_MOUNT_PATH).unwrap();

        let exposed_dir = Clone::clone(blobfs.exposed_dir());

        // Create the instance actor
        let instance_actor = Arc::new(Mutex::new(InstanceActor::new(fvm, blobfs, volume)));

        let mut rng = SmallRng::seed_from_u64(seed);

        // Create the blob actors
        let small_blob_actor = {
            let rng = SmallRng::from_seed(rng.random());
            let uncompressed_size = UncompressedSize::Exact(EIGHT_KIB);
            let compressibility = Compressibility::Uncompressible;
            let factory = FileFactory::new(rng, uncompressed_size, compressibility);
            Arc::new(Mutex::new(BlobActor::new(factory, connect_to_blob_creator(&exposed_dir))))
        };

        let medium_blob_actor = {
            let rng = SmallRng::from_seed(rng.random());
            let uncompressed_size = UncompressedSize::InRange(ONE_MIB, FOUR_MIB);
            let compressibility = Compressibility::Compressible;
            let factory = FileFactory::new(rng, uncompressed_size, compressibility);
            Arc::new(Mutex::new(BlobActor::new(factory, connect_to_blob_creator(&exposed_dir))))
        };
        let large_blob_actor = {
            let rng = SmallRng::from_seed(rng.random());
            let uncompressed_size = UncompressedSize::Exact(2 * disk_size);
            let compressibility = Compressibility::Compressible;
            let factory = FileFactory::new(rng, uncompressed_size, compressibility);
            Arc::new(Mutex::new(BlobActor::new(factory, connect_to_blob_creator(&exposed_dir))))
        };

        // Create the read actor
        let read_actor = {
            let rng = SmallRng::from_seed(rng.random());
            let root_dir = open_blobfs_root();
            Arc::new(Mutex::new(ReadActor::new(
                rng,
                root_dir,
                connect_to_blob_reader(&exposed_dir),
            )))
        };

        // Create the deletion actor
        let deletion_actor = {
            let rng = SmallRng::from_seed(rng.random());
            let root_dir = open_blobfs_root();
            Arc::new(Mutex::new(DeletionActor::new(rng, root_dir)))
        };

        Self {
            seed,
            args,
            vmo,
            instance_actor,
            small_blob_actor,
            medium_blob_actor,
            large_blob_actor,
            read_actor,
            deletion_actor,
        }
    }
}

impl std::fmt::Debug for BlobfsEnvironment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Environment").field("seed", &self.seed).field("args", &self.args).finish()
    }
}

#[async_trait]
impl Environment for BlobfsEnvironment {
    fn target_operations(&self) -> Option<u64> {
        self.args.num_operations
    }

    fn timeout_seconds(&self) -> Option<u64> {
        self.args.time_limit_secs
    }

    async fn actor_runners(&mut self) -> Vec<ActorRunner> {
        let mut runners = vec![
            ActorRunner::new("small_blob_actor", None, self.small_blob_actor.clone()),
            ActorRunner::new("medium_blob_actor", None, self.medium_blob_actor.clone()),
            ActorRunner::new("large_blob_actor", None, self.large_blob_actor.clone()),
            ActorRunner::new("read_actor", None, self.read_actor.clone()),
            ActorRunner::new(
                "deletion_actor",
                Some(Duration::from_secs(10)),
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
        let exposed_dir = {
            let mut actor = self.instance_actor.lock().await;

            // The environment is only reset when the instance is killed.
            // TODO(72385): Pass the actor error here, so it can be printed out on assert failure.
            assert!(actor.instance.is_none());

            // Create a ramdisk and setup FVM.
            let fvm = FvmInstance::new(&self.vmo, self.args.ramdisk_block_size, None).await;

            // Open the volume
            let volume = fvm.open_volume("blobfs").await;

            let connector = volume.block_connector();
            let mut blobfs =
                fs_management::filesystem::Filesystem::new(connector, Blobfs::default());

            // Mount the blobfs volume
            blobfs.fsck().await.unwrap();
            let mut blobfs = blobfs.serve().await.unwrap();
            blobfs.bind_to_path(BLOBFS_MOUNT_PATH).unwrap();

            let exposed_dir = Clone::clone(blobfs.exposed_dir());
            // Replace the fvm, blobfs and volume instances
            actor.instance = Some((blobfs, fvm, volume));
            exposed_dir
        };

        // Replace the root directory with a new one
        {
            let mut actor = self.small_blob_actor.lock().await;
            actor.creator = connect_to_blob_creator(&exposed_dir);
        }

        {
            let mut actor = self.medium_blob_actor.lock().await;
            actor.creator = connect_to_blob_creator(&exposed_dir);
        }

        {
            let mut actor = self.large_blob_actor.lock().await;
            actor.creator = connect_to_blob_creator(&exposed_dir);
        }

        {
            let mut actor = self.read_actor.lock().await;
            actor.root_dir = open_blobfs_root();
            actor.reader = connect_to_blob_reader(&exposed_dir);
        }

        {
            let mut actor = self.deletion_actor.lock().await;
            actor.root_dir = open_blobfs_root();
        }
    }
}
