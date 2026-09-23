// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Implements fuchsia.fxfs/BlobWriter for writing delivery blobs.

use crate::fuchsia::directory::FxDirectory;
use crate::fuchsia::errors::map_to_status;
use crate::fuchsia::fxblob::BlobDirectory;
use crate::fuchsia::fxblob::blob::FxBlob;
use crate::fuchsia::node::{FxNode, GetResult};
use crate::fuchsia::volume::FxVolume;
use anyhow::{Context as _, Error};
use base64::prelude::{BASE64_STANDARD, Engine as _};
use delivery_blob::compression::{
    ChunkInfo, ChunkedDecompressor, CompressionAlgorithm, CompressionInfo, decode_archive,
};
use delivery_blob::{DeliveryBlob, MINIMUM_HEADER_SIZE};
use fidl::endpoints::RequestStream as _;
use fidl_fuchsia_fxfs::{BlobWriterRequest, BlobWriterRequestStream};
use fuchsia_merkle::{BufferedMerkleRootBuilder, Hash};
use futures::{TryStreamExt as _, try_join};
use fxfs::blob_metadata::{
    BlobFormat, BlobMetadata, BlobMetadataLeafHashCollector, FxfsBlobMetadataExt, MerkleLeaves,
};
use fxfs::errors::FxfsError;
use fxfs::object_handle::ObjectHandle;
use fxfs::object_store::data_object_handle::OverwriteOptions;
use fxfs::object_store::directory::{ReplacedChild, replace_child_with_object};
use fxfs::object_store::transaction::{LockKey, lock_keys};
use fxfs::object_store::{
    DataObjectHandle, HandleOptions, ObjectDescriptor, ObjectStore, Timestamp,
};
use fxfs::round::round_up;
use fxfs_trace::{TraceFutureExt, trace_future_args};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use zx::Status;

static RING_BUFFER_SIZE: LazyLock<u64> = LazyLock::new(|| 64 * storage_units::PAGE_SIZE);

const PAYLOAD_BUFFER_FLUSH_THRESHOLD: usize = 131_072; /* 128 KiB */

/// Maximum amount of *compressed* data we'll record when we fail to decompress a chunk.
/// We use base64 encoding, so the actual storage of the data will be large (~50 KiB).
const MAX_CHUNK_DATA_ON_ERROR: usize = 32_768; /* 32 KiB */

fn on_decompression_error(
    merkle_root: Hash,
    chunk_index: usize,
    chunk_info: ChunkInfo,
    chunk_data: &[u8],
) {
    log::error!("Failed to decompress chunk {chunk_index} ({chunk_info:?}) for blob {merkle_root}");
    static RECORDED_CORRUPT_BLOB: AtomicBool = AtomicBool::new(false);
    if RECORDED_CORRUPT_BLOB.fetch_or(true, Ordering::Relaxed) == true {
        // To avoid buffering too much data in memory, we only keep the first corruption instance.
        return;
    }
    let len = std::cmp::min(chunk_data.len(), MAX_CHUNK_DATA_ON_ERROR);
    let encoded_chunk = BASE64_STANDARD.encode(&chunk_data[..len]);
    fxfs::metrics::detail().record_child("delivery_blob_corruption", |node| {
        node.record_string("merkle_root", &String::from(merkle_root));
        node.record_int("chunk_index", chunk_index as i64);
        node.record_int("compressed_length", chunk_info.compressed_range.len() as i64);
        node.record_int("decompressed_length", chunk_info.decompressed_range.len() as i64);
        node.record_string("chunk_data", encoded_chunk);
    });
}

enum Stage {
    Writing(DataObjectHandle<FxVolume>),
    Complete,
}

impl Stage {
    fn complete(&mut self) -> DataObjectHandle<FxVolume> {
        let old = std::mem::replace(self, Stage::Complete);
        match old {
            Stage::Writing(handle) => handle,
            Stage::Complete => panic!("Called complete twice!"),
        }
    }

    fn handle(&self) -> &DataObjectHandle<FxVolume> {
        match self {
            Stage::Writing(handle) => handle,
            Stage::Complete => panic!("Tried to access handle after completion"),
        }
    }
}

/// Represents an RFC-0207 compliant delivery blob that is being written. Used to implement the
/// fuchsia.fxfs/BlobWriter protocol (see [`Self::handle_requests`]).
///
/// Once all data for the delivery blob has been written, the calculated Merkle root is verified
/// against [`Self::hash`]. On success, the blob is added under the [`Self::parent`] directory.
pub struct DeliveryBlobWriter {
    /// The expected hash for this blob. **MUST** match the Merkle root from [`Self::tree_builder`].
    hash: Hash,
    /// The parent directory we will add this blob to once verified.
    parent: Arc<FxDirectory>,
    /// Either Stage::Writing with an associated Handle to this blob within the filesystem, or
    /// Stage::Complete once the blob has finished writing.
    stage: Stage,
    /// Ring buffer used for the fuchsia.fxfs/BlobWriter protocol. Initialized when
    /// [`Self::get_vmo`] is called.
    ring_buffer: Option<RingBuffer>,
    /// Temporary buffer used to copy data from [`Self::ring_buffer`].
    buffer: Vec<u8>,
    /// The header for this delivery blob. Decoded once we have enough data.
    header: Option<DeliveryBlob>,
    /// The Merkle tree builder for this blob. Once the Merkle tree is complete, the root hash of
    /// the blob is verified, before storing the Merkle tree as part of the blob's metadata.
    tree_builder: BufferedMerkleRootBuilder<BlobMetadataLeafHashCollector>,
    /// Set to true when we've allocated space for the blob payload on disk.
    allocated_space: bool,
    /// How many bytes from the delivery blob payload have been written to disk so far.
    payload_persisted: u64,
    /// Offset within the delivery blob payload we started writing data to disk.
    payload_offset: u64,
    /// Streaming decompressor used to calculate the Merkle tree of compressed delivery blobs.
    /// Initialized once the seek table has been decoded and verified.
    decompressor: Option<ChunkedDecompressor>,
}

impl DeliveryBlobWriter {
    /// Creates a new object in the `parent` object store that manages writing the blob `hash`.
    /// After the blob is fully written and verified, it will be added as an entry under `parent`.
    /// If this object is dropped before the blob is fully written/verified, it will be tombstoned.
    pub(crate) async fn new(parent: &Arc<BlobDirectory>, hash: Hash) -> Result<Self, Error> {
        let parent = parent.directory().clone();
        let store = parent.store();
        let keys = lock_keys![LockKey::object(store.store_object_id(), parent.object_id())];
        let mut transaction = store
            .new_transaction(keys, Default::default())
            .await
            .context("Failed to create transaction.")?;
        let handle = ObjectStore::create_object(
            parent.volume(),
            &mut transaction,
            // Checksums are redundant for blobs, which are already content-verified.
            HandleOptions { skip_checksums: true, ..Default::default() },
            None,
        )
        .await
        .context("Failed to create object.")?;
        // Add the object to the graveyard so that it's cleaned up if we crash.
        store.add_to_graveyard(&mut transaction, handle.object_id());
        transaction.commit().await.context("Failed to commit transaction.")?;
        Ok(Self {
            hash,
            parent,
            stage: Stage::Writing(handle),
            ring_buffer: None,
            buffer: Default::default(),
            header: None,
            tree_builder: BufferedMerkleRootBuilder::new(BlobMetadataLeafHashCollector::new()),
            allocated_space: false,
            payload_persisted: 0,
            payload_offset: 0,
            decompressor: None,
        })
    }
}

impl Drop for DeliveryBlobWriter {
    /// Tombstone the object if we didn't finish writing the blob or the hash didn't match.
    fn drop(&mut self) {
        if let Stage::Writing(handle) = &self.stage {
            let store = handle.store();
            store
                .filesystem()
                .graveyard()
                .queue_tombstone_object(store.store_object_id(), handle.object_id());
        }
    }
}

