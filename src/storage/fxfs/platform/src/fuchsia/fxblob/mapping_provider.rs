// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::fuchsia::errors::map_to_status;
use crate::fuchsia::fxblob::directory::BlobDirectory;
use crate::fuchsia::pager::PagerBacked;
use anyhow::Error;
use fidl_fuchsia_storage_mapping as fmapping;
use fuchsia_merkle::Hash;
use futures::TryStreamExt;
use fxfs::errors::FxfsError;
use log::{error, warn};
use mapping::{
    Extents, MAPPING_VMO_SIZE, MappingCommand, PENDING_COMMANDS_CAPACITY, RawMappingCommand,
};
use std::sync::Arc;
use vmo_fifo::AsyncSender;

/// BlobMappingProvider services requests to open mapping sessions over a given BlobDirectory.
pub struct BlobMappingProvider {
    blob_directory: Arc<BlobDirectory>,
}

impl BlobMappingProvider {
    pub fn new(blob_directory: Arc<BlobDirectory>) -> Result<Self, Error> {
        Ok(Self { blob_directory })
    }

    pub async fn handle_mapping_provider_requests(
        self: Arc<Self>,
        mut stream: fmapping::MappingProviderRequestStream,
    ) {
        while let Ok(Some(request)) = stream.try_next().await {
            match request {
                fmapping::MappingProviderRequest::OpenSession { session, responder } => {
                    let vmo = match zx::Vmo::create(MAPPING_VMO_SIZE) {
                        Ok(v) => v,
                        Err(e) => {
                            let _ = responder.send(Err(e.into_raw()));
                            continue;
                        }
                    };
                    let sender = match AsyncSender::<RawMappingCommand>::new(
                        vmo,
                        8,
                        PENDING_COMMANDS_CAPACITY,
                    ) {
                        Ok(s) => s,
                        Err(e) => {
                            let _ = responder.send(Err(e.into_raw()));
                            continue;
                        }
                    };
                    let vmo_clone = match sender.vmo().duplicate_handle(zx::Rights::SAME_RIGHTS) {
                        Ok(v) => v,
                        Err(e) => {
                            let _ = responder.send(Err(e.into_raw()));
                            continue;
                        }
                    };
                    if let Err(error) = responder.send(Ok(vmo_clone)) {
                        error!(error:?; "Failed to send open session response");
                    } else {
                        let mapping_session =
                            BlobMappingSession::new(self.blob_directory.clone(), sender);
                        self.blob_directory.volume().scope().spawn(async move {
                            mapping_session
                                .handle_mapping_session_requests(session.into_stream())
                                .await;
                        });
                    }
                }
                fmapping::MappingProviderRequest::_UnknownMethod { ordinal, .. } => {
                    warn!(ordinal; "Unknown MappingProvider method");
                }
            }
        }
    }
}

/// `BlobMappingSession` services requests to open or close blobs within a `BlobDirectory`.
/// Upon opening a blob, the blob's underlying extents (data and merkle blocks) are retrieved and
/// then forwarded to client asynchronously via `sender`. The client provides a session-unique
/// `key` for each opened blob, which can also be referenced during a `Close` request.
pub struct BlobMappingSession {
    blob_directory: Arc<BlobDirectory>,
    sender: AsyncSender<RawMappingCommand>,
}

impl BlobMappingSession {
    pub fn new(blob_directory: Arc<BlobDirectory>, sender: AsyncSender<RawMappingCommand>) -> Self {
        Self { blob_directory, sender }
    }

    /// Retrieves the extent mappings for the blob and registers the blob in the mapping session.
    /// Returns the uncompressed byte size of the blob.
    async fn open_blob(&mut self, key: u64, hash: Hash) -> Result<u64, Error> {
        let node = self.blob_directory.open_blob(&hash.into()).await?.ok_or(FxfsError::NotFound)?;

        let extents = node.get_mapping_extents().await?;
        let size = node.as_ref().byte_size();
        let stored_size = node.as_ref().stored_size().await?;
        let extent_count = extents.data.len() as u32;
        let metadata_count = extents.merkle.len() as u32;

        let allocation_size = (extent_count + metadata_count) as usize * std::mem::size_of::<u64>();

        if allocation_size > 0 {
            let mut payload = self.sender.reserve_payload(allocation_size).await?;
            let offset_in_vmo = payload.offset();

            let data_extents = Extents::try_new(&extents.data, 0)?;
            let merkle_extents = Extents::try_new(&extents.merkle, 0)?;

            for (mut chunk, val_res) in payload.data().chunks_mut(std::mem::size_of::<u64>()).zip(
                Extents::encode_extents(&data_extents)
                    .chain(Extents::encode_extents(&merkle_extents)),
            ) {
                chunk.copy_from_slice(&val_res.to_le_bytes());
            }

            let command = MappingCommand::Mappings {
                key,
                offset: offset_in_vmo as u32,
                stored_size,
                device_offset: 0,
                metadata_count,
                extent_count,
                encrypted: false,
            };

            payload.commit(command.into()).await?;
        }

        Ok(size)
    }

