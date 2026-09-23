// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use fidl_fuchsia_fxfs::BlobReaderProxy;
use fuchsia_merkle::Hash;
use log::{debug, info};
use rand::RngExt as _;
use rand::rngs::SmallRng;
use rand::seq::IndexedRandom as _;
use storage_stress_test_utils::io::Directory;
use stress_test::actor::{Actor, ActorError};
use zx::Status;

// Performs operations on blobs expected to exist on disk
pub struct ReadActor {
    // Random number generator used by the operator
    pub rng: SmallRng,

    // Blobfs root directory
    pub root_dir: Directory,

    pub reader: BlobReaderProxy,
}

impl ReadActor {
    pub fn new(rng: SmallRng, root_dir: Directory, reader: BlobReaderProxy) -> Self {
        Self { rng, root_dir, reader }
    }

    // Read a random amount of data at a random offset from a random blob
    async fn read_blob(&mut self) -> Result<(), Status> {
        // Decide how many blobs to create new handles for
        let blob_list = self.root_dir.entries().await?;

        if blob_list.is_empty() {
            // No blobs to read!
            return Ok(());
        }

        // Choose a random blob and get a handle to it.
        let blob = blob_list.choose(&mut self.rng).unwrap();
        let merkle: Hash = blob.parse().map_err(|_| Status::IO)?;
        let vmo = match self.reader.get_vmo(&merkle.into()).await {
            Ok(Ok(vmo)) => vmo,
            Ok(Err(e)) => return Err(Status::err_from_raw(e)),
            // Blobfs was shut down.
            Err(e) if e.is_closed() => return Err(Status::PEER_CLOSED),
            Err(e) => panic!("Unexpected FIDL error: {:?}", e),
        };

        debug!("Reading from {}", blob);
        let data_size_bytes = vmo.get_stream_size().expect("Failed to get VMO stream size");

        if data_size_bytes == 0 {
            // Nothing to read, blob is empty!
            return Ok(());
        }

        // Choose an offset
        let offset = self.rng.random_range(0..data_size_bytes - 1);

        // Determine the length of this read
        let end_pos = self.rng.random_range(offset..data_size_bytes);

        assert!(end_pos >= offset);
        let length = end_pos - offset;

        // Read the data from the handle and verify it.
        vmo.read_to_vec::<u8>(offset, length)?;
        debug!("Read {} bytes from {}", length, blob);
        Ok(())
    }
}

#[async_trait]
impl Actor for ReadActor {
    async fn perform(&mut self) -> Result<(), ActorError> {
        match self.read_blob().await {
            Ok(()) => Ok(()),
            Err(Status::NOT_FOUND) => Ok(()),
            // Any other error is assumed to come from an intentional crash.
            // The environment verifies that an intentional crash occurred
            // and will panic if that is not the case.
            Err(s) => {
                info!("Read actor got status: {}", s);
                Err(ActorError::ResetEnvironment)
            }
        }
    }
}
