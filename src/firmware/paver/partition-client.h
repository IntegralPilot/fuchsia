// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_FIRMWARE_PAVER_PARTITION_CLIENT_H_
#define SRC_FIRMWARE_PAVER_PARTITION_CLIENT_H_

#include <fidl/fuchsia.device/cpp/wire.h>
#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <lib/zx/channel.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <cstddef>
#include <memory>
#include <optional>
#include <vector>

#include <fbl/auto_lock.h>
#include <fbl/mutex.h>
#include <fbl/unique_fd.h>

#include "lib/fidl/cpp/wire/internal/transport_channel.h"
#include "src/firmware/paver/block-devices.h"
#include "src/lib/uuid/uuid.h"
#include "src/storage/lib/block_client/cpp/client.h"
#include "src/storage/lib/block_protocol/block-fifo.h"
#include "src/storage/lib/buffer/owned_vmoid.h"

namespace paver {

class BlockPartitionClient;

// Interface to synchronously read/write to a partition.
class PartitionClient {
 public:
  // Returns the block size which the vmo provided to read/write should be aligned to.
  virtual zx::result<size_t> GetBlockSize() = 0;

  virtual zx::result<size_t> GetPartitionSize() = 0;

  // Reads the specified size from the partition into |vmo|. |size| must be aligned to the block
  // size returned in `GetBlockSize`.
  virtual zx::result<> Read(const zx::vmo& vmo, size_t size) = 0;

  // Writes |vmo| into the partition. |vmo_size| must be aligned to the block size returned in
  // `GetBlockSize`.
  virtual zx::result<> Write(const zx::vmo& vmo, size_t vmo_size) = 0;

  // Issues a trim to the entire partition.
  virtual zx::result<> Trim() = 0;

  // Flushes all previous operations to persistent storage.
  virtual zx::result<> Flush() = 0;

  // Indicates whether the derrived class supports block operations.
  virtual bool SupportsBlockPartition() { return false; }

  virtual const VolumeConnector* connector() { return nullptr; }

  virtual ~PartitionClient() = default;
};

struct PartitionMetadata {
  std::string name;
  uuid::Uuid type_guid;
  uuid::Uuid instance_guid;
  uint64_t start_block_offset;
  uint64_t num_blocks;
  uint64_t flags;
};

class BlockPartitionClient : public PartitionClient {
 public:
  static zx::result<std::unique_ptr<BlockPartitionClient>> Create(
      std::unique_ptr<VolumeConnector> connector);

  zx::result<size_t> GetBlockSize() override;
  zx::result<size_t> GetPartitionSize() override;

  /// Fetches the metadata of the partition.  Fails if any of the fields aren't supported by the
  /// underlying partition implementation.  In general, this only makes sense to call on GPT-backed
  /// partitions.
  zx::result<PartitionMetadata> GetMetadata() const;

  zx::result<> Read(const zx::vmo& vmo, size_t size) override;
  zx::result<> Read(const zx::vmo& vmo, size_t size, size_t dev_offset, size_t vmo_offset);

  zx::result<> Write(const zx::vmo& vmo, size_t vmo_size) override;
  zx::result<> Write(const zx::vmo& vmo, size_t vmo_size, size_t dev_offset, size_t vmo_offset);

  zx::result<storage::OwnedVmoid> RegisterVmoid(const zx::vmo& vmo);
  zx::result<> Read(vmoid_t vmoid, size_t vmo_size, size_t dev_offset, size_t vmo_offset);
  zx::result<> Write(vmoid_t vmoid, size_t vmo_size, size_t dev_offset, size_t vmo_offset);

  zx::result<> Trim() override;
  zx::result<> Flush() override;

  const VolumeConnector* connector() override { return partition_connector_.get(); }

  // No copy.
  BlockPartitionClient(const BlockPartitionClient&) = delete;
  BlockPartitionClient& operator=(const BlockPartitionClient&) = delete;
  BlockPartitionClient(BlockPartitionClient&& o) = default;
  BlockPartitionClient& operator=(BlockPartitionClient&&) = default;

  bool SupportsBlockPartition() override { return true; }

  friend class FixedOffsetBlockPartitionClient;

