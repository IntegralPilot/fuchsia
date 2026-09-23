// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use blob_writer::{BlobWriter, CreateError, WriteError};
use delivery_blob::{CompressionMode, Type1Blob};
use fidl_fuchsia_fxfs::{BlobCreatorProxy, CreateBlobError};
use log::info;
use storage_stress_test_utils::data::FileFactory;
use stress_test::actor::{Actor, ActorError};
use zx::Status;

// Performs operations on blobs expected to exist on disk
pub struct BlobActor {
    // Factory used to generate blobs of specific size and compressibility
    pub factory: FileFactory,

    pub creator: BlobCreatorProxy,
}

impl BlobActor {
    pub fn new(factory: FileFactory, creator: BlobCreatorProxy) -> Self {
        Self { factory, creator }
    }

    async fn create_blob(&mut self) -> Result<(), Status> {
        // Create the root hash for the blob
        let data_bytes = self.factory.generate_bytes();
        let delivery_blob = Type1Blob::generate(&data_bytes, CompressionMode::Attempt);
        let merkle_root_hash = fuchsia_merkle::root_from_slice(&data_bytes);

        // Write the file to disk.
        let writer_client_end = match self.creator.create(&merkle_root_hash.into(), false).await {
            Ok(Ok(writer)) => writer,
            Ok(Err(CreateBlobError::AlreadyExists)) => return Err(Status::ALREADY_EXISTS),
            Ok(Err(CreateBlobError::Internal)) => return Err(Status::INTERNAL),
            Err(e) if e.is_closed() => return Err(Status::PEER_CLOSED),
            Err(e) => panic!("Unexpected FIDL error: {:?}", e),
        };
        let mut writer =
            match BlobWriter::create(writer_client_end.into_proxy(), delivery_blob.len() as u64)
                .await
            {
                Ok(writer) => writer,
                Err(CreateError::Fidl(e)) if e.is_closed() => return Err(Status::PEER_CLOSED),
                Err(CreateError::Fidl(e)) => panic!("Unexpected FIDL error: {:?}", e),
                Err(CreateError::GetVmo(e)) => return Err(e),
                Err(CreateError::GetSize(e)) => panic!("zx_vmo_get_size should not fail: {:?}", e),
            };
        match writer.write(&delivery_blob).await {
            Ok(()) => Ok(()),
            Err(WriteError::Fidl(e)) if e.is_closed() => Err(Status::PEER_CLOSED),
            Err(WriteError::Fidl(e)) => panic!("Unexpected FIDL error: {:?}", e),
            Err(WriteError::QueueEnded) => Err(Status::PEER_CLOSED),
            Err(WriteError::BytesReady(e)) => Err(e),
            Err(WriteError::VmoWrite(e)) => panic!("zx_vmo_writer should not fail: {:?}", e),
            Err(WriteError::EndOfBlob) => panic!("Tried to write past the end of the blob"),
        }
    }
}

#[async_trait]
impl Actor for BlobActor {
    async fn perform(&mut self) -> Result<(), ActorError> {
        match self.create_blob().await {
            Ok(()) => Ok(()),
            Err(Status::ALREADY_EXISTS) => Ok(()),
            Err(Status::NO_SPACE) => Ok(()),
            // Any other error is assumed to come from an intentional crash.
            // The environment verifies that an intentional crash occurred
            // and will panic if that is not the case.
            Err(s) => {
                info!("Blob actor got status: {}", s);
                Err(ActorError::ResetEnvironment)
            }
        }
    }
}