impl DeliveryBlobWriter {
    fn header(&self) -> &DeliveryBlob {
        self.header.as_ref().unwrap()
    }

    fn ring_buffer(&self) -> &RingBuffer {
        self.ring_buffer.as_ref().unwrap()
    }

    fn decompressor(&self) -> &ChunkedDecompressor {
        self.decompressor.as_ref().unwrap()
    }

    fn storage_size(&self) -> usize {
        let header = self.header();
        if header.is_compressed {
            let seek_table = self.decompressor().seek_table();
            if seek_table.is_empty() {
                return 0;
            }
            // TODO(https://fxbug.dev/42078146): If the uncompressed size of the blob is smaller than the
            // filesystem's block size, we should decompress it before persisting it on disk.
            return seek_table.last().unwrap().compressed_range.end;
        }
        // Data is uncompressed, storage size is equal to the payload length.
        header.payload_length
    }

    async fn write_payload(&mut self) -> Result<(), Error> {
        self.ensure_allocated().await.with_context(|| {
            format!(
                "Failed to allocate space for blob (required space = {} bytes)",
                self.storage_size()
            )
        })?;
        let final_write = self.payload_persisted.checked_add(self.buffer.len() as u64)
            == Some(self.header().payload_length as u64);
        let block_size = self.stage.handle().block_size();
        let flush_threshold =
            std::cmp::max(block_size.get() as usize, PAYLOAD_BUFFER_FLUSH_THRESHOLD);
        // If we expect more data but haven't met the flush threshold, wait for more.
        if !final_write && self.buffer.len() < flush_threshold {
            return Ok(());
        }
        let len = if final_write {
            self.buffer.len()
        } else {
            block_size.align_down(self.buffer.len() as u64) as usize
        };
        // Update Merkle tree.
        let data = &self.buffer.as_slice()[..len];
        let update_merkle_tree_fut = async {
            if let Some(ref mut decompressor) = self.decompressor {
                // Data is compressed, decompress to update Merkle tree.
                decompressor
                    .update(data, &mut |chunk_data| self.tree_builder.write(chunk_data))
                    .context("Failed to decompress archive")?;
            } else {
                // Data is uncompressed, use payload to update Merkle tree.
                self.tree_builder.write(data);
            }
            Ok::<(), Error>(())
        };

        debug_assert!(self.payload_persisted >= self.payload_offset);

        // Copy data into transfer buffer, zero pad if required.
        let aligned_len = block_size.align_up(len as u64).ok_or(FxfsError::OutOfRange)? as usize;
        let mut buffer = self.stage.handle().allocate_buffer(aligned_len).await;
        buffer.as_mut().subslice_mut(..len).copy_from_slice(&self.buffer[..len]);
        buffer.as_mut().subslice_mut(len..).fill(0);

        // Overwrite allocated bytes in the object's handle.
        let overwrite_fut = async {
            self.stage
                .handle()
                .overwrite(
                    self.payload_persisted - self.payload_offset,
                    buffer.as_mut(),
                    OverwriteOptions::default(),
                )
                .await
                .context("Failed to write data to object handle.")
        };
        // NOTE: `overwrite_fut` needs to be polled first to initiate the asynchronous write to the
        // block device which will then run in parallel with the synchronous
        // `update_merkle_tree_fut`.
        try_join!(overwrite_fut, update_merkle_tree_fut)?;
        self.buffer.drain(..len);
        self.payload_persisted =
            self.payload_persisted.checked_add(len as u64).ok_or(FxfsError::OutOfRange)?;
        Ok(())
    }

    fn generate_metadata(&self, hashes: MerkleLeaves) -> Result<BlobMetadata, Error> {
        let metadata = match &self.decompressor {
            // Uncompressed blob.
            None => BlobMetadata { merkle_leaves: hashes, format: BlobFormat::Uncompressed },
            // The delivery blob said it was compressed but there are no chunks so it's actually the
            // empty blob.
            Some(decompressor) if decompressor.seek_table().is_empty() => BlobMetadata::empty(),
            // Compressed blob.
            Some(decompressor) => {
                let (uncompressed_size, chunk_size, compressed_offsets) =
                    parse_seek_table(decompressor.seek_table())
                        .context("Failed to parse seek table.")?;
                let format = match decompressor.algorithm() {
                    CompressionAlgorithm::Zstd => BlobFormat::ChunkedZstd {
                        uncompressed_size,
                        chunk_size,
                        compressed_offsets,
                    },
                    CompressionAlgorithm::Lz4 => {
                        BlobFormat::ChunkedLz4 { uncompressed_size, chunk_size, compressed_offsets }
                    }
                };
                BlobMetadata { merkle_leaves: hashes, format }
            }
        };
        Ok(metadata)
    }

    async fn ensure_allocated(&mut self) -> Result<(), Error> {
        if self.allocated_space {
            return Ok(());
        }
        let handle = self.stage.handle();
        let size = self.storage_size() as u64;
        let mut range = 0..handle.block_size().align_up(size).ok_or(FxfsError::OutOfRange)?;
        let mut first_time = true;
        while range.start < range.end {
            let mut transaction =
                handle.new_transaction().await.context("Failed to create transaction.")?;
            if first_time {
                handle
                    .grow(&mut transaction, 0, size)
                    .await
                    .with_context(|| format!("Failed to grow handle to {} bytes.", size))?;
                first_time = false;
            }
            handle.preallocate_range(&mut transaction, &mut range).await.with_context(|| {
                format!("Failed to allocate range ({} to {}).", range.start, range.end)
            })?;
            transaction.commit().await.context("Failed to commit transaction.")?;
        }
        self.allocated_space = true;
        Ok(())
    }

    async fn complete(&mut self) -> Result<(), Error> {
        debug_assert!(self.payload_persisted == self.header().payload_length as u64);
        // Finish building Merkle tree and verify the hash matches the filename.
        let (root, hashes) = std::mem::replace(
            &mut self.tree_builder,
            BufferedMerkleRootBuilder::new(BlobMetadataLeafHashCollector::new()),
        )
        .complete();
        if root != self.hash {
            return Err(FxfsError::IntegrityError).with_context(|| {
                format!(
                    "Calculated Merkle root ({}) does not match given hash ({}).",
                    root, self.hash
                )
            });
        }
        // Write metadata to disk.
        let metadata =
            self.generate_metadata(hashes).context("Failed to generate metadata for blob.")?;
        metadata.write_to(self.stage.handle()).await.context("Failed to write blob metadata")?;

        let compressed_size = self.payload_persisted - self.payload_offset;
        let compression_info = match &metadata.format {
            BlobFormat::Uncompressed => None,
            BlobFormat::ChunkedZstd { chunk_size, compressed_offsets, .. } => {
                Some(CompressionInfo::new(
                    *chunk_size,
                    compressed_size,
                    &compressed_offsets,
                    CompressionAlgorithm::Zstd,
                )?)
            }
            BlobFormat::ChunkedLz4 { chunk_size, compressed_offsets, .. } => {
                Some(CompressionInfo::new(
                    *chunk_size,
                    compressed_size,
                    &compressed_offsets,
                    CompressionAlgorithm::Lz4,
                )?)
            }
        };
        let merkle_verifier = metadata.into_merkle_verifier(root)?;

        let name = format!("{}", self.hash);
        let (volume, store, object_id) = {
            let handle = self.stage.handle();
            (handle.owner().clone(), handle.store(), handle.object_id())
        };
        let dir = volume
            .cache()
            .get(store.root_directory_object_id())
            .unwrap()
            .into_any()
            .downcast::<BlobDirectory>()
            .expect("Expected blob directory");

        let mut transaction = dir
            .directory()
            .directory()
            .acquire_context_for_replace(None, &name, false)
            .await
            .context("Failed to acquire context for replacement.")?
            .transaction;

        store.remove_from_graveyard(&mut transaction, object_id);

        let replacing = match replace_child_with_object(
            &mut transaction,
            Some((object_id, ObjectDescriptor::File)),
            (dir.directory().directory(), &name),
            0,
            false,
            Timestamp::now(),
        )
        .await
        .context("Replacing child failed.")?
        {
            ReplacedChild::None => None,
            ReplacedChild::Object(old) => {
                let reservation = match volume.cache().get_or_reserve(object_id).await {
                    GetResult::Placeholder(p) => p,
                    _ => unreachable!(),
                };
                Some(((old), reservation))
            }
            _ => return Err(FxfsError::Inconsistent.into()),
        };

        transaction
            .commit_with_callback(|_| {
                let handle = self.stage.complete();
                if replacing.is_some() {
                    self.parent.did_remove(&name);
                }
                self.parent.did_add(&name, None);
                if let Some((old_id, reservation)) = replacing {
                    // If the blob is in the cache, then we need to swap it.
                    match handle.owner().cache().get(old_id) {
                        Some(old_blob) => {
                            let old_blob = old_blob.into_any().downcast::<FxBlob>().unwrap();
                            let new_blob =
                                old_blob.overwrite_me(handle, merkle_verifier, compression_info);
                            old_blob.mark_to_be_purged();
                            reservation.commit(&(new_blob as Arc<dyn FxNode>));
                        }
                        None => {
                            // If it isn't open now, it is no longer reachable to be opened, so just
                            // tombstone it.
                            self.parent.store().filesystem().graveyard().queue_tombstone_object(
                                self.parent.store().store_object_id(),
                                old_id,
                            );
                        }
                    }
                }
            })
            .await
            .context("Failed to commit transaction!")?;
        Ok(())
    }