    /// Unregisters the blob mapping and signals the block driver to terminate tracking.
    async fn close_blob(&mut self, key: u64) -> Result<(), Error> {
        self.sender.push(MappingCommand::CloseBlob { key }.into()).await?;
        Ok(())
    }

    pub async fn handle_mapping_session_requests(
        mut self,
        mut stream: fmapping::MappingSessionRequestStream,
    ) {
        while let Ok(Some(request)) = stream.try_next().await {
            match request {
                fmapping::MappingSessionRequest::Open { key, identifier, responder } => {
                    // We expect the identifier to be a Merkle Root Hash for the blob.
                    if identifier.len() != fuchsia_hash::HASH_SIZE {
                        responder.send(Err(zx::Status::INVALID_ARGS.into_raw())).unwrap_or_else(
                            |error| warn!(error:?; "Failed to send mapping session response"),
                        );
                        continue;
                    }
                    let hash = Hash::from(<[u8; 32]>::try_from(identifier.as_slice()).unwrap());
                    match self.open_blob(key, hash).await {
                        Ok(size) => {
                            responder.send(Ok(size)).unwrap_or_else(
                                |error| warn!(error:?; "Failed to send mapping session response"),
                            );
                        }
                        Err(error) => {
                            error!(error:?; "Failed to open blob");
                            responder.send(Err(map_to_status(error).into_raw())).unwrap_or_else(
                                |error| warn!(error:?; "Failed to send mapping session response"),
                            );
                        }
                    }
                }
                fmapping::MappingSessionRequest::Close { key, responder } => {
                    let result = match self.close_blob(key).await {
                        Ok(()) => Ok(()),
                        Err(error) => {
                            error!(error:?; "Failed to close blob");
                            Err(map_to_status(error).into_raw())
                        }
                    };
                    responder.send(result).unwrap_or_else(
                        |error| warn!(error:?; "Failed to send mapping session response"),
                    );
                }
                fmapping::MappingSessionRequest::_UnknownMethod { ordinal, .. } => {
                    warn!(ordinal; "Unknown MappingSession method");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fuchsia::fxblob::testing::{BlobFixture, new_blob_fixture, open_blob_fixture};
    use crate::fuchsia::testing::TestFixture;
    use blob_writer::BlobWriter;
    use delivery_blob::{CompressionMode, Type1Blob};
    use fidl_fuchsia_io::UnlinkOptions;
    use fuchsia_async as fasync;
    use futures::channel::oneshot;
    use storage_device::Device;
    use storage_device::buffer::OwnedBuffer;
    use storage_device::buffer_allocator::{BufferAllocator, BufferSource};
    use vmo_fifo::Receiver;

    #[fuchsia::test]
    async fn test_blob_mapping_provider() {
        let fixture = new_blob_fixture().await;
        // Test with a large amount of non-compressible data to generate many extents
        let data = vec![42; 300_000];
        let hash = fixture.write_blob(&data, CompressionMode::Never).await;

        let blob_dir = fixture
            .volume()
            .root()
            .clone()
            .as_node()
            .into_any()
            .downcast::<BlobDirectory>()
            .expect("Failed to downcast root directory to BlobDirectory");

        let node = blob_dir
            .open_blob(&hash.into())
            .await
            .expect("Failed to open blob in Fxfs")
            .expect("open_blob returned None instead of node");
        let extents = node.get_mapping_extents().await.expect("Failed to retrieve extents");
        let data_extents = extents.data;
        let merkle_extents = extents.merkle;

        // Un-dropped nodes pin the Blob as actively opened. The unmount routine in fixture.close()
        // will wait forever for this blob to be fully closed, causing a test timeout.
        drop(node);

        let vmo = zx::Vmo::create(MAPPING_VMO_SIZE).unwrap();
        let client_mapping = vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
        let sender =
            AsyncSender::<RawMappingCommand>::new(vmo, 8, PENDING_COMMANDS_CAPACITY).unwrap();

        let mut session = BlobMappingSession::new(blob_dir, sender);

        let receiver_task = fasync::unblock(move || {
            let mut receiver = Receiver::<RawMappingCommand>::new(client_mapping, 256)
                .expect("Failed to create the Receiver wrapper");

            // First Open Command
            let cmd1_raw = receiver.peek().expect("peek failed");
            let cmd1 = MappingCommand::try_from(*cmd1_raw).expect("try_from failed");

            let (cmd1_offset, cmd1_extent_count, cmd1_metadata_count) = match cmd1 {
                MappingCommand::Mappings {
                    key,
                    offset,
                    stored_size: _,
                    device_offset: _,
                    metadata_count,
                    extent_count,
                    encrypted,
                } => {
                    assert_eq!(key, 1);
                    assert_eq!(extent_count, data_extents.len() as u32);
                    assert_eq!(metadata_count, merkle_extents.len() as u32);
                    assert!(!encrypted);
                    (offset, extent_count, metadata_count)
                }
                _ => panic!("Expected Mappings command"),
            };

            // Verify payload
            let total_extents = cmd1_extent_count + cmd1_metadata_count;
            let buffer = cmd1_raw.payload_slice(cmd1_offset, total_extents * 8).to_vec();

            let data_extents_container = Extents::try_new(&data_extents, 0).unwrap();
            let merkle_extents_container = Extents::try_new(&merkle_extents, 0).unwrap();
            let mut expected_payload = Vec::new();
            for val in Extents::encode_extents(&data_extents_container)
                .chain(Extents::encode_extents(&merkle_extents_container))
            {
                expected_payload.extend_from_slice(&val.to_le_bytes());
            }
            assert_eq!(buffer, expected_payload);
            cmd1_raw.pop().expect("Failed pop_commit");

            let cmd2_raw = receiver.peek().expect("peek failed");
            let cmd2 = MappingCommand::try_from(*cmd2_raw).expect("try_from failed");
            match cmd2 {
                MappingCommand::CloseBlob { key } => assert_eq!(key, 1),
                _ => panic!("Expected CloseBlob command"),
            };
            cmd2_raw.pop().expect("Failed pop_commit");
        });

        let server_task = async move {
            let key = 1;
            let size = session.open_blob(key, hash).await.expect("open_blob failed");
            assert_eq!(size, data.len() as u64);

            session.close_blob(key).await.expect("close_blob failed on existing key");

            std::mem::drop(session);
        };

        futures::join!(receiver_task, server_task);

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_missing_blob() {
        let fixture = new_blob_fixture().await;
        let blob_dir = fixture
            .volume()
            .root()
            .clone()
            .as_node()
            .into_any()
            .downcast::<BlobDirectory>()
            .expect("Failed to downcast");

        let vmo = zx::Vmo::create(MAPPING_VMO_SIZE).unwrap();
        let sender =
            AsyncSender::<RawMappingCommand>::new(vmo, 8, PENDING_COMMANDS_CAPACITY).unwrap();
        let mut session = BlobMappingSession::new(blob_dir, sender);
        let hash = Hash::from([1u8; 32]);
        session
            .open_blob(1, hash)
            .await
            .expect_err("open_blob should fail with blob that doesn't exist");

        std::mem::drop(session);
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_invalid_key() {
        let fixture = new_blob_fixture().await;
        let blob_dir = fixture
            .volume()
            .root()
            .clone()
            .as_node()
            .into_any()
            .downcast::<BlobDirectory>()
            .expect("Failed to downcast");

        let vmo = zx::Vmo::create(MAPPING_VMO_SIZE).unwrap();
        let sender =
            AsyncSender::<RawMappingCommand>::new(vmo, 8, PENDING_COMMANDS_CAPACITY).unwrap();
        let mut session = BlobMappingSession::new(blob_dir, sender);
        session.close_blob(42).await.expect("close_blob should return Ok with invalid key");
        std::mem::drop(session);

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_mapping_provider_and_mapping_session() {
        let fixture = new_blob_fixture().await;
        let data = vec![42; 8192];
        let hash = fixture.write_blob(&data, CompressionMode::Never).await;

        let blob_dir = fixture
            .volume()
            .root()
            .clone()
            .as_node()
            .into_any()
            .downcast::<BlobDirectory>()
            .expect("Failed to downcast root directory to BlobDirectory");

        let scope = blob_dir.volume().scope().clone();
        let server = Arc::new(
            BlobMappingProvider::new(blob_dir).expect("Failed to create BlobMappingProvider"),
        );

        // Spawn the mapping provider stream.
        let (provider_proxy, provider_server_end) =
            fidl::endpoints::create_proxy::<fmapping::MappingProviderMarker>();
        scope.spawn(async move {
            server.handle_mapping_provider_requests(provider_server_end.into_stream()).await;
        });

        // Open a mapping session from the mapping provider
        let (session_proxy, session_server_end) =
            fidl::endpoints::create_proxy::<fmapping::MappingSessionMarker>();
        let _shared_vmo = provider_proxy
            .open_session(session_server_end)
            .await
            .expect("open_session failed")
            .expect("vmo returned an error");

        let id: [u8; 32] = hash.into();
        let key = 1;
        let size = session_proxy
            .open(key, &id)
            .await
            .expect("open failed")
            .expect("open explicitly returned an error");

        assert_eq!(size, 8192);

        session_proxy
            .close(key)
            .await
            .expect("close failed")
            .expect("close explicitly returned an error");

        // Test some failures

        // Sending an invalid length hash to open should return INVALID_ARGS
        let bad_length_id = vec![1u8, 2, 3];
        let invalid_args_err = session_proxy
            .open(key, &bad_length_id)
            .await
            .expect("open wire call failed")
            .unwrap_err();
        assert_eq!(invalid_args_err, zx::Status::INVALID_ARGS.into_raw());

        // Sending a valid length hash that does not exist should return NOT_FOUND
        let not_found_id = [0u8; 32];
        let not_found_err = session_proxy
            .open(key, &not_found_id)
            .await
            .expect("open wire call failed")
            .unwrap_err();
        assert_eq!(not_found_err, zx::Status::NOT_FOUND.into_raw());

        // Trying to close an invalid blob key currently always succeeds. The BlobMappingServer
        // unconditionally passes the command down the FIFO queue to the block driver and doesn't
        // explicitly track active connections.
        session_proxy.close(123).await.expect("close wire call failed").expect("close failed");

        fixture.close().await;
    }

    struct DeviceBlockService {
        device: Arc<dyn Device>,
    }

    impl mapping::reader::BlockService for DeviceBlockService {
        fn allocate_buffer(&self, max_len: usize) -> OwnedBuffer {
            let block_size = self.device.block_size() as usize;
            let nblocks = (std::cmp::max(max_len, block_size) + block_size - 1) / block_size;
            let aligned_len = nblocks * block_size;
            let pool_size = nblocks.next_power_of_two() * block_size;
            let buffer_source = BufferSource::new(pool_size);
            let allocator = BufferAllocator::new(block_size, buffer_source);
            Arc::new(allocator).allocate_buffer_sync_owned(aligned_len)
        }

        fn read_blocks(
            &self,
            device_offset: u64,
            mut dest_buffer: OwnedBuffer,
            on_complete: Box<dyn FnOnce(Result<OwnedBuffer, anyhow::Error>) + Send>,
        ) -> Result<(), anyhow::Error> {
            let device = self.device.clone();
            futures::executor::block_on(async move {
                let res = device.read(device_offset, dest_buffer.as_mut()).await;
                on_complete(res.map(|_| dest_buffer));
            });
            Ok(())
        }
    }

    async fn run_blob_mapping_test(
        fixture: &TestFixture,
        test_data: &[u8],
        mode: CompressionMode,
        min_data_extents: usize,
        min_merkle_extents: usize,
    ) {
        let hash = fixture.write_blob(test_data, mode).await;
        run_blob_mapping_test_with_hash(
            fixture,
            hash,
            test_data,
            min_data_extents,
            min_merkle_extents,
        )
        .await;
    }

    async fn write_blob_chunked(fx: &TestFixture, data: &[u8], mode: CompressionMode) -> Hash {
        let hash = fuchsia_merkle::root_from_slice(data);
        let compressed_data = Type1Blob::generate(data, mode);
        let writer = fx.create_blob(&hash.into(), false).await.expect("create blob failed");
        let vmo = writer
            .get_vmo(compressed_data.len() as u64)
            .await
            .expect("transport error on get_vmo")
            .expect("failed to get vmo");
        let vmo_size = vmo.get_size().expect("failed to get vmo size");

        // Write in small chunks (512 B) to allocate extents across transactions.
        let chunk_size = 512;
        let mut write_offset = 0u64;
        let mut bytes_left = compressed_data.len() as u64;
        while bytes_left > 0 {
            let chunk_len = std::cmp::min(bytes_left, chunk_size);
            vmo.write(
                &compressed_data[write_offset as usize..(write_offset + chunk_len) as usize],
                write_offset % vmo_size,
            )
            .expect("failed to write to vmo");
            let _ = writer
                .bytes_ready(chunk_len)
                .await
                .expect("transport error on bytes_ready")
                .expect("failed to write data to vmo");
            write_offset += chunk_len;
            bytes_left -= chunk_len;
        }
        hash
    }

    async fn run_blob_mapping_test_with_hash(
        fixture: &TestFixture,
        hash: Hash,
        uncompressed_data: &[u8],
        min_data_extents: usize,
        min_merkle_extents: usize,
    ) {
        let blob_dir = fixture
            .volume()
            .root()
            .clone()
            .as_node()
            .into_any()
            .downcast::<BlobDirectory>()
            .expect("Failed to downcast root directory to BlobDirectory");

        let scope = blob_dir.volume().scope().clone();
        let server = Arc::new(
            BlobMappingProvider::new(blob_dir).expect("Failed to create BlobMappingProvider"),
        );

        let (provider_proxy, provider_server_end) =
            fidl::endpoints::create_proxy::<fmapping::MappingProviderMarker>();
        scope.spawn(async move {
            server.handle_mapping_provider_requests(provider_server_end.into_stream()).await;
        });

        let (session_proxy, session_server_end) =
            fidl::endpoints::create_proxy::<fmapping::MappingSessionMarker>();
        let mapping_vmo = provider_proxy
            .open_session(session_server_end)
            .await
            .expect("open_session failed")
            .expect("vmo returned an error");

        let mut receiver =
            vmo_fifo::Receiver::<RawMappingCommand>::new(mapping_vmo, PENDING_COMMANDS_CAPACITY)
                .expect("Failed to create receiver");

        let device = fixture.fs().device().clone();
        let service: Arc<dyn mapping::reader::BlockService> =
            Arc::new(DeviceBlockService { device });

        let port = zx::Port::create();
        let delivery_queue = zx::Vmo::create(mapping::DELIVERY_VMO_SIZE)
            .expect("Failed to create delivery_queue VMO");
        let pager = Arc::new(zx::Pager::create(zx::PagerOptions::empty()).unwrap());
        let vmo_provider = Arc::new(blob_pager_and_verifier::TestVmoProvider::new(
            pager.clone(),
            delivery_queue.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
        ));
        let delivery_receiver = vmo_fifo::Receiver::<mapping::RawDeliveryCommand>::new(
            delivery_queue.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
            mapping::PENDING_DELIVERY_COMMANDS_CAPACITY,
        )
        .unwrap();
        let _delivery_processor = blob_pager_and_verifier::DeliveryQueueProcessor::spawn(
            delivery_receiver,
            vmo_provider.clone(),
            delivery_queue.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
        )
        .unwrap();

        let verifier = block_server::verifier::Verifier::new(delivery_queue);
        let files = Arc::new(mapping::Files::new(service, verifier));

        let id: [u8; 32] = hash.into();
        let key = 1;
        let blob_size = session_proxy
            .open(key, &id)
            .await
            .expect("open failed")
            .expect("open explicitly returned an error");

        assert_eq!(blob_size as usize, uncompressed_data.len());

        let msg = receiver.peek().expect("Failed to peek message");
        if let Ok(MappingCommand::Mappings { extent_count, metadata_count, .. }) =
            MappingCommand::try_from(*msg)
        {
            if (extent_count as usize) < min_data_extents
                || (metadata_count as usize) < min_merkle_extents
            {
                panic!(
                    "EXTENTS MISMATCH: extent_count = {}, metadata_count = {}, \
                     required min_data = {}, min_merkle = {}",
                    extent_count, metadata_count, min_data_extents, min_merkle_extents
                );
            }
        }
        mapping::process_mapping_command(&msg, &files).expect("process_mapping_command failed");
        msg.pop().expect("pop failed");

        let paged_vmo = pager.create_vmo(zx::VmoOptions::empty(), &port, key, blob_size).unwrap();
        vmo_provider
            .register_vmo(key, paged_vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap());

        let _pager_thread = mapping::PagerThread::spawn(port, files.clone());

        let (tx, rx) = oneshot::channel();
        let len = uncompressed_data.len();
        std::thread::spawn(move || {
            let mut buf = vec![0u8; len];
            paged_vmo.read(&mut buf, 0).expect("paged vmo read failed");
            let _ = tx.send(buf);
        });

        let read_bytes = rx.await.unwrap();
        assert_eq!(&read_bytes[..], &uncompressed_data[..]);

        session_proxy.close(key).await.expect("close failed").expect("close error");
        let msg = receiver.peek().expect("Failed to peek close message");
        mapping::process_mapping_command(&msg, &files).expect("process_mapping_command failed");
        msg.pop().expect("pop failed");
    }

    async fn fragment_free_space(
        fixture: TestFixture,
        block_size: usize,
        anchor_size: Option<usize>,
    ) -> TestFixture {
        let mut data = vec![0u8; block_size];
        let mut hashes = Vec::new();
        let mut i = 0usize;
        loop {
            rand::fill(&mut data[..]);
            data[..8].copy_from_slice(&(i as u64).to_le_bytes());
            let hash = fuchsia_merkle::root_from_slice(&data);
            let delivery_data = Type1Blob::generate(&data, CompressionMode::Never);
            let writer = match fixture.create_blob(&hash.into(), false).await {
                Ok(w) => w,
                Err(_) => break,
            };
            if let Ok(mut blob_writer) =
                BlobWriter::create(writer, delivery_data.len() as u64).await
            {
                if blob_writer.write(&delivery_data).await.is_err() {
                    break;
                }
            } else {
                break;
            }
            hashes.push(hash);
            i += 1;
        }

        // Unlink every second small blob to create scattered free space holes.
        let root = fixture.root();
        for ix in (0..hashes.len()).step_by(2) {
            let _ = root.unlink(&format!("{}", hashes[ix]), &UnlinkOptions::default()).await;
        }

        // Remount fixture so unlinked blobs are purged and blocks are freed to allocator.
        let device = fixture.close().await;
        let fixture = open_blob_fixture(device).await;

        if let Some(size) = anchor_size {
            let anchor_data = vec![0u8; size];
            let _ = fixture.write_blob(&anchor_data, CompressionMode::Never).await;
        }

        fixture
    }

    #[fuchsia::test]
    async fn test_fxfs_blob_mapping_uncompressed() {
        let fixture = new_blob_fixture().await;
        let test_data = vec![123u8; 8192];
        run_blob_mapping_test(&fixture, &test_data, CompressionMode::Never, 1, 0).await;
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_fxfs_blob_mapping_compressed() {
        let fixture = new_blob_fixture().await;
        // Generate pseudo-random compressible test data.
        let mut test_data = vec![0u8; 16384];
        for i in 0..test_data.len() {
            test_data[i] = ((i / 64) % 256) as u8;
        }
        run_blob_mapping_test(&fixture, &test_data, CompressionMode::Always, 1, 0).await;
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_fxfs_blob_mapping_fragmented_data() {
        let fixture = new_blob_fixture().await;
        let fixture = fragment_free_space(fixture, 32768, Some(1_000_000)).await;

        let mut uncompressed_data = vec![0u8; 262_144];
        rand::fill(&mut uncompressed_data[..]);
        let hash1 = write_blob_chunked(&fixture, &uncompressed_data, CompressionMode::Never).await;
        run_blob_mapping_test_with_hash(&fixture, hash1, &uncompressed_data, 2, 0).await;
        fixture
            .root()
            .unlink(&format!("{}", hash1), &UnlinkOptions::default())
            .await
            .unwrap()
            .unwrap();

        let mut compressed_data = vec![0u8; 262_144];
        rand::fill(&mut compressed_data[..]);
        let hash2 = write_blob_chunked(&fixture, &compressed_data, CompressionMode::Always).await;
        run_blob_mapping_test_with_hash(&fixture, hash2, &compressed_data, 2, 0).await;

        fixture.close().await;
    }
}
