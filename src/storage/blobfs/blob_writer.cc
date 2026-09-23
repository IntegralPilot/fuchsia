// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/blobfs/blob_writer.h"

#include <lib/fit/defer.h>
#include <lib/fpromise/result.h>
#include <lib/sync/completion.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/status.h>
#include <zircon/syscalls.h>
#include <zircon/time.h>
#include <zircon/types.h>

#include <algorithm>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <iterator>
#include <limits>
#include <memory>
#include <optional>
#include <span>
#include <type_traits>
#include <utility>
#include <vector>

#include <fbl/algorithm.h>
#include <fbl/array.h>
#include <fbl/ref_ptr.h>
#include <safemath/checked_math.h>
#include <safemath/safe_conversions.h>

#include "src/lib/chunked-compression/chunked-archive.h"
#include "src/lib/chunked-compression/status.h"
#include "src/storage/blobfs/allocator/extent_reserver.h"
#include "src/storage/blobfs/allocator/node_reserver.h"
#include "src/storage/blobfs/blob.h"
#include "src/storage/blobfs/blob_data_producer.h"
#include "src/storage/blobfs/blob_layout.h"
#include "src/storage/blobfs/blobfs.h"
#include "src/storage/blobfs/common.h"
#include "src/storage/blobfs/compression/external_decompressor.h"
#include "src/storage/blobfs/compression/streaming_chunked_decompressor.h"
#include "src/storage/blobfs/compression_settings.h"
#include "src/storage/blobfs/delivery_blob.h"
#include "src/storage/blobfs/delivery_blob_private.h"
#include "src/storage/blobfs/format.h"
#include "src/storage/blobfs/iterator/block_iterator.h"
#include "src/storage/blobfs/iterator/node_populator.h"
#include "src/storage/blobfs/iterator/vector_extent_iterator.h"
#include "src/storage/blobfs/transaction.h"
#include "src/storage/lib/operation/operation.h"
#include "src/storage/lib/operation/unbuffered_operation.h"
#include "src/storage/lib/trace/trace.h"
#include "src/storage/lib/vfs/cpp/journal/data_streamer.h"
#include "src/storage/lib/vfs/cpp/ticker.h"
#include "src/storage/lib/vfs/cpp/vnode.h"