    /// Attempts to decode the delivery blob header from `self.buffer`.
    /// Returns `Ok(true)` if the header is decoded (or was already decoded), or `Ok(false)` if
    /// more data is needed.
    fn decode_delivery_blob_header(&mut self) -> Result<bool, Error> {
        if self.header.is_some() {
            return Ok(true);
        }
        // Type 3 delivery blobs are currently UNSTABLE / EXPERIMENTAL and subject to change,
        // so they are gated by `allow_type3_blobs` (defaults to false).
        let allow_type3 = self.parent.store().filesystem().options().allow_type3_blobs;
        let Some((header, payload)) = DeliveryBlob::parse(&self.buffer, allow_type3)
            .context("Failed to decode delivery blob header.")?
        else {
            return Ok(false); // Not enough data to decode header yet.
        };
        let expected_size = self.ring_buffer().expected_size();
        let delivery_size: u64 = (header.header.header_length as u64)
            .checked_add(header.payload_length as u64)
            .ok_or(FxfsError::OutOfRange)?;
        if expected_size != delivery_size {
            return Err(FxfsError::IntegrityError).with_context(|| {
                format!(
                    "Expected size ({}) does not match size from blob header ({})!",
                    expected_size, delivery_size
                )
            });
        }
        let header_len = self.buffer.len() - payload.len();
        self.buffer.drain(..header_len);
        self.header = Some(header);
        Ok(true)
    }

    /// Attempts to decode the chunked archive header and initialize the decompressor for compressed
    /// blobs. Returns `Ok(true)` if initialized (or uncompressed/already initialized), or
    /// `Ok(false)` if more data is needed.
    fn decode_archive_header(&mut self) -> Result<bool, Error> {
        if !self.header().is_compressed || self.decompressor.is_some() {
            return Ok(true);
        }
        let archive_length = self.header().payload_length;
        let Some((decoded_archive, chunk_data)) = decode_archive(&self.buffer, archive_length)
            .context("Failed to decode archive header")?
        else {
            return Ok(false); // Not enough data to decode archive header/seek table.
        };
        // We store the seek table out-of-line with the data, so we don't persist that
        // part of the payload directly.
        let archive_header_len = self.buffer.len() - chunk_data.len();
        self.buffer.drain(..archive_header_len);
        self.payload_offset = archive_header_len as u64;
        self.payload_persisted = self.payload_offset;
        let hash = self.hash;
        self.decompressor = Some(
            ChunkedDecompressor::new_with_error_handler(
                decoded_archive,
                Box::new(move |chunk_index, chunk_info, chunk_data| {
                    on_decompression_error(hash, chunk_index, chunk_info, chunk_data);
                }),
            )
            .context("Failed to create decompressor")?,
        );
        Ok(true)
    }

    /// Process the data that exists in the writer's buffer. The writer cannot recover from errors,
    /// so this function should not be invoked after it returns any errors.
    async fn process_buffer(&mut self) -> Result<(), Error> {
        if !self.decode_delivery_blob_header()? {
            return Ok(());
        }
        if !self.decode_archive_header()? {
            return Ok(());
        }
        // Write payload to disk and update Merkle tree.
        if !self.buffer.is_empty() {
            // This ends up being a large future, so we box it.
            Box::pin(self.write_payload()).await?;
        }
        Ok(())
    }

    fn get_vmo(&mut self, size: u64) -> Result<zx::Vmo, Error> {
        if self.ring_buffer.is_some() {
            return Err(Status::BAD_STATE).context("Blob was already truncated.");
        }
        let (ring_buffer, vmo_dup) = RingBuffer::new(size)
            .with_context(|| format!("Failed to truncate blob {} to size {}", self.hash, size))?;
        self.ring_buffer = Some(ring_buffer);
        Ok(vmo_dup)
    }

    /// Called when there is more data in the ring buffer ready to be processed. Once all data has
    /// been written and verified, it will be added to the parent directory.
    ///
    /// *WARNING*: If this function fails, the writer will remain in an invalid state. Errors should
    /// be latched by the caller instead of calling this function again. The writer can be closed,
    /// and a new one can be opened to attempt writing the delivery blob again.
    async fn bytes_ready(&mut self, bytes_written: u64) -> Result<(), Error> {
        let (write_offset, expected_size) = {
            let ring_buffer = self
                .ring_buffer
                .as_mut()
                .ok_or(Status::BAD_STATE)
                .context("BlobWriter.GetVmo must be called before BlobWriter.BytesReady.")?;
            (ring_buffer.read(bytes_written, &mut self.buffer)?, ring_buffer.expected_size())
        };
        // Process the data from the buffer.
        self.process_buffer().await.with_context(|| {
            // LINT.IfChange(blob_write_failure)
            format!(
                "failed to write blob {} (bytes_written = {}, offset = {}, delivery_size = {})",
                self.hash, bytes_written, write_offset, expected_size
            )
            // LINT.ThenChange(/tools/testing/tefmocheck/string_in_log_check.go:blob_write_failure)
        })?;
        // If all bytes for this delivery blob were written successfully, attempt to verify the blob
        // and add it to the parent directory.
        if self.ring_buffer().is_complete() {
            self.complete().await?;
        }
        Ok(())
    }

    /// Handle fuchsia.fxfs/BlobWriter requests for this delivery blob.
    pub(crate) async fn handle_requests(
        mut self,
        mut request_stream: BlobWriterRequestStream,
    ) -> Result<(), Error> {
        while let Some(request) = request_stream.try_next().await? {
            match request {
                BlobWriterRequest::GetVmo { size, responder } => {
                    fxfs_trace::duration!("BlobWriter::GetVmo");
                    let result = self.get_vmo(size).map_err(|error| {
                        log::error!(error:?; "BlobWriter.GetVmo failed.");
                        map_to_status(error).into_raw()
                    });
                    responder.send(result).unwrap_or_else(|error| {
                        log::error!(error:?; "Failed to send BlobWriter.GetVmo response.");
                    });
                }
                BlobWriterRequest::BytesReady { bytes_written, responder } => {
                    let result = async {
                        let result = self.bytes_ready(bytes_written).await.map_err(|error| {
                            log::error!(error:?; "BlobWriter.BytesReady failed.");
                            map_to_status(error)
                        });
                        responder.send(result.map_err(Status::into_raw)).unwrap_or_else(|error| {
                            log::error!(error:?; "Failed to send BlobWriter.BytesReady response.");
                        });
                        result
                    }
                    .trace(trace_future_args!("BlobWriter::BytesReady"))
                    .await;

                    // If any error occurs when handling a BytesReady request, the writer will
                    // remain in an unrecoverable state. The client must use the BlobCreator
                    // protocol to open a new writer and try writing the blob again.
                    if let Err(status) = result {
                        request_stream.control_handle().shutdown_with_epitaph(status);
                        break;
                    }
                }
            }
        }
        Ok(())
    }
}

