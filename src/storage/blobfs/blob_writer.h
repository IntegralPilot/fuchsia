// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_BLOBFS_BLOB_WRITER_H_
#define SRC_STORAGE_BLOBFS_BLOB_WRITER_H_

#ifndef __Fuchsia__
#error Fuchsia-only Header
#endif

#include <lib/fzl/owned-vmo-mapper.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>
#include <zircon/compiler.h>

#include <cstddef>
#include <cstdint>
#include <memory>
#include <optional>
#include <vector>

#include <fbl/array.h>
#include <fbl/ref_ptr.h>
#include <safemath/checked_math.h>

#include "src/lib/chunked-compression/chunked-archive.h"
#include "src/lib/digest/digest.h"
#include "src/lib/digest/merkle-tree.h"
#include "src/storage/blobfs/allocator/extent_reserver.h"
#include "src/storage/blobfs/allocator/node_reserver.h"
#include "src/storage/blobfs/blob.h"
#include "src/storage/blobfs/blob_data_producer.h"
#include "src/storage/blobfs/blob_layout.h"
#include "src/storage/blobfs/blobfs.h"
#include "src/storage/blobfs/compression/streaming_chunked_decompressor.h"
#include "src/storage/blobfs/compression_settings.h"
#include "src/storage/blobfs/delivery_blob.h"
#include "src/storage/blobfs/delivery_blob_private.h"
#include "src/storage/blobfs/iterator/block_iterator.h"
#include "src/storage/blobfs/transaction.h"
#include "src/storage/lib/vfs/cpp/journal/data_streamer.h"

namespace blobfs {

// Data used exclusively during writeback. Upon failures of any public methods, the blob should be
// moved into an error state and removed from the cache so that a client may attempt another write.
class Blob::Writer {
 public:
  // Construct a new blob writer. `blob` must outlive this object. Data being written must conform
  // to the format specified by RFC 0207.
  explicit Writer(const Blob& blob);

  // Not copyable or movable because merkle_tree_creator_ has a pointer to digest.
  Writer(const Writer&) = delete;
  Writer& operator=(const Writer&) = delete;

  ~Writer();

  // Prepare `blob` and this writer to accept `data_size` bytes of data. If this is a null blob,
  // `WriteNullBlob()` should be called instead. `blob` should be the same object this writer was
  // created using.
  //
  // *WARNING*: Upon failures of this function the associated blob should be evicted from the cache.
  // The failure reason can be obtained again by calling `status()`.
  zx::result<> Prepare(Blob& blob, uint64_t data_size);

  // Write blob data into memory (or stream to disk) and update Merkle tree. When all data has been
  // written, a transaction is scheduled, and returns information to move `blob` into a readable
  // state. `blob` should be the same object this writer was created using.
  //
  // *WARNING*: Upon failures of this function the associated blob should be evicted from the cache.
  // The failure reason can be obtained again by calling `status()`.
  zx::result<std::optional<WrittenBlob>> Write(Blob& blob, const void* data, size_t len,
                                               size_t* actual) __TA_REQUIRES(blob.mutex_);

  // Writes out a null blob and returns information to move `blob` into a readable state. This
  // skips the normal write path. The digest of the associated `blob` is verified before any
  // transactions are scheduled. `blob` should be the same object this writer was created using.
  //
  // *WARNING*: Upon failures of this function the associated blob should be evicted from the cache.
  // The failure reason can be obtained again by calling `status()`.
  zx::result<WrittenBlob> WriteNullBlob(Blob& blob);

  // Set the status of this writer. Used by the parent blob to store any errors when moving the blob
  // into a readable state.
  void set_status(zx::result<> status) { status_ = status; }

  // The fused write error. Once writing has failed, we return the same error on subsequent writes
  // in case a higher layer dropped the error and returned a short write instead.
  zx::result<> status() const { return status_; }

  // Amount of data written into this writer so far.
  uint64_t total_written() const { return total_written_; }

