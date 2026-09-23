// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::fuchsia::fxblob::BlobDirectory;
use crate::fuchsia::fxblob::blob::FxBlob;
use crate::fuchsia::node::OpenedNode;
use crate::fuchsia::testing::{TestFixture, TestFixtureOptions};
use crate::fuchsia::volume::FxVolume;
use anyhow::Error;
use async_trait::async_trait;
use blob_writer::BlobWriter;
use delivery_blob::{CompressionMode, DeliveryBlobType, Type1Blob, Type2Blob};
use fidl_fuchsia_fxfs::{BlobCreatorMarker, BlobReaderMarker, BlobWriterProxy, CreateBlobError};
use fuchsia_component_client::connect_to_protocol_at_dir_svc;
use fuchsia_merkle::Hash;
use fxfs::errors::FxfsError;
use fxfs::object_store::directory::Directory;
use fxfs::object_store::{DataObjectHandle, HandleOptions, ObjectStore};
use std::sync::Arc;
use storage_device::DeviceHolder;
use storage_device::fake_device::FakeDevice;

pub async fn new_blob_fixture() -> TestFixture {
    TestFixture::open(
        DeviceHolder::new(FakeDevice::new(16384, 512)),
        TestFixtureOptions { encrypted: false, as_blob: true, ..Default::default() },
    )
    .await
}

pub async fn open_blob_fixture(device_holder: DeviceHolder) -> TestFixture {
    TestFixture::open(
        device_holder,
        TestFixtureOptions { encrypted: false, as_blob: true, format: false, ..Default::default() },
    )
    .await
}

#[async_trait]
pub trait BlobFixture {
    async fn write_blob(&self, data: &[u8], mode: CompressionMode) -> Hash;
    async fn write_blob_with_type(
        &self,
        data: &[u8],
        delivery_type: DeliveryBlobType,
        mode: CompressionMode,
    ) -> Hash;
    async fn read_blob(&self, hash: Hash) -> Vec<u8>;
    async fn get_blob_handle(&self, name: &str) -> DataObjectHandle<FxVolume>;
    async fn get_blob_vmo(&self, hash: Hash) -> zx::Vmo;
    async fn create_blob(
        &self,
        hash: &[u8; 32],
        allow_existing: bool,
    ) -> Result<BlobWriterProxy, CreateBlobError>;
    async fn get_blob(&self, hash: Hash) -> Result<Arc<FxBlob>, Error>;
    async fn get_opened_blob(&self, hash: Hash) -> Result<OpenedNode<FxBlob>, Error>;
    /// Returns `true` if we could lookup the blob, or `false` if we get [`FxfsError::NotFound`].
    /// Panics otherwise.
    async fn blob_exists(&self, hash: Hash) -> bool;
}

#[async_trait]
impl BlobFixture for TestFixture {
    async fn write_blob(&self, data: &[u8], mode: CompressionMode) -> Hash {
        self.write_blob_with_type(data, DeliveryBlobType::Type1, mode).await
    }

    async fn write_blob_with_type(
        &self,
        data: &[u8],
        delivery_type: DeliveryBlobType,
        mode: CompressionMode,
    ) -> Hash {
        let hash = fuchsia_merkle::root_from_slice(data);
        let delivery_data = match delivery_type {
            DeliveryBlobType::Type1 => Type1Blob::generate(data, mode),
            DeliveryBlobType::Type2 => Type2Blob::generate(data, mode),
            _ => panic!("Unsupported delivery blob type"),
        };
        let writer = self.create_blob(&hash.into(), false).await.expect("failed to create blob");
        let mut blob_writer = BlobWriter::create(writer, delivery_data.len() as u64)
            .await
            .expect("failed to create BlobWriter");
        blob_writer.write(&delivery_data).await.unwrap();
        hash
    }

    async fn read_blob(&self, hash: Hash) -> Vec<u8> {
        let vmo = self.get_blob_vmo(hash).await;
        vmo.read_to_vec(0, vmo.get_stream_size().unwrap()).expect("vmo read failed")
    }

    async fn get_blob_handle(&self, name: &str) -> DataObjectHandle<FxVolume> {
        let root_object_id = self.volume().volume().store().root_directory_object_id();
        let root_dir =
            Directory::open(self.volume().volume(), root_object_id).await.expect("open failed");
        let (object_id, _, _) =
            root_dir.lookup(name).await.expect("lookup failed").expect("file doesn't exist yet");

        ObjectStore::open_object(self.volume().volume(), object_id, HandleOptions::default(), None)
            .await
            .expect("open_object failed")
    }

    async fn get_blob_vmo(&self, hash: Hash) -> zx::Vmo {
        let blob_reader = connect_to_protocol_at_dir_svc::<BlobReaderMarker>(self.volume_out_dir())
            .expect("failed to connect to the BlobReader service");
        blob_reader
            .get_vmo(&hash.into())
            .await
            .expect("transport error on BlobReader.GetVmo")
            .expect("get_vmo failed")
    }

    async fn get_blob(&self, hash: Hash) -> Result<Arc<FxBlob>, Error> {
        // Downgrade from OpenedNode<FxBlob> to Arc<FxBlob>
        self.get_opened_blob(hash).await.map(|blob| (*blob).clone())
    }

    async fn get_opened_blob(&self, hash: Hash) -> Result<OpenedNode<FxBlob>, Error> {
        self.volume()
            .root()
            .clone()
            .into_any()
            .downcast::<BlobDirectory>()
            .unwrap()
            .open_blob(&(hash.into()))
            .await?
            .ok_or_else(|| FxfsError::NotFound.into())
    }

    async fn create_blob(
        &self,
        hash: &[u8; 32],
        allow_existing: bool,
    ) -> Result<BlobWriterProxy, CreateBlobError> {
        let blob_proxy = connect_to_protocol_at_dir_svc::<BlobCreatorMarker>(self.volume_out_dir())
            .expect("failed to connect to the BlobCreator service");
        let blob_writer =
            blob_proxy.create(hash, allow_existing).await.expect("transport error on create")?;
        Ok(blob_writer.into_proxy())
    }

    async fn blob_exists(&self, hash: Hash) -> bool {
        match self.get_blob(hash).await {
            Ok(_) => true,
            Err(e) => match e.downcast::<FxfsError>().expect("expected FxfsError") {
                FxfsError::NotFound => false,
                other => panic!("error during lookup: {:?}", other),
            },
        }
    }
}