fn parse_seek_table(
    seek_table: &[ChunkInfo],
) -> Result<(/*uncompressed_size*/ u64, /*chunk_size*/ u64, /*compressed_offsets*/ Vec<u64>), Error>
{
    let uncompressed_size = seek_table.last().unwrap().decompressed_range.end;
    let chunk_size = seek_table.first().unwrap().decompressed_range.len();
    // fxblob only supports archives with equally sized chunks.
    if seek_table.len() > 1
        && seek_table[1..seek_table.len() - 1]
            .iter()
            .any(|entry| entry.decompressed_range.len() != chunk_size)
    {
        return Err(FxfsError::NotSupported)
            .context("Unsupported archive: compressed length of each chunk must be the same");
    }
    let compressed_offsets = seek_table
        .iter()
        .map(|entry| TryInto::<u64>::try_into(entry.compressed_range.start))
        .collect::<Result<Vec<_>, _>>()?;

    // TODO(https://fxbug.dev/42078146): The pager assumes chunk_size alignment is at least the size of a
    // Merkle tree block. We should allow arbitrary chunk sizes. For now, we reject archives with
    // multiple chunks that don't meet this requirement (since we control archive generation), and
    // round up the chunk size for archives with a single chunk, as we won't read past the file end.
    let chunk_size: u64 = chunk_size.try_into()?;
    let alignment: u64 = fuchsia_merkle::BLOCK_SIZE.try_into()?;
    let aligned_chunk_size = if seek_table.len() > 1 {
        if chunk_size < alignment || chunk_size % alignment != 0 {
            return Err(FxfsError::NotSupported)
                .context("Unsupported archive: chunk size must be multiple of Merkle tree block");
        }
        chunk_size
    } else {
        round_up(chunk_size, alignment).unwrap()
    };

    Ok((uncompressed_size.try_into()?, aligned_chunk_size, compressed_offsets))
}

/// Manages the ring buffer used for the fuchsia.fxfs/BlobWriter protocol.
struct RingBuffer {
    /// Total number of bytes we expect for this delivery blob (via fuchsia.fxfs/BlobWriter.GetVmo).
    /// This is checked against the header + payload size encoded within the delivery blob.
    expected_size: u64,
    /// Total number of bytes that have been written to this delivery blob so far. When this reaches
    /// [`Self::expected_size`], the entire blob will have been received.
    total_written: u64,
    /// Vmo for the ring buffer used for the fuchsia.fxfs/BlobWriter protocol.
    vmo: zx::Vmo,
}

impl RingBuffer {
    fn new(expected_size: u64) -> Result<(Self, zx::Vmo), Error> {
        if expected_size < MINIMUM_HEADER_SIZE as u64 {
            return Err(Status::INVALID_ARGS).context("Invalid size (too small).");
        }
        let vmo = zx::Vmo::create(*RING_BUFFER_SIZE).with_context(|| {
            format!("Failed to create VMO of size {} for writing", *RING_BUFFER_SIZE)
        })?;
        let vmo_dup = vmo
            .duplicate_handle(zx::Rights::SAME_RIGHTS)
            .context("Failed to duplicate VMO handle")?;
        Ok((Self { expected_size, total_written: 0, vmo }, vmo_dup))
    }

    fn expected_size(&self) -> u64 {
        self.expected_size
    }

    fn is_complete(&self) -> bool {
        self.total_written == self.expected_size
    }

