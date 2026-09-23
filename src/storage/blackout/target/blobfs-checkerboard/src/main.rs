// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use async_trait::async_trait;
use blackout_target::{Test, TestServer};
use byteorder::{NativeEndian, ReadBytesExt, WriteBytesExt};
use fidl_fuchsia_io as fio;
use fs_management::Blobfs;
use fs_management::filesystem::Filesystem;
use fuchsia_fs::directory::readdir;
use rand::distr::Standard;
use rand::rngs::StdRng;
use rand::{Rng, RngExt as _, SeedableRng as _};
use std::collections::HashMap;
use std::sync::Arc;

async fn write_blob(rng: &mut impl Rng, root: &fio::DirectoryProxy, i: u64) -> Result<String> {
    let mut data = vec![];
    data.write_u64::<NativeEndian>(i)?;
    // length of extra random data in bytes
    let rand_length: usize = rng.gen_range(0..6000);
    data.extend(rng.sample_iter::<u8, _>(&Standard).take(rand_length));

    // generate merkle root for new blob
    let path = fuchsia_merkle::root_from_slice(&data).to_string();

    // blob writing dance
    let blob = fuchsia_fs::directory::open_file(
        root,
        &path,
        fio::OpenFlags::CREATE | fio::OpenFlags::RIGHT_WRITABLE,
    )
    .await?;
    blob.resize(data.len() as u64).await?.map_err(zx::Status::err_from_raw)?;
    fuchsia_fs::file::write(&blob, &data).await?;

    Ok(path)
}

async fn setup_blobfs(device_path: Option<String>, device_label: String) -> Result<Filesystem> {
    let device_dir = match device_path {
        Some(path) => {
            let device_dir =
                fuchsia_fs::directory::open_in_namespace(&path, fio::OpenFlags::empty())?;
            Some(device_dir)
        }
        None => None,
    };
    let block_device = blackout_target::find_partition(&device_label, device_dir.as_ref()).await?;
    Ok(Blobfs::new(block_device))
}

#[derive(Copy, Clone)]
struct BlobfsCheckerboard;

#[async_trait]
impl Test for BlobfsCheckerboard {
    async fn setup(
        self: Arc<Self>,
        device_label: String,
        device_path: Option<String>,
        seed: u64,
    ) -> Result<()> {
        log::info!(
            "setting up device with label {} and potential path {:?}",
            device_label,
            device_path
        );
        let device_dir = match device_path {
            Some(path) => {
                let device_dir =
                    fuchsia_fs::directory::open_in_namespace(&path, fio::OpenFlags::empty())?;
                Some(device_dir)
            }
            None => None,
        };
        let blobfs_controller = blackout_target::set_up_partition(device_label.clone()).await?;

        let mut blobfs = Blobfs::new(blobfs_controller);
        let mut rng = StdRng::seed_from_u64(seed);

        log::info!("formatting provided block device with blobfs");
        blobfs.format().await.context("failed to format blobfs")?;

        log::info!("starting blobfs");
        let blobfs = blobfs.serve().await.context("failed to mount blobfs")?;

        // Normally these tests just format in the setup, but I want a pile of files that I'm never
        // going to touch again, so this is the best place to set them up. Each file has a number
        // followed by a random amount of random garbage up to 6k (so the data for each blob takes
        // up one block at most). We want to stay within the bounds of the provided partition, so
        // query the size of the filesystem, and fill about 3/4ths of it with blobs.
        let q = blobfs.query().await?;
        log::info!("got query results - {:#?}", q);
        let num_blobs = (((q.total_bytes - q.used_bytes) / q.block_size as u64) * 3) / 4;
        let num_blobs = num_blobs - (num_blobs % 2);
        log::info!("just kidding - creating {} blobs on disk for setup", num_blobs);

        for i in 0..num_blobs {
            let _ = write_blob(&mut rng, &blobfs.root(), i).await?;
        }

        log::info!("unmounting blobfs");
        blobfs.shutdown().await.context("failed to unmount blobfs")?;

        Ok(())
    }