namespace blobfs {

namespace {

// When performing streaming writes, to ensure block alignment, we must cache data in memory before
// it is streamed into the writeback buffer. The lower this value is, the less memory will be used
// during streaming writes, at the expense of performing more (smaller) unbuffered IO operations.
constexpr size_t kCacheFlushThreshold = 4;
static_assert(kCacheFlushThreshold <= Blobfs::WriteBufferBlockCount(),
              "Number of cached blocks exceeds size of writeback cache.");

// Maximum amount of data which can be kept in memory while decompressing pre-compressed blobs. Must
// be big enough to hold the largest decompressed chunk of a blob but small enough to prevent denial
// of service attacks via memory exhaustion. Arbitrarily set at 256 MiB to match the pager. Chunks
// may not be page aligned, thus maximum memory consumption may be one page more than this amount.
constexpr uint64_t kMaxDecompressionMemoryUsage = 256 * (1ull << 20);

const size_t kSystemPageSize = zx_system_get_page_size();

}  // namespace

Blob::Writer::Writer(const Blob& blob) : blob_(blob) {}

Blob::Writer::~Writer() {
  if (to_overwrite_.has_value()) {
    zx_status_t status = to_overwrite_.value()->ClearOverwritingBy();
    ZX_DEBUG_ASSERT(status == ZX_OK);
    to_overwrite_ = std::nullopt;
  }
}

zx::result<Blob::WrittenBlob> Blob::Writer::WriteNullBlob(Blob& blob) {
  ZX_DEBUG_ASSERT(&blob_ == &blob);

  if (zx::result status = Initialize(/*blob_size*/ 0, /*data_size*/ 0); status.is_error()) {
    return status.take_error();
  }

  // Reserve a node for blob's inode.
  if (zx_status_t status = blobfs().GetAllocator()->ReserveNodes(1, &node_indices_);
      status != ZX_OK) {
    return zx::error(status);
  }
  map_index_ = node_indices_[0].index();

  if (zx::result status = VerifyNullBlob(blobfs(), blob_.digest()); status.is_error()) {
    return status.take_error();
  }

  BlobTransaction transaction;
  if (zx::result status = WriteMetadata(transaction); status.is_error()) {
    return status.take_error();
  }
  auto transaction_completion = [&transaction, &journal = *blobfs().GetJournal(),
                                 &blob]() -> zx::result<> {
    transaction.Commit(journal, {}, [blob = fbl::RefPtr(&blob)]() {});
    return zx::ok();
  };

  if (to_overwrite_.has_value()) {
    if (zx::result result = to_overwrite_.value()->ReplaceBlob(
            transaction, map_index_, blob_layout_->TotalBlockCount(), transaction_completion);
        result.is_error()) {
      return result.take_error();
    }
  } else if (zx::result<> result = transaction_completion(); result.is_error()) {
    return result.take_error();
  }

  return zx::ok(WrittenBlob{.map_index = map_index_, .layout = std::move(blob_layout_)});
}

zx::result<> Blob::Writer::Prepare(Blob& blob, uint64_t data_size) {
  ZX_DEBUG_ASSERT(&blob_ == &blob);
  ZX_DEBUG_ASSERT_MSG(data_size > 0, "Use `WriteNullBlob` if data_size is zero!");

  if (data_size < MetadataType1::kHeaderSize) {
    FX_LOGS(ERROR) << "Size too small for delivery blob!";
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  const uint64_t payload_size = data_size - MetadataType1::kHeaderSize;

  // Fail early if the buffer size will overflow when padding the payload to ensure block alignment.
  if (payload_size % block_size_ != 0) {
    const uint64_t alignment_amount = block_size_ - (payload_size % block_size_);
    if ((std::numeric_limits<uint64_t>::max() - data_size) < alignment_amount) {
      return zx::error(ZX_ERR_OUT_OF_RANGE);
    }
  }

  VmoNameBuffer name = FormatWritingBlobDataVmoName(blob_.digest());
  const uint64_t buffer_size =
      MetadataType1::kHeaderSize + fbl::round_up(payload_size, block_size_);
  if (zx_status_t status = buffer_.CreateAndMap(buffer_size, name.c_str()); status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Failed to create vmo for writing blob " << blob_.digest()
                            << " (vmo size = " << buffer_size << ")";
    return zx::error(status);
  }

  // Reserve a node for blob's inode. We might need more nodes for extents later.
  if (zx_status_t status = blobfs().GetAllocator()->ReserveNodes(1, &node_indices_);
      status != ZX_OK) {
    return zx::error(status);
  }

  data_size_ = data_size;
  map_index_ = node_indices_[0].index();
  streamer_ =
      std::make_unique<fs::DataStreamer>(blobfs().GetJournal(), Blobfs::WriteBufferBlockCount());

  return zx::ok();
}

zx::result<> Blob::Writer::SpaceAllocate() {
  ZX_DEBUG_ASSERT(!allocated_space_);

  TRACE_DURATION("blobfs", "Blob::Writer::SpaceAllocate", "block_count",
                 blob_layout_->TotalBlockCount());

  fs::Ticker ticker;

  std::vector<ReservedExtent> extents;
  std::vector<ReservedNode> nodes;

  // Reserve space for the blob.
  const uint64_t block_count = blob_layout_->TotalBlockCount();
  const uint64_t reserved_blocks = blobfs().GetAllocator()->ReservedBlockCount();
  zx_status_t status = blobfs().GetAllocator()->ReserveBlocks(block_count, &extents);
  if (status == ZX_ERR_NO_SPACE && reserved_blocks > 0) {
    // It's possible that a blob has just been unlinked but has yet to be flushed through the
    // journal, and the blocks are still reserved, so if that looks likely, force a flush and then
    // try again.  This might need to be revisited if/when blobfs becomes multi-threaded.
    sync_completion_t sync;
    blobfs().Sync([&](zx_status_t) { sync_completion_signal(&sync); });
    sync_completion_wait(&sync, ZX_TIME_INFINITE);
    status = blobfs().GetAllocator()->ReserveBlocks(block_count, &extents);
  }
  if (status != ZX_OK) {
    static OutOfSpaceLogSite site;
    if (site.ShouldLog()) {
      FX_PLOGS(ERROR, status) << "Failed to allocate " << blob_layout_->TotalBlockCount()
                              << " blocks for blob";
    }
    return zx::error(status);
  }
  if (extents.size() > kMaxExtentsPerBlob) {
    FX_LOGS(ERROR) << "Error: Block reservation requires too many extents (" << extents.size()
                   << " vs " << kMaxExtentsPerBlob << " max)";
    return zx::error(ZX_ERR_BAD_STATE);
  }

  // Reserve space for all additional nodes necessary to contain this blob. The inode has already
  // been reserved in Blob::Writer::Prepare. Hence, we need to reserve one less node here.
  size_t node_count = NodePopulator::NodeCountForExtents(extents.size()) - 1;
  status = blobfs().GetAllocator()->ReserveNodes(node_count, &nodes);
  if (status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Failed to reserve " << node_count << " nodes for blob";
    return zx::error(status);
  }

  extents_ = std::move(extents);
  node_indices_.insert(node_indices_.end(), std::make_move_iterator(nodes.begin()),
                       std::make_move_iterator(nodes.end()));
  block_iter_ = BlockIterator(std::make_unique<VectorExtentIterator>(extents_));
  allocated_space_ = true;

  blobfs().GetMetrics()->UpdateAllocation(blob_layout_->FileSize(), ticker.End());
  return zx::ok();
}

zx::result<> Blob::Writer::WriteMetadata(BlobTransaction& transaction) {
  TRACE_DURATION("blobfs", "Blob::Writer::WriteMetadata");

  // We utilize the NodePopulator class to take our reserved blocks and nodes and fill the
  // persistent map with an allocated inode / container.

  // If `on_node` is invoked on a node, it means that node was necessary to represent this
  // blob. Persist the node back to durable storage.
  auto on_node = [this, &transaction](uint32_t node_index) {
    blobfs().PersistNode(node_index, transaction);
  };

  // If `on_extent` is invoked on an extent, it was necessary to represent this blob. Persist
  // the allocation of these blocks back to durable storage.
  auto on_extent = [this, &transaction](ReservedExtent& extent) {
    blobfs().PersistBlocks(extent, transaction);
    return NodePopulator::IterationCommand::Continue;
  };

  zx::result mapped_inode_ptr = blobfs().GetNode(map_index_);
  if (mapped_inode_ptr.is_error()) {
    return mapped_inode_ptr.take_error();
  }
  *mapped_inode_ptr.value() = Inode{
      .blob_size = blob_layout_->FileSize(),
      .block_count = safemath::checked_cast<uint32_t>(blob_layout_->TotalBlockCount()),
  };
  blob_.digest().CopyTo(mapped_inode_ptr->merkle_root_hash);
  NodePopulator populator(blobfs().GetAllocator(), std::move(extents_), std::move(node_indices_));
  zx_status_t status = populator.Walk(on_node, on_extent);
  ZX_ASSERT_MSG(status == ZX_OK, "populator.Walk failed with error: %s",
                zx_status_get_string(status));
  // Only set compression flags for a non-null blob.
  if (blob_layout_->FileSize() > 0) {
    SetCompressionAlgorithm(mapped_inode_ptr.value().get(), data_format_);
  }
  SetBlobLayoutFormat(mapped_inode_ptr.value().get(), blob_layout_->Format());
  return zx::ok();
}

zx::result<std::optional<Blob::WrittenBlob>> Blob::Writer::Write(Blob& blob, const void* data,
                                                                 size_t len, size_t* actual) {
  ZX_DEBUG_ASSERT(&blob_ == &blob);
  // Null blobs should be written via `WriteNullBlob()`.
  ZX_DEBUG_ASSERT(data_size_ > 0);
  ZX_DEBUG_ASSERT(len > 0);

  if (actual) {
    *actual = 0;
  }

  // Return a copy of any latched write errors if a previous `Write()` failed.
  if (status_.is_error()) {
    return status().take_error();
  }

  // Perform the actual data write, latching any errors for future retrieval.
  zx::result write_result = WriteInternal(blob, data, len, actual);
  if (write_result.is_error()) {
    status_ = zx::error(write_result.error_value());
  }
  return write_result;
}

zx::result<std::optional<Blob::WrittenBlob>> Blob::Writer::WriteInternal(Blob& blob,
                                                                         const void* data,
                                                                         size_t len,
                                                                         size_t* actual) {
  TRACE_DURATION("blobfs", "Blob::Writer::WriteInternal", "data", data, "len", len);
  // The BlobWriter protocol implementation ensures that the client doesn't send more data than they
  // said they would.
  ZX_ASSERT_MSG(len <= data_size_ - total_written_, "Received more data than expected");

  // Cache the data in the write buffer.
  memcpy(static_cast<uint8_t*>(buffer_.start()) + total_written_, data, len);
  total_written_ += len;

  // Decode the header + metadata.
  //
  // We carefully track how much data we've consumed, as this `Write()` call might include a partial
  // or full header, as well as a partial or full data payload. Parse the header as per RFC 0207.
  if (!header_complete_) {
    if (zx::result status = ParseDeliveryBlob(); status.is_error()) {
      if (status.error_value() == ZX_ERR_BUFFER_TOO_SMALL) {
        // We don't have enough data to decode the header, so wait for more.
        *actual = len;
        return zx::ok(std::nullopt);
      }
      FX_PLOGS(ERROR, status.status_value()) << "Failed to parse delivery blob";
      return status.take_error();
    }
    ZX_DEBUG_ASSERT(header_complete_);

    // Special case: If this is the null blob, finish the write immediately.
    if (metadata_.payload_length == 0) {
      *actual = len;
      return WriteNullBlob(blob);
    }

    // If the blob is uncompressed, we can initialize the blob layout/Merkle tree buffers as we
    // know how large the blob is based on the payload length.
    if (!metadata_.IsCompressed()) {
      if (zx::result status = Initialize(/*blob_size*/ metadata_.payload_length,
                                         /*data_size*/ metadata_.payload_length);
          status.is_error()) {
        return status.take_error();
      }
    }
  }
  ZX_DEBUG_ASSERT(header_complete_);

  // If blob is pre-compressed, prepare the decompressor to calculate the Merkle tree.
  if (metadata_.IsCompressed() && !streaming_decompressor_) {
    if (zx::result status = InitializeDecompressor(); status.is_error()) {
      if (status.error_value() == ZX_ERR_BUFFER_TOO_SMALL) {
        *actual = len;
        return zx::ok(std::nullopt);  // Not enough data for seek table, wait for more.
      }
      return status.take_error();
    }
    // Special case: If the archive is empty (i.e. this is the null blob), skip the write phase.
    if (seek_table_.DecompressedSize() == 0) {
      *actual = len;
      return WriteNullBlob(blob);
    }
  }

  ZX_DEBUG_ASSERT(blob_layout_);
  ZX_DEBUG_ASSERT(payload_written() >= payload_processed_);

  // Update the Merkle tree with the incoming data. If the blob is pre-compressed, we use the
  // decompressor to update the Merkle tree via a callback, otherwise we update it directly.
  if (streaming_decompressor_) {
    // Update the decompressor with the data we got since the last write.
    zx::result status = streaming_decompressor_->Update(
        {payload() + payload_processed_, payload_written() - payload_processed_});
    if (status.is_error()) {
      FX_PLOGS(ERROR, status.status_value()) << "Failed to decompress blob data";
      return status.take_error();
    }
  } else {
    // We have to update the Merkle tree before calling `StreamBufferedData()` otherwise we may
    // decommit pages from `payload()` causing an incorrect digest.
    const uint8_t* buff = payload() + payload_processed_;
    if (zx_status_t status =
            merkle_tree_creator_.Append(buff, payload_written() - payload_processed_);
        status != ZX_OK) {
      FX_PLOGS(ERROR, status) << "MerkleTreeCreator::Append failed";
      return zx::error(status);
    }
  }

  // If we're doing streaming writes, try to persist all the data we have buffered so far.
  if (streaming_write_) {
    // Stream buffered data to disk.
    if (zx::result status = StreamBufferedData(); status.is_error()) {
      if (status.status_value() == ZX_ERR_NO_SPACE) {
        static OutOfSpaceLogSite site;
        if (site.ShouldLog())
          FX_PLOGS(ERROR, status.status_value()) << "Failed to perform streaming write";
      } else {
        FX_PLOGS(ERROR, status.status_value()) << "Failed to perform streaming write";
      }
      return status.take_error();
    }
  }

  payload_processed_ = payload_written();

  // More data to write.
  if (total_written_ < data_size_) {
    *actual = len;
    return zx::ok(std::nullopt);
  }

  if (zx::result status = Commit(blob); status.is_error()) {
    return status.take_error();
  }

  *actual = len;
  return zx::ok(WrittenBlob{.map_index = map_index_, .layout = std::move(blob_layout_)});
}

zx::result<> Blob::Writer::Commit(Blob& blob) {
  if (blob_.digest() != digest_) {
    FX_LOGS(ERROR) << "downloaded blob did not match provided digest " << blob_.digest();
    return zx::error(ZX_ERR_IO_DATA_INTEGRITY);
  }

  fs::Duration generation_time;

  // For non-streaming writes, we lazily allocate space.
  if (!allocated_space_) {
    if (zx::result status = SpaceAllocate(); status.is_error()) {
      return status.take_error();
    }
  }

  // There are several situations to consider to finish writing out the blob.
  //  - If the blob is larger than 8KiB then the merkle tree won't be empty and needs to be written
  //    out.
  //    - If the blob is being stored in the deprecated padded format then the merkle tree is stored
  //      before the blob and is block aligned. All of the data will have already been written out
  //      so only the merkle tree needs to be written.
  //    - If the blob is being stored in the compact format then the merkle tree is stored at the
  //      end of the blob and might share the last block of the blob. All of the whole data blocks
  //      may not have been written yet either.
  //  - If the blob is less than 8KiB and was compressed in the delivery blob then the uncompressed
  //    blob should be written out of the |decompressed_data_| buffer.

  const size_t merkle_size = merkle_tree_creator_.GetTreeLength();
  std::span<uint8_t> merkle_data(merkle_tree(), merkle_size);
  uint64_t block_count = 0;
  if (!decompressed_data_.empty()) {
    // The |decompressed_data_| is only used when the blob is less than or equal to 8KiB which also
    // means that there is no merkle tree.
    ZX_ASSERT(merkle_data.empty());
    ZX_ASSERT(blob_layout_->TotalBlockCount() == 1);
    SimpleBlobDataProducer data_producer(decompressed_data_);
    block_count = 1;
    if (zx::result status = WriteDataBlocks(block_count, /*block_offset=*/0, data_producer);
        status.is_error()) {
      return status.take_error();
    }
  } else if (blob_layout_->Format() == BlobLayoutFormat::kDeprecatedPaddedMerkleTreeAtStart) {
    // |StreamBufferedData| writes out all data blocks when the padded format is used. Only the
    // merkle tree needs to be written at the start of the blob. The merkle tree is always a
    // multiple of the |kBlobfsBlockSize| in the deprecated padded format.
    ZX_ASSERT(payload_persisted_ == payload_length());
    ZX_ASSERT(merkle_size % block_size_ == 0);
    if (!merkle_data.empty()) {
      SimpleBlobDataProducer merkle_producer(merkle_data);
      block_count = blob_layout_->MerkleTreeBlockCount();
      if (zx::result status =
              WriteDataBlocks(block_count, blob_layout_->MerkleTreeBlockOffset(), merkle_producer);
          status.is_error()) {
        return status.take_error();
      }
    }
  } else {
    // The end of the payload needs to be merged with the merkle tree.
    const size_t padding =
        safemath::CheckSub(blob_layout_->MerkleTreeOffset(), blob_layout_->DataSizeUpperBound())
            .ValueOrDie();
    MergeBlobDataProducer producer(
        std::span(payload() + payload_persisted_, payload_length() - payload_persisted_), padding,
        merkle_data);
    uint64_t block_offset = payload_persisted_ / block_size_;
    block_count = blob_layout_->TotalBlockCount() - block_offset;
    if (block_count > 0) {
      if (zx::result status = WriteDataBlocks(block_count, block_offset, producer);
          status.is_error()) {
        return status.take_error();
      }
    }
  }

  // No more data to write. Flush data to disk and commit metadata.
  fs::Ticker ticker;  // Tracking enqueue time.

  if (zx::result status = FlushData(blob); status.is_error()) {
    return status.take_error();
  }

  blobfs().GetMetrics()->UpdateClientWrite(block_count * block_size_, merkle_size, ticker.End(),
                                           generation_time);
  blobfs().GetMetrics()->IncrementBlobLayoutCount(blob_layout_->Format());
  return zx::ok();
}

zx::result<> Blob::Writer::FlushData(Blob& blob) {
  // Enqueue the blob's final data work. Metadata must be enqueued separately.
  zx_status_t data_status = ZX_ERR_IO;
  sync_completion_t data_written;
  // Issue the signal when the callback is destroyed rather than in the callback because the
  // callback won't get called in some error paths.
  auto data_written_finished = fit::defer([&] { sync_completion_signal(&data_written); });
  auto write_all_data = streamer_->Flush().then(
      [&data_status, data_written_finished = std::move(data_written_finished)](
          const fpromise::result<void, zx_status_t>& result) {
        data_status = result.is_ok() ? ZX_OK : result.error();
        return result;
      });

  // Discard things we don't need any more. This has to be after the Flush call above to ensure
  // all data has been copied from these buffers.
  buffer_.Reset();
  merkle_tree_buffer_.reset();
  streaming_decompressor_.reset();
  decompressed_data_.reset();

  // FreePagedVmo() will return the reference that keeps this object alive on behalf of the paging
  // system so we can free it outside the lock. However, when a Blob is being written it can't be
  // mapped so we know there should be no pager reference. Otherwise, calling FreePagedVmo() will
  // make future uses of the mapped data go invalid.
  //
  // If in the future we need to support memory mapping a paged VMO (like we allow mapping and using
  // the portions of a blob that are already known), then this code will have to be changed to not
  // free the VMO here (which will in turn require other changes).
  fbl::RefPtr<fs::Vnode> pager_reference = blob.FreePagedVmo();
  ZX_DEBUG_ASSERT(!pager_reference);

  // Wrap all pending writes with a strong reference to this Blob, so that it stays
  // alive while there are writes in progress acting on it.
  BlobTransaction transaction;
  if (zx::result status = WriteMetadata(transaction); status.is_error()) {
    return status.take_error();
  }

  auto completion = [&journal = *blobfs().GetJournal(), &write_all_data, &transaction, &blob,
                     &data_written, &data_status]() -> zx::result<> {
    transaction.Commit(journal, std::move(write_all_data), [self = fbl::RefPtr(&blob)]() {});

    // It's not safe to continue until all data has been written because we might need to reload it
    // (e.g. if the blob is immediately read after writing), and the journal caches data in ring
    // buffers, so wait until that has happened.  We don't need to wait for the metadata because we
    // cache that.
    sync_completion_wait(&data_written, ZX_TIME_INFINITE);
    if (data_status != ZX_OK) {
      return zx::error(data_status);
    }
    return zx::ok();
  };

  if (to_overwrite_.has_value()) {
    if (zx::result<> result = to_overwrite_.value()->ReplaceBlob(
            transaction, map_index_, blob_layout_->TotalBlockCount(), completion);
        result.is_error()) {
      return result.take_error();
    }
  } else {
    return completion();
  }
  return zx::ok();
}

zx::result<> Blob::Writer::WriteDataBlocks(uint64_t block_count, uint64_t block_offset,
                                           BlobDataProducer& producer) {
  if (zx_status_t status = IterateToBlock(&block_iter_, block_offset); status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Failed to iterate to block offset " << block_offset;
    return zx::error(status);
  }
  const uint64_t data_start = DataStartBlock(blobfs().Info());
  zx_status_t status = StreamBlocks(
      &block_iter_, block_count,
      [&](uint64_t vmo_offset, uint64_t dev_offset, uint64_t block_count) {
        while (block_count) {
          auto data = producer.Consume(block_count * block_size_);
          ZX_ASSERT_MSG(!data.empty(), "Data span for writing should not be empty.");
          storage::UnbufferedOperation op = {.data = data.data(),
                                             .op = {
                                                 .type = storage::OperationType::kWrite,
                                                 .dev_offset = dev_offset + data_start,
                                                 .length = data.size() / block_size_,
                                             }};
          block_count -= op.op.length;
          dev_offset += op.op.length;
          streamer_->StreamData(std::move(op));
        }  // while (block_count)
        return ZX_OK;
      });
  if (status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok();
}

zx::result<> Blob::Writer::Initialize(uint64_t blob_size, uint64_t data_size) {
  zx::result blob_layout =
      BlobLayout::CreateFromSizes(blobfs().BlobWriteFormat(), blob_size, data_size, block_size_);
  if (blob_layout.is_error()) {
    FX_PLOGS(ERROR, blob_layout.status_value()) << "Failed to create blob layout";
    return blob_layout.take_error();
  }

  if (blob_size > 0 && merkle_tree_buffer_.empty()) {
    merkle_tree_creator_.SetUseCompactFormat(
        ShouldUseCompactMerkleTreeFormat(blob_layout->Format()));
    zx_status_t status = merkle_tree_creator_.SetDataLength(blob_size);
    if (status != ZX_OK) {
      FX_PLOGS(ERROR, status) << "Failed to set Merkle tree data length to " << blob_size
                              << " bytes";
      return zx::error(status);
    }
    const size_t tree_len = merkle_tree_creator_.GetTreeLength();
    // Allow for other data before the tree. In the compact merkle tree format, the merkle tree may
    // not be a multiple of the block size and is aligned to the end of a block. Blob data and zeros
    // may be added at the start of the first merkle tree to achieve this alignment.
    merkle_tree_buffer_ = fbl::MakeArray<uint8_t>(tree_len + block_size_);
    status = merkle_tree_creator_.SetTree(merkle_tree(), tree_len, &digest_, sizeof digest_);
    if (status != ZX_OK) {
      FX_PLOGS(ERROR, status) << "Failed to set Merkle tree data length to " << blob_size
                              << " bytes";
      return zx::error(status);
    }
  }

  ZX_DEBUG_ASSERT(blob_layout->DataSizeUpperBound() == data_size);
  ZX_DEBUG_ASSERT(blob_layout->FileSize() == blob_size);
  blob_layout_ = std::move(blob_layout).value();
  return zx::ok();
}

zx::result<> Blob::Writer::StreamBufferedData() {
  if (!allocated_space_) {
    if (zx::result status = SpaceAllocate(); status.is_error()) {
      return status.take_error();
    }
  }

  // Write as many block-aligned bytes from `payload()` to disk as we can.
  ZX_DEBUG_ASSERT(payload_written() >= payload_persisted_);
  const uint64_t amount_buffered = payload_written() - payload_persisted_;
  if (amount_buffered >= (kCacheFlushThreshold * block_size_)) {
    const uint64_t write_amount = fbl::round_down(amount_buffered, block_size_);
    const uint64_t start_block = blob_layout_->DataBlockOffset() + payload_persisted_ / block_size_;
    SimpleBlobDataProducer data({payload() + payload_persisted_, write_amount});
    if (zx::result status = WriteDataBlocks(write_amount / block_size_, start_block, data);
        status.is_error()) {
      FX_PLOGS(ERROR, status.status_value()) << "Failed to stream blob data to disk";
      return status;
    }
    // Ensure data is copied into writeback cache so we can decommit those pages from the buffer.
    streamer_->IssueOperations();
    payload_persisted_ += write_amount;
    // Decommit now unused pages from the buffer.
    const uint64_t page_aligned_offset =
        fbl::round_down(header_.header_length + payload_persisted_, kSystemPageSize);
    if (zx_status_t status = zx_vmo_op_range(buffer_.vmo().get(), ZX_VMO_OP_DECOMMIT, 0,
                                             page_aligned_offset, nullptr, 0);
        status != ZX_OK) {
      return zx::error(status);
    }
  }
  ZX_DEBUG_ASSERT(payload_persisted_ % block_size_ == 0);

  // To simplify the Commit logic when using the deprecated format (Merkle tree at beginning), if we
  // received all data for the blob, enqueue the remaining data so we only have the Merkle tree left
  // to write to disk. This ensures Commit only has to deal with contiguous chunks of data.
  if (payload_written() == payload_length() &&
      blob_layout_->Format() == BlobLayoutFormat::kDeprecatedPaddedMerkleTreeAtStart) {
    if (zx::result status = WriteRemainingDataForDeprecatedFormat(); status.is_error()) {
      return status.take_error();
    }
  }
  return zx::ok();
}

zx::result<> Blob::Writer::WriteRemainingDataForDeprecatedFormat() {
  ZX_DEBUG_ASSERT(blob_layout_->Format() == BlobLayoutFormat::kDeprecatedPaddedMerkleTreeAtStart);
  ZX_DEBUG_ASSERT(payload_persisted_ % block_size_ == 0);

  if (payload_persisted_ < payload_length()) {
    const size_t remaining = payload_length() - payload_persisted_;
    const size_t remaining_aligned = fbl::round_up(remaining, block_size_);
    const uint64_t block_count = remaining_aligned / block_size_;
    const uint64_t block_offset =
        blob_layout_->DataBlockOffset() + (payload_persisted_ / block_size_);
    // The data buffer is already padded to ensure it's a multiple of the block size.
    SimpleBlobDataProducer data({payload() + payload_persisted_, remaining_aligned});
    if (zx::result status = WriteDataBlocks(block_count, block_offset, data); status.is_error()) {
      FX_PLOGS(ERROR, status.status_value()) << "Failed to write final block to disk";
      return status.take_error();
    }
    payload_persisted_ += remaining;
  }
  ZX_DEBUG_ASSERT(payload_persisted_ == payload_length());

  // We've now persisted all of the blob's data to disk. The only remaining thing to write out is
  // the Merkle tree, which is at the first block, so we need to reset the block iterator before
  // writing any more data to disk.
  block_iter_ = BlockIterator(std::make_unique<VectorExtentIterator>(extents_));
  return zx::ok();
}

zx::result<> Blob::Writer::InitializeDecompressor() {
  ZX_DEBUG_ASSERT(!streaming_decompressor_);
  ZX_DEBUG_ASSERT(metadata_.payload_length > 0);  // Null blobs should skip the normal write path.
  ZX_DEBUG_ASSERT(metadata_.IsCompressed());
  ZX_DEBUG_ASSERT(data_format_ == CompressionAlgorithm::kChunked);

  // Try to load the seek table and initialize the decompressor.
  chunked_compression::HeaderReader reader;
  // We validate the maximum chunk size below to prevent any potential memory exhaustion.
  const chunked_compression::Status status =
      reader.Parse(payload(), payload_written(), payload_length(), &seek_table_);
  if (status != chunked_compression::kStatusOk) {
    return zx::error(chunked_compression::ToZxStatus(status));
  }

  // The chunked compression library is responsible for ensuring that the seek table is
  // consistent (i.e. chunks don't overlap, the last chunk matches decompressed size, etc...).
  // We also perform a consistency check against the payload size reported in the metadata.
  if (seek_table_.CompressedSize() != metadata_.payload_length) {
    FX_LOGS(ERROR) << "Seek table compressed size (" << seek_table_.CompressedSize()
                   << ") does not match payload length from metadata (" << metadata_.payload_length
                   << ")!";
    return zx::error(ZX_ERR_IO_DATA_INTEGRITY);
  }
  if (seek_table_.Entries().empty()) {
    return zx::ok();  // Archive is empty, no decompression is required.
  }

  // The StreamingChunkedDecompressor decommits chunks as they are decompressed, so we just need to
  // ensure the maximum decompressed chunk size does not exceed our set upper bound.
  using chunked_compression::SeekTableEntry;
  const size_t largest_decompressed_size =
      std::max_element(seek_table_.Entries().begin(), seek_table_.Entries().end(),
                       [](const SeekTableEntry& a, const SeekTableEntry& b) {
                         return a.decompressed_size < b.decompressed_size;
                       })
          ->decompressed_size;
  if (largest_decompressed_size > kMaxDecompressionMemoryUsage) {
    FX_LOGS(ERROR) << "Largest seek table entry (decompressed size = " << largest_decompressed_size
                   << ") exceeds set memory consumption limit (" << kMaxDecompressionMemoryUsage
                   << ")!";
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  // Special case: decompress the blob data before persisting it to disk if compression does not
  // save on-disk space.
  const bool write_uncompressed =
      (seek_table_.DecompressedSize() <= kCompressionSizeThresholdBytes);
  if (write_uncompressed) {
    streaming_write_ = false;
    // The buffer must be block aligned for writing to disk.
    decompressed_data_ =
        fbl::MakeArray<uint8_t>(fbl::round_up(seek_table_.DecompressedSize(), block_size_));
    memset(decompressed_data_.data(), 0, decompressed_data_.size());
    data_format_ = CompressionAlgorithm::kUncompressed;
  }
  // Initialize blob layout and Merkle tree buffers now that we know the amount of data to persist.
  if (zx::result status =
          Initialize(/*blob_size*/ seek_table_.DecompressedSize(),
                     /*data_size*/ write_uncompressed ? seek_table_.DecompressedSize()
                                                      : seek_table_.CompressedSize());
      status.is_error()) {
    return status.take_error();
  }

  // TODO(https://fxbug.dev/42179006): Offline compression *requires* an external sandboxed
  // decompressor, but not all targets currently enable this option. For now, we fall back to the
  // same service connector that Blobfs would attempt to use if the option was enabled but a
  // specific sandbox service was not specified.
  DecompressorCreatorConnector& connector =
      blobfs().decompression_connector() ? *blobfs().decompression_connector()
                                         : DecompressorCreatorConnector::DefaultServiceConnector();
  zx::result streaming_decompressor = StreamingChunkedDecompressor::Create(
      connector, seek_table_,
      [this, write_uncompressed,
       last_offset = size_t{0}](std::span<const uint8_t> data) mutable -> zx::result<> {
        if (zx_status_t status = merkle_tree_creator_.Append(data.data(), data.size());
            status != ZX_OK) {
          FX_PLOGS(ERROR, status) << "MerkleTreeCreator::Append failed";
          return zx::error(status);
        }
        if (write_uncompressed) {
          ZX_DEBUG_ASSERT(last_offset + data.size() <= decompressed_data_.size());
          std::memcpy(decompressed_data_.data() + last_offset, data.data(), data.size());
          last_offset += data.size();
        }
        return zx::ok();
      });
  if (streaming_decompressor.is_error()) {
    return zx::error(streaming_decompressor.error_value());
  }

  streaming_decompressor_ = std::move(streaming_decompressor).value();
  return zx::ok();
}

zx::result<> Blob::Writer::ParseDeliveryBlob() {
  std::span<const uint8_t> data = {static_cast<const uint8_t*>(buffer_.start()), total_written_};
  // Try to decode the header.
  zx::result<DeliveryBlobHeader> header = DeliveryBlobHeader::FromBuffer(data);
  if (header.is_error()) {
    return header.take_error();
  }
  if (header->type != DeliveryBlobType::kType1 && header->type != DeliveryBlobType::kType2) {
    FX_LOGS(ERROR) << "Unsupported delivery blob type: "
                   << static_cast<std::underlying_type_t<DeliveryBlobType>>(header->type);
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }
  if (header->header_length != MetadataType1::kHeaderSize) {
    FX_LOGS(ERROR) << "Invalid header length for type 1 or 2 blob: actual = "
                   << header->header_length << ", expected = " << MetadataType1::kHeaderSize;
    return zx::error(ZX_ERR_IO_DATA_INTEGRITY);
  }
  // Try to decode the metadata.
  zx::result<MetadataType1> metadata =
      MetadataType1::FromBuffer(data.subspan(sizeof(DeliveryBlobHeader)), *header);
  if (metadata.is_error()) {
    return metadata.take_error();
  }

  // We currently require a call to `Blob::Truncate()` / `Blob::Writer::Prepare()` with the correct
  // data size.
  const uint64_t expected_data_size = header->header_length + metadata->payload_length;
  if (data_size_ != expected_data_size) {
    FX_LOGS(ERROR) << "Delivery blob length mismatch: actual = " << data_size_
                   << ", expected = " << expected_data_size;
  }

  header_complete_ = true;
  header_ = *header;
  metadata_ = *metadata;
  data_format_ = metadata_.IsCompressed() ? CompressionAlgorithm::kChunked
                                          : CompressionAlgorithm::kUncompressed;
  return zx::ok();
}

void Blob::Writer::SetBlobToOverwrite(fbl::RefPtr<Blob> to_overwrite) {
  to_overwrite_ = std::move(to_overwrite);
}

}  // namespace blobfs