    /// Reads `bytes_written` from the VMO and appends them to `buf`. Returns the offset at which
    /// the write started.
    fn read(&mut self, bytes_written: u64, buf: &mut Vec<u8>) -> Result<u64, Error> {
        if bytes_written > *RING_BUFFER_SIZE {
            return Err(FxfsError::OutOfRange).with_context(|| {
                format!(
                    "bytes_written ({}) exceeds size of ring buffer ({})",
                    bytes_written, *RING_BUFFER_SIZE
                )
            });
        }
        let write_offset = self.total_written;
        self.total_written = write_offset
            .checked_add(bytes_written)
            .ok_or(Status::OUT_OF_RANGE)
            .context("Total written bytes overflowed")?;
        if self.total_written > self.expected_size {
            return Err(Status::BUFFER_TOO_SMALL).with_context(|| {
                format!(
                    "Wrote more bytes than passed to BlobWriter.GetVmo\
                     (expected = {}, written = {}).",
                    self.expected_size, self.total_written
                )
            });
        }
        // Extend our write buffer by the amount of data that was written.
        let prev_len = buf.len();
        let new_len = prev_len.checked_add(bytes_written as usize).ok_or(Status::OUT_OF_RANGE)?;
        buf.resize(new_len, 0);

        let new_section = &mut buf[prev_len..];
        // Copy data from the ring buffer into our internal buffer.
        let vmo_offset = write_offset % *RING_BUFFER_SIZE;
        if vmo_offset + bytes_written > *RING_BUFFER_SIZE {
            let split = (*RING_BUFFER_SIZE - vmo_offset) as usize;
            self.vmo
                .read(&mut new_section[0..split], vmo_offset)
                .context("failed to read from VMO")?;
            self.vmo.read(&mut new_section[split..], 0).context("failed to read from VMO")?;
        } else {
            self.vmo.read(new_section, vmo_offset).context("failed to read from VMO")?;
        }
        Ok(write_offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fuchsia::fxblob::testing::{BlobFixture, new_blob_fixture, open_blob_fixture};
    use crate::fuchsia::testing::{TestFixture, TestFixtureOptions};
    use core::ops::Range;
    use delivery_blob::{CompressionMode, Type1Blob, Type3Blob};
    use fidl_fuchsia_fxfs::{BlobCreatorMarker, BlobWriterProxy, CreateBlobError};
    use fidl_fuchsia_io::UnlinkOptions;
    use fuchsia_async::epoch::Epoch;
    use fuchsia_async::{self as fasync, TimeoutExt as _};
    use fuchsia_component_client::connect_to_protocol_at_dir_svc;
    use fxfs_make_blob_image::{CompressionAlgorithm, FxBlobBuilder};
    use storage_device::DeviceHolder;
    use storage_device::fake_device::FakeDevice;

    fn generate_list_of_writes(compressed_data_len: u64) -> Vec<Range<u64>> {
        let mut list_of_writes = vec![];
        let mut bytes_left_to_write = compressed_data_len;
        let mut write_offset = 0;
        let half_ring_buffer = *RING_BUFFER_SIZE / 2;
        while bytes_left_to_write > half_ring_buffer {
            list_of_writes.push(write_offset..write_offset + half_ring_buffer);
            write_offset += half_ring_buffer;
            bytes_left_to_write -= half_ring_buffer;
        }
        if bytes_left_to_write > 0 {
            list_of_writes.push(write_offset..write_offset + bytes_left_to_write);
        }
        list_of_writes
    }

    async fn write_delivery_blob_in_chunks(writer: &BlobWriterProxy, vmo: &zx::Vmo, data: &[u8]) {
        let vmo_size = vmo.get_size().expect("failed to get vmo size");
        let list_of_writes = generate_list_of_writes(data.len() as u64);
        for range in list_of_writes {
            let len = range.end - range.start;
            vmo.write(&data[range.start as usize..range.end as usize], range.start % vmo_size)
                .expect("failed to write to vmo");
            writer
                .bytes_ready(len)
                .await
                .expect("transport error on bytes_ready")
                .expect("failed to write data to vmo");
        }
    }

    async fn write_delivery_blob_all_at_once(writer: &BlobWriterProxy, vmo: &zx::Vmo, data: &[u8]) {
        vmo.write(data, 0).expect("failed to write to vmo");
        writer
            .bytes_ready(data.len() as u64)
            .await
            .expect("transport error on bytes_ready")
            .expect("failed to write data to vmo");
    }

    /// Tests for the new write API.
    #[fuchsia::test(threads = 10)]
    async fn test_new_write_empty_blob() {
        let fixture = new_blob_fixture().await;

        let data = vec![];
        let hash = fuchsia_merkle::root_from_slice(&data);
        let compressed_data = Type1Blob::generate(&data, CompressionMode::Always);

        {
            let writer =
                fixture.create_blob(&hash.into(), false).await.expect("failed to create blob");
            let vmo = writer
                .get_vmo(compressed_data.len() as u64)
                .await
                .expect("transport error on get_vmo")
                .expect("failed to get vmo");
            write_delivery_blob_in_chunks(&writer, &vmo, &compressed_data).await;
        }
        assert_eq!(fixture.read_blob(hash).await, data);
        fixture.close().await;
    }

    /// We should fail early when truncating a delivery blob if the size is too small.
    #[fuchsia::test(threads = 10)]
    async fn test_reject_too_small() {
        let fixture = new_blob_fixture().await;
        let hash = fuchsia_merkle::root_from_slice(&[]);
        // The smallest possible delivery blob should be an uncompressed null/empty Type 1 blob.
        let delivery_data = Type1Blob::generate(&[], CompressionMode::Never);

        {
            let writer =
                fixture.create_blob(&hash.into(), false).await.expect("failed to create blob");
            assert_eq!(
                writer
                    .get_vmo(delivery_data.len() as u64 - 1)
                    .await
                    .expect("transport error on get_vmo")
                    .map_err(Status::err_from_raw)
                    .expect_err("get_vmo unexpectedly succeeded"),
                zx::Status::INVALID_ARGS
            );
        }
        fixture.close().await;
    }

    /// Calling bytes_ready with a value that would cause integer overflow should fail.
    #[fuchsia::test(threads = 10)]
    async fn test_reject_exceeds_ring_buffer_size() {
        let fixture = new_blob_fixture().await;
        let data = vec![1; 100];
        let hash = fuchsia_merkle::root_from_slice(&data);
        let delivery_data = Type1Blob::generate(&data, CompressionMode::Never);

        {
            let writer =
                fixture.create_blob(&hash.into(), false).await.expect("failed to create blob");
            let _vmo = writer
                .get_vmo(delivery_data.len() as u64)
                .await
                .expect("transport error on get_vmo")
                .expect("failed to get vmo");

            assert_eq!(
                writer
                    .bytes_ready(*RING_BUFFER_SIZE + 10)
                    .await
                    .expect("transport error on bytes_ready")
                    .map_err(Status::err_from_raw)
                    .expect_err("bytes_ready unexpectedly succeeded"),
                zx::Status::OUT_OF_RANGE
            );
        }
        fixture.close().await;
    }

    /// A blob should fail to write if the calculated Merkle root doesn't match the filename.
    #[fuchsia::test(threads = 10)]
    async fn test_reject_bad_hash() {
        let fixture = new_blob_fixture().await;

        let data = vec![3; 1_000];
        let incorrect_hash = fuchsia_merkle::root_from_slice(&data[..data.len() - 1]);
        let delivery_data = Type1Blob::generate(&data, CompressionMode::Never);

        {
            let writer = fixture
                .create_blob(&incorrect_hash.into(), false)
                .await
                .expect("failed to create blob");
            let vmo = writer
                .get_vmo(delivery_data.len() as u64)
                .await
                .expect("transport error on get_vmo")
                .expect("failed to get vmo");
            vmo.write(&delivery_data, 0).expect("failed to write to vmo");

            assert_eq!(
                writer
                    .bytes_ready(delivery_data.len() as u64)
                    .await
                    .expect("transport error on bytes_ready")
                    .map_err(Status::err_from_raw)
                    .expect_err("write unexpectedly succeeded"),
                zx::Status::IO_DATA_INTEGRITY
            );
        }
        fixture.close().await;
    }

    /// Ensure we get `IO_DATA_INTEGRITY` if one of the compressed chunks is corrupted.
    #[fuchsia::test(threads = 10)]
    async fn test_detect_corruption() {
        let fixture = new_blob_fixture().await;

        let mut data = vec![1; 65536];
        rand::fill(&mut data[..]);

        let hash = fuchsia_merkle::root_from_slice(&data);
        let mut compressed_data = Type1Blob::generate(&data, CompressionMode::Always);
        let len = compressed_data.len();
        compressed_data[len - 1024..].fill(0);
        {
            let writer =
                fixture.create_blob(&hash.into(), false).await.expect("failed to create blob");
            let vmo = writer
                .get_vmo(compressed_data.len() as u64)
                .await
                .expect("transport error on get_vmo")
                .expect("failed to get vmo");
            vmo.write(&compressed_data, 0).expect("failed to write to vmo");

            assert_eq!(
                writer
                    .bytes_ready(compressed_data.len() as u64)
                    .await
                    .expect("transport error on bytes_ready")
                    .map_err(Status::err_from_raw)
                    .expect_err("write unexpectedly succeeded"),
                zx::Status::IO_DATA_INTEGRITY
            );
        }
        fixture.close().await;
    }

    #[fuchsia::test(threads = 10)]
    async fn test_new_rewrite_succeeds() {
        let fixture = new_blob_fixture().await;

        let mut data = vec![1; 196608];
        rand::fill(&mut data[..]);

        let hash = fuchsia_merkle::root_from_slice(&data);
        let compressed_data = Type1Blob::generate(&data, CompressionMode::Always);

        {
            let writer =
                fixture.create_blob(&hash.into(), false).await.expect("failed to create blob");
            let vmo = writer
                .get_vmo(compressed_data.len() as u64)
                .await
                .expect("transport error on get_vmo")
                .expect("failed to get vmo");

            let writer_2 =
                fixture.create_blob(&hash.into(), false).await.expect("failed to create blob");
            let vmo_2 = writer_2
                .get_vmo(compressed_data.len() as u64)
                .await
                .expect("transport error on get_vmo")
                .expect("failed to get vmo");

            write_delivery_blob_in_chunks(&writer, &vmo, &compressed_data).await;
            write_delivery_blob_in_chunks(&writer_2, &vmo_2, &compressed_data).await;
        }
        assert_eq!(fixture.read_blob(hash).await, data);
        fixture.close().await;
    }

    #[fuchsia::test(threads = 10)]
    async fn test_new_tombstone_dropped_incomplete_delivery_blobs() {
        let fixture = new_blob_fixture().await;
        const BLOB_SIZE: u64 = 4 * 1024 * 1024;
        // Sanity check that an empty fxfs isn't 4MiB.
        assert!(fixture.fs().allocator().get_allocated_bytes() < BLOB_SIZE);

        {
            let data = vec![1; BLOB_SIZE as usize];
            let hash = fuchsia_merkle::root_from_slice(&data);
            let compressed_data = Type1Blob::generate(&data, CompressionMode::Never);
            let writer =
                fixture.create_blob(&hash.into(), false).await.expect("failed to create blob");
            let vmo = writer
                .get_vmo(compressed_data.len() as u64)
                .await
                .expect("transport error on get_vmo")
                .expect("failed to get vmo");
            // Write all but the last byte to avoid completing the blob.
            write_delivery_blob_in_chunks(
                &writer,
                &vmo,
                &compressed_data[0..compressed_data.len() - 1],
            )
            .await;
            // At least 4MiB should now be allocated in fxfs.
            assert!(fixture.fs().allocator().get_allocated_bytes() > BLOB_SIZE);
        }

        // The BlobWriterProxy was dropped above but the DeliveryBlobWriter won't be dropped and the
        // blob added to the tombstone queue until the PEER_CLOSED signal from dropping the proxy is
        // handled. Waiting on the graveyard to flush doesn't work because the flush will be queued
        // before the blob.
        //
        // Wait for the allocated bytes to drop back below the size of the blob as a way of
        // verifying that the blob was removed.
        async {
            while fixture.fs().allocator().get_allocated_bytes() > BLOB_SIZE {
                fasync::Timer::new(std::time::Duration::from_millis(5)).await;
            }
        }
        .on_timeout(std::time::Duration::from_secs(10), || {
            panic!("The partially written blob was not cleaned up")
        })
        .await;

        fixture.close().await;
    }

    #[fuchsia::test(threads = 10)]
    async fn test_new_write_small_blob_no_wrap() {
        let fixture = new_blob_fixture().await;

        let mut data = vec![1; 196608];
        rand::fill(&mut data[..]);

        let hash = fuchsia_merkle::root_from_slice(&data);
        let compressed_data = Type1Blob::generate(&data, CompressionMode::Always);

        {
            let writer =
                fixture.create_blob(&hash.into(), false).await.expect("failed to create blob");
            let vmo = writer
                .get_vmo(compressed_data.len() as u64)
                .await
                .expect("transport error on get_vmo")
                .expect("failed to get vmo");
            write_delivery_blob_in_chunks(&writer, &vmo, &compressed_data).await;
        }
        assert_eq!(fixture.read_blob(hash).await, data);
        fixture.close().await;
    }

    #[fuchsia::test(threads = 10)]
    async fn test_new_write_blob_already_exists() {
        let fixture = new_blob_fixture().await;

        let data = vec![1; 65536];

        let hash = fuchsia_merkle::root_from_slice(&data);
        let delivery_data = Type1Blob::generate(&data, CompressionMode::Never);

        {
            let writer =
                fixture.create_blob(&hash.into(), false).await.expect("failed to create blob");
            let vmo = writer
                .get_vmo(delivery_data.len() as u64)
                .await
                .expect("transport error on get_vmo")
                .expect("failed to get vmo");
            write_delivery_blob_all_at_once(&writer, &vmo, &delivery_data).await;

            assert_eq!(fixture.read_blob(hash).await, data);
            assert_eq!(
                fixture.create_blob(&hash.into(), false).await.expect_err("rewrite succeeded"),
                CreateBlobError::AlreadyExists
            );
        }

        fixture.close().await;
    }

    #[fuchsia::test(threads = 10)]
    async fn test_new_write_large_blob_wraps() {
        let fixture = new_blob_fixture().await;

        let mut data = vec![1; 1024921];
        rand::fill(&mut data[..]);

        let hash = fuchsia_merkle::root_from_slice(&data);
        let compressed_data = Type1Blob::generate(&data, CompressionMode::Always);
        assert!(compressed_data.len() as u64 > *RING_BUFFER_SIZE);

        {
            let writer =
                fixture.create_blob(&hash.into(), false).await.expect("failed to create blob");
            let vmo = writer
                .get_vmo(compressed_data.len() as u64)
                .await
                .expect("transport error on get_vmo")
                .expect("failed to get vmo");
            write_delivery_blob_in_chunks(&writer, &vmo, &compressed_data).await;
        }
        assert_eq!(fixture.read_blob(hash).await, data);
        fixture.close().await;
    }

    #[fuchsia::test(threads = 10)]
    async fn test_allocate_with_large_transaction() {
        const NUM_BLOBS: usize = 1024;
        let fixture = new_blob_fixture().await;

        let mut data = [0; 128];
        let mut hashes = Vec::new();

        // It doesn't matter if these blobs are compressed. We just need to fragment space.
        for _ in 0..NUM_BLOBS {
            rand::fill(&mut data[..]);
            let hash = fixture.write_blob(&data, CompressionMode::Never).await;
            hashes.push(hash);
        }

        // Delete every second blob, fragmenting free space.
        for ix in 0..NUM_BLOBS / 2 {
            fixture
                .root()
                .unlink(&format!("{}", hashes[ix * 2]), &UnlinkOptions::default())
                .await
                .expect("FIDL failed")
                .expect("unlink failed");
        }

        // Create one large blob (reusing fragmented extents).
        {
            let mut data = vec![1; 1024921];
            rand::fill(&mut data[..]);

            let hash = fuchsia_merkle::root_from_slice(&data);
            let compressed_data = Type1Blob::generate(&data, CompressionMode::Always);
            assert!(compressed_data.len() as u64 > *RING_BUFFER_SIZE);

            let writer =
                fixture.create_blob(&hash.into(), false).await.expect("failed to create blob");
            let vmo = writer
                .get_vmo(compressed_data.len() as u64)
                .await
                .expect("transport error on get_vmo")
                .expect("failed to get vmo");
            write_delivery_blob_in_chunks(&writer, &vmo, &compressed_data).await;
            // Ensure that the blob is readable and matches what we expect.
            assert_eq!(fixture.read_blob(hash).await, data);
        }

        fixture.close().await;
    }

    #[fuchsia::test(threads = 10)]
    async fn bytes_ready_should_fail_if_size_invalid() {
        let fixture = new_blob_fixture().await;
        // Generate a delivery blob (size doesn't matter).
        let mut data = vec![1; 196608];
        rand::fill(&mut data[..]);
        let hash = fuchsia_merkle::root_from_slice(&data);
        let blob_data = Type1Blob::generate(&data, CompressionMode::Always);
        // To simplify the test, we make sure to write enough bytes on the first call to bytes_ready
        // so that the header can be decoded (and thus the length mismatch is detected).
        let bytes_to_write = std::cmp::min((*RING_BUFFER_SIZE / 2) as usize, blob_data.len());
        Type1Blob::parse(&blob_data[..bytes_to_write])
            .unwrap()
            .expect("bytes_to_write must be long enough to cover delivery blob header!");
        // We can call get_vmo with the wrong size, because we don't know what size the blob should
        // be. Once we call bytes_ready with enough data for the header to be decoded, we should
        // detect the failure.
        for incorrect_size in [blob_data.len() - 1, blob_data.len() + 1] {
            let writer =
                fixture.create_blob(&hash.into(), false).await.expect("failed to create blob");
            let vmo = writer
                .get_vmo(incorrect_size as u64)
                .await
                .expect("transport error on get_vmo")
                .expect("failed to get vmo");
            assert!(vmo.get_size().unwrap() >= bytes_to_write as u64);
            vmo.write(&blob_data[..bytes_to_write], 0).unwrap();
            writer
                .bytes_ready(bytes_to_write as u64)
                .await
                .expect("transport error on bytes_ready")
                .expect_err("should fail writing blob if size passed to get_vmo is incorrect");
        }
        fixture.close().await;
    }

    #[fuchsia::test(threads = 10)]
    async fn allow_existing_cleans_up_old_while_open() {
        let fixture = new_blob_fixture().await;

        let data = vec![1; 65536];

        let hash = fuchsia_merkle::root_from_slice(&data);
        let delivery_data = Type1Blob::generate(&data, CompressionMode::Never);

        {
            let writer =
                fixture.create_blob(&hash.into(), true).await.expect("failed to create blob");
            let vmo = writer
                .get_vmo(delivery_data.len() as u64)
                .await
                .expect("transport error on get_vmo")
                .expect("failed to get vmo");
            write_delivery_blob_all_at_once(&writer, &vmo, &delivery_data).await;

            assert_eq!(fixture.read_blob(hash).await, data);
        };
        {
            let old_blob = fixture.get_blob(hash).await.expect("Looking up blob");
            let old_id = old_blob.object_id();

            let writer =
                fixture.create_blob(&hash.into(), true).await.expect("failed to create blob");
            let vmo = writer
                .get_vmo(delivery_data.len() as u64)
                .await
                .expect("transport error on get_vmo")
                .expect("failed to get vmo");
            write_delivery_blob_all_at_once(&writer, &vmo, &delivery_data).await;

            let new_blob = fixture.get_blob(hash).await.expect("Looking up blob");
            assert_ne!(new_blob.object_id(), old_id);

            // Wait for our Arc to be the last strong ref, then it gets dropped. Nothing else
            // should be referencing it here after the tasks complete.
            async move {
                while Arc::strong_count(&old_blob) > 1 {
                    fasync::Timer::new(std::time::Duration::from_millis(25)).await;
                }
            }
            .on_timeout(std::time::Duration::from_secs(10), || {
                panic!("The old blob was not cleaned up")
            })
            .await;

            fixture.fs().graveyard().flush().await;
            fixture
                .volume()
                .volume()
                .get_or_load_node(old_id, ObjectDescriptor::File, None)
                .await
                .unwrap_err();

            // Blob is still readable.
            assert_eq!(fixture.read_blob(hash).await, data);
        };
        fixture.close().await;
    }

    #[fuchsia::test(threads = 10)]
    async fn allow_existing_cleans_up_old_while_vmo_held() {
        let fixture = new_blob_fixture().await;

        let data = vec![1; 65536];

        let hash = fuchsia_merkle::root_from_slice(&data);
        let hash_string: String = hash.into();
        let delivery_data = Type1Blob::generate(&data, CompressionMode::Never);

        {
            let writer =
                fixture.create_blob(&hash.into(), true).await.expect("failed to create blob");
            let vmo = writer
                .get_vmo(delivery_data.len() as u64)
                .await
                .expect("transport error on get_vmo")
                .expect("failed to get vmo");
            write_delivery_blob_all_at_once(&writer, &vmo, &delivery_data).await;

            assert_eq!(fixture.read_blob(hash).await, data);
        };
        {
            let blob_dir =
                fixture.volume().root().clone().into_any().downcast::<BlobDirectory>().unwrap();
            let (old_id, _, _) =
                blob_dir.directory().directory().lookup(&hash_string).await.unwrap().expect("");
            let read_vmo = fixture.get_blob_vmo(hash).await;

            {
                let writer =
                    fixture.create_blob(&hash.into(), true).await.expect("failed to create blob");
                let vmo = writer
                    .get_vmo(delivery_data.len() as u64)
                    .await
                    .expect("transport error on get_vmo")
                    .expect("failed to get vmo");
                write_delivery_blob_all_at_once(&writer, &vmo, &delivery_data).await;
            }

            let (new_id, _, _) =
                blob_dir.directory().directory().lookup(&hash_string).await.unwrap().expect("");
            assert_ne!(new_id, old_id);

            // The old blob data should be gone.
            Epoch::global().barrier().await;
            fixture.fs().graveyard().flush().await;
            fixture
                .volume()
                .volume()
                .get_or_load_node(old_id, ObjectDescriptor::File, None)
                .await
                .unwrap_err();

            // Can read from the original vmo.
            let mut buf = [0u8; 1];
            read_vmo.read(buf.as_mut_slice(), 0).expect("Reading from vmo");
            assert_eq!(buf[0], data[0]);
        };
        fixture.close().await;
    }

    #[fuchsia::test(threads = 10)]
    async fn allow_existing_cleans_up_old_while_closed() {
        let fixture = new_blob_fixture().await;

        let data = vec![1; 65536];

        let hash = fuchsia_merkle::root_from_slice(&data);
        let delivery_data = Type1Blob::generate(&data, CompressionMode::Never);

        {
            let writer =
                fixture.create_blob(&hash.into(), true).await.expect("failed to create blob");
            let vmo = writer
                .get_vmo(delivery_data.len() as u64)
                .await
                .expect("transport error on get_vmo")
                .expect("failed to get vmo");
            write_delivery_blob_all_at_once(&writer, &vmo, &delivery_data).await;

            assert_eq!(fixture.read_blob(hash).await, data);
        }

        let blob_dir =
            fixture.volume().root().clone().into_any().downcast::<BlobDirectory>().unwrap();
        let hash_string: String = hash.into();
        let (old_id, _, _) =
            blob_dir.directory().directory().lookup(&hash_string).await.unwrap().expect("");
        // The `read_blob` call above gets a VMO for the blob. Even though the VMO is dropped, the
        // blob receives the ON_ZERO_CHILDREN signal asynchronously meaning that the blob may still
        // be open here. The dirent cache also keeps the blob open so it needs to be cleared.
        // Waiting for the blob to be removed from the node cache is the best guarantee that the
        // blob is actually closed.
        fixture.volume().volume().dirent_cache().clear();
        async {
            while fixture.volume().volume().cache().get(old_id).is_some() {
                fuchsia_async::Timer::new(std::time::Duration::from_millis(5)).await;
            }
        }
        .on_timeout(std::time::Duration::from_secs(10), || panic!("The old blob is still open"))
        .await;

        {
            let writer =
                fixture.create_blob(&hash.into(), true).await.expect("failed to create blob");
            let vmo = writer
                .get_vmo(delivery_data.len() as u64)
                .await
                .expect("transport error on get_vmo")
                .expect("failed to get vmo");
            write_delivery_blob_all_at_once(&writer, &vmo, &delivery_data).await;

            let new_blob = fixture.get_blob(hash).await.expect("Looking up blob");
            assert_ne!(new_blob.object_id(), old_id);

            fixture.fs().graveyard().flush().await;
            fixture
                .volume()
                .volume()
                .get_or_load_node(old_id, ObjectDescriptor::File, None)
                .await
                .unwrap_err();

            // Blob is still readable.
            assert_eq!(fixture.read_blob(hash).await, data);
        }
        std::mem::drop(blob_dir);
        fixture.close().await;
    }

    #[fuchsia::test(threads = 10)]
    async fn allow_existing_page_in_smoke_test() {
        let fixture = new_blob_fixture().await;

        const SIZE: usize = 65536;
        let data = vec![1; SIZE];

        let hash = fuchsia_merkle::root_from_slice(&data);
        let delivery_data = Type1Blob::generate(&data, CompressionMode::Never);

        // Create the initial blob.
        {
            let writer =
                fixture.create_blob(&hash.into(), false).await.expect("failed to create blob");
            let vmo = writer
                .get_vmo(delivery_data.len() as u64)
                .await
                .expect("transport error on get_vmo")
                .expect("failed to get vmo");
            write_delivery_blob_all_at_once(&writer, &vmo, &delivery_data).await;
        };

        let finished = Arc::new(AtomicBool::new(false));
        let stop_looping = finished.clone();
        let creator_proxy =
            connect_to_protocol_at_dir_svc::<BlobCreatorMarker>(fixture.volume_out_dir())
                .expect("failed to connect to the BlobCreator service");
        let hash_cpy = hash;
        let graveyard = fixture.fs().graveyard().clone();
        // Repeatedly overwrite the blob.
        let overwrite_loop = fasync::Task::spawn(async move {
            let hash = hash_cpy;
            while !stop_looping.load(Ordering::Relaxed) {
                let writer = creator_proxy
                    .create(&hash.into(), true)
                    .await
                    .expect("transport error on create")
                    .unwrap()
                    .into_proxy();
                let vmo = writer
                    .get_vmo(delivery_data.len() as u64)
                    .await
                    .expect("transport error on get_vmo")
                    .expect("failed to get vmo");
                write_delivery_blob_all_at_once(&writer, &vmo, &delivery_data).await;
                // Add barrier and await graveyard so that we don't fill up the disk.
                Epoch::global().barrier().await;
                graveyard.flush().await;
            }
        });

        let mut last_koid = 0;
        let mut reopen_count = 0;
        for _ in 0..1000 {
            // Flush the cache first. This is racy and doesn't always cause the blob to get dropped
            // if it happens during overwrite completion.
            fixture.volume().volume().dirent_cache().clear();

            let child_vmo = fixture.get_blob_vmo(hash).await;
            let koid = child_vmo.info().unwrap().parent_koid.raw_koid();
            if last_koid != koid {
                last_koid = koid;
                reopen_count += 1;
            }
            assert_eq!(child_vmo.read_to_vec::<u8>(0, SIZE as u64).expect("vmo read failed"), data);
        }
        finished.store(true, Ordering::Relaxed);
        overwrite_loop.await;

        // If this never reopens, then we're probably not clearing cache correctly anymore and this
        // test will not be hitting the code paths required. This is technically racy, but it would
        // require overwrite to be as fast or faster than open read and close every one of the
        // thousand iterations.
        assert_ne!(reopen_count, 0);
        fixture.close().await;
    }

    #[fuchsia::test(threads = 10)]
    async fn test_overwrite_with_different_chunk_size() {
        // The merkle verifier and chunks supplied bits can't be cloned from the original blob to
        // the new blob because the chunk size might be different in the new blob.
        //
        // We need to write a blob with one chunk size and then overwrite it with another chunk
        // size. The chunk size of the new blob can't be a multiple of the original blob's chunk
        // size. The delivery_blob crate doesn't provide an easy way of generating a delivery blob
        // for the same content with different chunk sizes.
        //
        // Below, the blob is written with the default chunk size and should have at least 2 chunks.
        // The blob's chunk metadata is then modified to look like there is only 1 chunk. The blob
        // must be reloaded from storage to pick up the metadata changes.
        //
        // The blob is then overwritten using the default chunk size. If `FxBlob::overwrite_me`
        // clones the merkle verifier instead of recreating it then paging in the blob will fail.
        // This is because the blob will try to verify data that isn't a multiple of the original
        // blob's chunk size.
        const BLOB_SIZE: usize = (128 + 16) * 1024;
        let fixture = new_blob_fixture().await;
        let blob_data = vec![0xAA; BLOB_SIZE];
        let hash = fixture.write_blob(&blob_data, CompressionMode::Always).await;
        {
            let blob_object_id =
                fixture.get_blob(hash).await.expect("Failed to get blob").object_id();
            // Make sure the blob isn't in the cache. The blob must be re-opened after modifying the
            // chunk information.
            fixture.volume().volume().dirent_cache().clear();
            assert!(fixture.volume().volume().cache().get(blob_object_id).is_none());

            let blob_object = ObjectStore::open_object(
                fixture.volume().volume(),
                blob_object_id,
                HandleOptions::default(),
                None,
            )
            .await
            .unwrap();
            let mut metadata =
                BlobMetadata::read_from(&blob_object).await.expect("Failed to read blob metadata");
            match &mut metadata.format {
                BlobFormat::Uncompressed => panic!("The blob should be compressed"),
                BlobFormat::ChunkedZstd { chunk_size, compressed_offsets, .. } => {
                    assert!(compressed_offsets.len() > 1);
                    *chunk_size = BLOB_SIZE as u64;
                    compressed_offsets.truncate(1);
                }
                BlobFormat::ChunkedLz4 { .. } => panic!("Expected Zstd"),
            }
            metadata.write_to(&blob_object).await.expect("Failed to write blob metadata");
        };
        // A VMO from the original blob must be held to get `overwrite_me` to be called.
        let old_blob_vmo = fixture.get_blob_vmo(hash).await;

        {
            let writer =
                fixture.create_blob(&hash, true).await.expect("Failed to create BlobWriter");
            let delivery_data = Type1Blob::generate(&blob_data, CompressionMode::Always);
            let vmo = writer
                .get_vmo(delivery_data.len() as u64)
                .await
                .expect("transport error on get_vmo")
                .expect("failed to get vmo");
            write_delivery_blob_all_at_once(&writer, &vmo, &delivery_data).await;
        }

        assert_eq!(fixture.read_blob(hash).await, blob_data);
        std::mem::drop(old_blob_vmo);
        fixture.close().await;
    }

    #[fuchsia::test(threads = 10)]
    async fn test_overwrite_lz4_to_zstd() {
        // There's currently no LZ4 delivery blobs so only LZ4 -> ZSTD can be tested. That's also
        // why make-blob-image is used to generate the LZ4 blob.

        const BLOB_SIZE: usize = (128 + 16) * 1024;
        let device = DeviceHolder::new(FakeDevice::new(16384, 512));
        let blob_data = vec![0xAA; BLOB_SIZE];
        let fxblob_builder = FxBlobBuilder::new(device).await.unwrap();
        let blob = fxblob_builder
            .generate_blob(blob_data.clone(), Some(CompressionAlgorithm::Lz4))
            .unwrap();
        let blob_hash = blob.hash();
        fxblob_builder.install_blob(&blob).await.unwrap();
        let device = fxblob_builder.finalize().await.unwrap().0;
        device.reopen(/*read_only=*/ false);
        let fixture = open_blob_fixture(device).await;
        assert_eq!(fixture.read_blob(blob_hash).await, blob_data);

        // A VMO from the original blob must be held to get `overwrite_me` to be called.
        let old_blob_vmo = fixture.get_blob_vmo(blob_hash).await;

        {
            let writer =
                fixture.create_blob(&blob_hash, true).await.expect("Failed to create BlobWriter");
            let delivery_data = Type1Blob::generate(&blob_data, CompressionMode::Always);
            let vmo = writer
                .get_vmo(delivery_data.len() as u64)
                .await
                .expect("transport error on get_vmo")
                .expect("failed to get vmo");
            write_delivery_blob_all_at_once(&writer, &vmo, &delivery_data).await;
        }

        assert_eq!(fixture.read_blob(blob_hash).await, blob_data);
        std::mem::drop(old_blob_vmo);
        fixture.close().await;
    }

    #[fuchsia::test(threads = 10)]
    async fn test_write_type2_delivery_blob() {
        let fixture = new_blob_fixture().await;

        let mut data = vec![0xAB; 300_000];
        rand::fill(&mut data[..]);

        let hash = fixture
            .write_blob_with_type(
                &data,
                delivery_blob::DeliveryBlobType::Type2,
                CompressionMode::Always,
            )
            .await;

        assert_eq!(fixture.read_blob(hash).await, data);
        fixture.close().await;
    }

    #[fuchsia::test(threads = 10)]
    async fn test_write_type_3_blob_rejected_by_default() {
        let fixture = new_blob_fixture().await;
        let data = vec![1, 2, 3, 4, 5];
        let hash = fuchsia_merkle::root_from_slice(&data);
        let delivery_data = Type3Blob::generate(&data, CompressionMode::Always);

        let writer =
            fixture.create_blob(&hash.into(), false).await.expect("Failed to create BlobWriter");
        let vmo = writer
            .get_vmo(delivery_data.len() as u64)
            .await
            .expect("transport error on get_vmo")
            .expect("failed to get vmo");
        vmo.write(&delivery_data, 0).expect("failed to write to vmo");

        let result = writer.bytes_ready(delivery_data.len() as u64).await.expect("transport error");
        assert!(result.is_err());
        fixture.close().await;
    }

    #[fuchsia::test(threads = 10)]
    async fn test_write_type_3_blob_allowed_when_enabled() {
        let fixture = TestFixture::open(
            DeviceHolder::new(FakeDevice::new(16384, 512)),
            TestFixtureOptions {
                allow_type3_blobs: true,
                encrypted: false,
                as_blob: true,
                ..Default::default()
            },
        )
        .await;
        let data = vec![1, 2, 3, 4, 5];
        let hash = fuchsia_merkle::root_from_slice(&data);
        let delivery_data = Type3Blob::generate(&data, CompressionMode::Always);

        let writer =
            fixture.create_blob(&hash.into(), false).await.expect("Failed to create BlobWriter");
        let vmo = writer
            .get_vmo(delivery_data.len() as u64)
            .await
            .expect("transport error on get_vmo")
            .expect("failed to get vmo");
        write_delivery_blob_all_at_once(&writer, &vmo, &delivery_data).await;

        assert_eq!(fixture.read_blob(hash).await, data);
        fixture.close().await;
    }

    #[fuchsia::test]
    fn test_reject_overflow() {
        let (mut ring_buffer, _vmo) = RingBuffer::new(u64::MAX).unwrap();
        ring_buffer.total_written = u64::MAX - 10;
        let mut buf = Vec::new();
        ring_buffer.read(11, &mut buf).expect_err("Should overflow");
    }
}