 private:
  BlockPartitionClient(std::unique_ptr<VolumeConnector> connector,
                       fidl::WireSyncClient<fuchsia_storage_block::Block> partition)
      : partition_connector_(std::move(connector)), partition_(std::move(partition)) {}

  zx::result<> RegisterFastBlockIo();
  zx::result<std::reference_wrapper<fuchsia_storage_block::wire::BlockInfo>> ReadBlockInfo();

  std::unique_ptr<VolumeConnector> partition_connector_;
  fidl::WireSyncClient<fuchsia_storage_block::Block> partition_;
  std::unique_ptr<block_client::Client> client_;
  std::optional<fuchsia_storage_block::wire::BlockInfo> block_info_;
};

// A variant of BlockPartitionClient that reads/writes starting from a fixed offset in
// the partition and from a fixed offset in the given buffer.
// This is for those cases where image doesn't necessarily start from the beginning of
// the partition, (i.e. for preserving metatdata/header).
// It's also used for cases where input image is a combined image for multiple partitions.
class FixedOffsetBlockPartitionClient final : public BlockPartitionClient {
 public:
  FixedOffsetBlockPartitionClient(BlockPartitionClient client, size_t offset_partition_in_blocks,
                                  size_t offset_buffer_in_blocks)
      : BlockPartitionClient(std::move(client)),
        offset_partition_in_blocks_(offset_partition_in_blocks),
        offset_buffer_in_blocks_(offset_buffer_in_blocks) {}

  static zx::result<std::unique_ptr<FixedOffsetBlockPartitionClient>> Create(
      std::unique_ptr<VolumeConnector> connector, size_t offset_partition_in_blocks,
      size_t offset_buffer_in_blocks);

  zx::result<size_t> GetPartitionSize() final;
  zx::result<> Read(const zx::vmo& vmo, size_t size) final;
  zx::result<> Write(const zx::vmo& vmo, size_t vmo_size) final;

  zx::result<> Read(vmoid_t vmoid, size_t vmo_size, size_t dev_offset, size_t vmo_offset);
  zx::result<> Write(vmoid_t vmoid, size_t vmo_size, size_t dev_offset, size_t vmo_offset);

  // No copy, no move.
  FixedOffsetBlockPartitionClient(const FixedOffsetBlockPartitionClient&) = delete;
  FixedOffsetBlockPartitionClient& operator=(const FixedOffsetBlockPartitionClient&) = delete;
  FixedOffsetBlockPartitionClient(FixedOffsetBlockPartitionClient&&) = delete;
  FixedOffsetBlockPartitionClient& operator=(FixedOffsetBlockPartitionClient&&) = delete;

  zx::result<size_t> GetBufferOffsetInBytes();

 private:
  // offset in blocks for partition
  size_t offset_partition_in_blocks_ = 0;
  // offset in blocks for the input buffer
  size_t offset_buffer_in_blocks_ = 0;
};

// Specialized partition client which duplicates to multiple partitions, and attempts to read from
// each.
class PartitionCopyClient final : public PartitionClient {
 public:
  explicit PartitionCopyClient(std::vector<std::unique_ptr<PartitionClient>> partitions)
      : partitions_(std::move(partitions)) {}

  // Returns the LCM of all block sizes.
  zx::result<size_t> GetBlockSize() final;
  // Returns the minimum of all partition sizes.
  zx::result<size_t> GetPartitionSize() final;
  // Attempt to read from each partition, returning on the first successful one.
  zx::result<> Read(const zx::vmo& vmo, size_t size) final;
  // Write to *every* partition.
  zx::result<> Write(const zx::vmo& vmo, size_t vmo_size) final;
  // Trim *every* partition.
  zx::result<> Trim() final;
  // Flush *every* partition.
  zx::result<> Flush() final;

  // No copy, no move.
  PartitionCopyClient(const PartitionCopyClient&) = delete;
  PartitionCopyClient& operator=(const PartitionCopyClient&) = delete;
  PartitionCopyClient(PartitionCopyClient&&) = delete;
  PartitionCopyClient& operator=(PartitionCopyClient&&) = delete;

 private:
  std::vector<std::unique_ptr<PartitionClient>> partitions_;
};

}  // namespace paver

#endif  // SRC_FIRMWARE_PAVER_PARTITION_CLIENT_H_