  // Saves a reference to a blob being overwritten.
  void SetBlobToOverwrite(fbl::RefPtr<Blob> to_overwrite);

 private:
  // Initialize `blob_layout_` and Merkle tree buffers, if required, using specified `blob_size`
  // and current `data_size`. Merkle tree buffers will only be initialized on first call such that
  // the blob layout can be recalculated without affecting the existing Merkle tree.
  zx::result<> Initialize(uint64_t blob_size, uint64_t data_size);

  // Initializes the seek table and decompressor. If not enough data is buffered to decode the seek
  // table, returns ZX_ERR_BUFFER_TOO_SMALL.
  zx::result<> InitializeDecompressor();

  // If successful, allocates Blob Node and Blocks (in-memory) and sets `allocated_space_` to true.
  // Should not be called if `allocated_space_` is true.
  zx::result<> SpaceAllocate();

  // Handles writing blob data to disk or caching in memory.
  zx::result<std::optional<WrittenBlob>> WriteInternal(Blob& blob, const void* data, size_t len,
                                                       size_t* actual) __TA_REQUIRES(blob.mutex_);

  // Stream as much outstanding data as possible when performing streaming writes.
  zx::result<> StreamBufferedData();

  // Write `block_count` blocks at `block_offset` using the data from `producer` into `streamer`.
  zx::result<> WriteDataBlocks(uint64_t block_count, uint64_t block_offset,
                               BlobDataProducer& producer);

  // Called by the Vnode once the last write has completed, updating the on-disk metadata.
  zx::result<> WriteMetadata(BlobTransaction& transaction);

  // When using deprecated blob format with Merkle tree at beginning, ensure outstanding data writes
  // have been issued, and reset block iterator. This ensures only the Merkle tree is outstanding.
  zx::result<> WriteRemainingDataForDeprecatedFormat();

  // Commits (remaining) blob data and the Merkle tree to persistent storage. The blob can be marked
  // as readable once this succeeds.
  zx::result<> Commit(Blob& blob) __TA_REQUIRES(blob.mutex_);

  // Flush blob data to persistent storage, and issue commit of metadata. Only blob data is
  // guaranteed to persist on return. Metadata may still remain in cache until next journal flush.
  zx::result<> FlushData(Blob& blob) __TA_REQUIRES(blob.mutex_);

  // Helper function to decode `header_` and `metadata_` from buffered data.
  zx::result<> ParseDeliveryBlob();

  // Filesystem backing this blob.
  Blobfs& blobfs() const { return blob_.blobfs_; }

  // Pointer to the beginning of the Merkle tree. As the tree may overlap with some of the blob's
  // data, there will always be at least `block_size_` bytes of padding before the returned pointer.
  // Can only be called after `Initialize()` has succeeded.
  uint8_t* merkle_tree() const {
    ZX_DEBUG_ASSERT(merkle_tree_buffer_);
    return merkle_tree_buffer_.get() + block_size_;
  }

  // Pointer within `buffer` where the actual data payload to be persisted on disk starts.
  uint8_t* payload() const {
    return static_cast<uint8_t*>(buffer_.start()) + header_.header_length;
  }

  // Size of the payload to be persisted on disk.
  size_t payload_length() const { return metadata_.payload_length; }

  // Amount of the payload written into `Write()` so far.
  size_t payload_written() const {
    return safemath::CheckSub(total_written_, header_.header_length).ValueOrDie();
  }

  // Blob we're writing. Must outlive this object and be the same blob passed to any public methods.
  //
  // *NOTE*: As the shared state of the blob is guarded by mutexes, any modifications to the blob's
  // state are done by passing an explicit Blob& reference. This improves readability by making it
  // apparent which call sites may modify the Blob's state, and is required in order to statically
  // verify that the caller is holding the required locks.
  const Blob& blob_;

  // Block size being used by the Blobfs instance that this blob is being written to.
  const uint64_t block_size_ = blobfs().Info().block_size;