    async fn test(
        self: Arc<Self>,
        device_label: String,
        device_path: Option<String>,
        seed: u64,
    ) -> Result<()> {
        log::info!(
            "finding block device based on name {} and potential path {:?}",
            device_label,
            device_path
        );
        let mut blobfs = setup_blobfs(device_path, device_label).await?;
        let mut rng = StdRng::seed_from_u64(seed);

        log::info!("running blobfs");
        let blobfs = blobfs.serve().await.context("failed to mount blobfs")?;

        log::info!("some prep work...");
        // Get a list of all the blobs on the partition so we can generate our load gen state. We
        // have exclusive access to this block device, so they were either made by us in setup or
        // made by us in a previous iteration of the test. This test is designed to be run multiple
        // times in a row and could be in any state when we cut the power, so we have to
        // reconstruct it based off the test invariants. Those invariants are
        //   1. even number of blob "slots"
        //   2. odd number blobs are never modified
        //   3. even number blobs can be deleted and rewritted with new data
        //   4. that means they might not be there when we start (hence "slots")
        //   5. blobs start with their number, which is a u64 written in native endian with
        //      byteorder

        #[derive(Clone, Debug)]
        enum Slot {
            Empty,
            Blob { path: String },
        }
        let mut blobs: HashMap<u64, Slot> = HashMap::new();

        // first we figure out what blobs are there.
        for entry in readdir(blobfs.root()).await? {
            let blob = fuchsia_fs::directory::open_file(
                blobfs.root(),
                &entry.name,
                fio::OpenFlags::RIGHT_READABLE,
            )
            .await?;
            let content = fuchsia_fs::file::read_num_bytes(&blob, 8).await?;
            let slot_num = content.as_slice().read_u64::<NativeEndian>()?;
            debug_assert!(!blobs.contains_key(&slot_num));
            blobs.insert(slot_num, Slot::Blob { path: entry.name });
        }
        log::info!("found {} blobs", blobs.len());

        // What is the max slot number we found? If it's even, it's the number of slots, if it's
        // odd, then it's the number of slots - 1. There should always be at least one slot filled
        // out (odds are never touched, so really we are going to have at least 1/2 of all possible
        // slots filled already).
        let max_found =
            blobs.keys().max().expect("Didn't find a maximum slot number. No blobs on disk?");
        // Either the last even slot was filled or we found the largest odd slot so this gets the
        // maximum number of slots.
        let max_found = max_found + 1;
        let max_slots = max_found + (max_found % 2);
        debug_assert!(max_slots % 2 == 0);
        let half_slots = max_slots / 2;
        log::info!(
            "max_found = {}. assuming max_slots = {} (half_slots = {})",
            max_found,
            max_slots,
            half_slots
        );

        let mut slots = vec![Slot::Empty; max_slots as usize];
        for (k, v) in blobs.into_iter() {
            slots[k as usize] = v;
        }

        log::info!("generating load");
        loop {
            // Get a random, even numbered slot and do the "next thing" to it.
            //   1. if the slot is empty, create a blob and write random data to it
            //   2. if it's not empty
            //      - 50% chance we just open it and read the contents and close it again
            //      - 50% chance we delete it
            // Obviously this isn't a "realistic" workload - blobs in the wild are going to spend a
            // lot of time getting read before they are deleted - but we want things to change a
            // lot.
            let slot_num = rng.gen_range(0..half_slots as usize) * 2;
            let maybe_new_slot = match &slots[slot_num] {
                Slot::Empty => {
                    let path = write_blob(&mut rng, blobfs.root(), slot_num as u64).await?;
                    Some(Slot::Blob { path })
                }
                Slot::Blob { path } => {
                    if rng.gen_bool(1.0 / 2.0) {
                        let blob = fuchsia_fs::directory::open_file(
                            blobfs.root(),
                            path,
                            fio::OpenFlags::RIGHT_READABLE,
                        )
                        .await?;
                        let _ = fuchsia_fs::file::read(&blob).await?;
                        None
                    } else {
                        blobfs
                            .root()
                            .unlink(path, &fio::UnlinkOptions::default())
                            .await?
                            .map_err(zx::Status::err_from_raw)?;
                        Some(Slot::Empty)
                    }
                }
            };

            if let Some(new_slot) = maybe_new_slot {
                slots[slot_num] = new_slot;
            }
        }
    }

    async fn verify(
        self: Arc<Self>,
        device_label: String,
        device_path: Option<String>,
        _seed: u64,
    ) -> Result<()> {
        log::info!(
            "finding block device based on name {} and potential path {:?}",
            device_label,
            device_path
        );
        let mut blobfs = setup_blobfs(device_path, device_label).await?;
        log::info!("verifying disk with fsck");
        blobfs.fsck().await.context("fsck failed")?;

        log::info!("verification successful");
        Ok(())
    }
}

#[fuchsia::main]
async fn main() -> Result<()> {
    let server = TestServer::new(BlobfsCheckerboard)?;
    server.serve().await;

    Ok(())
}