  // Format of the data being written/persisted to disk.
  CompressionAlgorithm data_format_ = {};

  // Mapped VMO which we write incoming data into. When performing streaming writes, we decommit
  // pages which are no longer required to save memory (see kCacheFlushThreshold).
  fzl::OwnedVmoMapper buffer_;

  // Helper to stream data blocks to disk.
  std::unique_ptr<fs::DataStreamer> streamer_ = nullptr;

  // The extents we reserved for this blob from the filesystem's allocator.
  std::vector<ReservedExtent> extents_;

  // The nodes we reserved for this blob from the filesystem's allocator.
  std::vector<ReservedNode> node_indices_;

  // If true, indicates we are streaming the current blob to disk as written. Streaming is only
  // disabled when storing a pre-compressed delivery blob uncompressed. This is only done for blobs
  // that are smaller than a single block.
  bool streaming_write_ = true;

  // Block iterator used when writing data to disk.
  //
  // *NOTE*: When writing the deprecated blob format, this may not write all blocks in order (i.e.
  // we may write the Merkle tree at the beginning after the data has been written).
  BlockIterator block_iter_ = BlockIterator{nullptr};

  // Total amount of data in bytes we expect to be written for this blob, *including* headers.
  uint64_t data_size_ = 0;

  // The inode number we reserved for this blob.
  uint32_t map_index_ = 0;

  // As data is written, we build the merkle tree using the tree creator.
  digest::MerkleTreeCreator merkle_tree_creator_ = {};

  // The merkle tree creator stores the root digest here.
  uint8_t digest_[digest::kSha256Length] = {};

  // The merkle tree creator stores the rest of the tree here. The buffer must include at least one
  // block's worth of space for padding (see `merkle_tree()` for details).
  fbl::Array<uint8_t> merkle_tree_buffer_;

  // On-disk layout of the blob itself. Can only be initialized once we know the actual size of the
  // blob (i.e. the blob is compressed).
  std::unique_ptr<BlobLayout> blob_layout_ = nullptr;

  // Bytes of data written into this writer via `Write()` so far, including headers.
  // We expect that when `total_written_ == `data_size_` this blob is complete.
  uint64_t total_written_ = 0;

  // Amount of the payload that has been processed so far.
  uint64_t payload_processed_ = 0;

  // Bytes of data persisted to disk thus far (always <= `total_written_`). Will always be 0 if
  // streaming writes has been disabled.
  uint64_t payload_persisted_ = 0;

  // True once `SpaceAllocate()` has been called and has succeeded.
  bool allocated_space_ = false;

  // Seek table of the incoming data if `data_format_` is CompressionAlgorithm::kChunked.
  chunked_compression::SeekTable seek_table_;

  // Decompressor to use on incoming data if `data_format_` is CompressionAlgorithm::kChunked.
  std::unique_ptr<StreamingChunkedDecompressor> streaming_decompressor_ = nullptr;

  // Status of the writer, used to latch any write errors. See `error()` for details.
  zx::result<> status_ = zx::ok();

  // Delivery Blob (RFC 0207) Related Fields

  // True if `header_` and `metadata_` have been decoded, false otherwise.
  bool header_complete_ = false;

  // Delivery blob header as specified by RFC 0207.
  DeliveryBlobHeader header_ = {};

  // Delivery blob metadata (currently only type 1 blobs are supported).
  MetadataType1 metadata_ = {};

  // Buffer to store decompressed data in when we will not save any space persisting compressed data
  // to disk (i.e. the decompressed size is smaller than one block). Must be block aligned.
  fbl::Array<uint8_t> decompressed_data_;

  // The blob that should be overwritten. When this object is destroyed it should go to this Blob
  // to mark that it is no longer being overwritten, either due to successful overwrite, or failure.
  std::optional<fbl::RefPtr<Blob>> to_overwrite_ = std::nullopt;
};

}  // namespace blobfs

#endif  // SRC_STORAGE_BLOBFS_BLOB_WRITER_H_
