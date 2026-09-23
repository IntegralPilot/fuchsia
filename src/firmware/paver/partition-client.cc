// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/firmware/paver/partition-client.h"

#include <lib/component/incoming/cpp/clone.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/fd.h>
#include <lib/fzl/vmo-mapper.h>
#include <zircon/errors.h>
#include <zircon/limits.h>
#include <zircon/status.h>

#include <cstdint>
#include <memory>
#include <numeric>

#include <fbl/algorithm.h>
#include <fbl/unique_fd.h>

#include "lib/fidl/cpp/wire/channel.h"
#include "src/firmware/paver/pave-logging.h"
#include "src/lib/uuid/uuid.h"
#include "src/storage/lib/buffer/owned_vmoid.h"

namespace paver {

namespace block = fuchsia_storage_block;

zx::result<std::unique_ptr<BlockPartitionClient>> BlockPartitionClient::Create(
    std::unique_ptr<VolumeConnector> connector) {
  zx::result partition_client_end = connector->Connect();
  if (partition_client_end.is_error()) {
    return partition_client_end.take_error();
  }
  fidl::WireSyncClient<fuchsia_storage_block::Block> partition(
      fidl::ClientEnd<fuchsia_storage_block::Block>(partition_client_end->TakeChannel()));
  return zx::ok(std::unique_ptr<BlockPartitionClient>(
      new BlockPartitionClient(std::move(connector), std::move(partition))));
}

zx::result<std::reference_wrapper<fuchsia_storage_block::wire::BlockInfo>>
BlockPartitionClient::ReadBlockInfo() {
  if (block_info_.has_value()) {
    return zx::ok(std::reference_wrapper(block_info_.value()));
  }
  const fidl::WireResult result = partition_->GetInfo();
  if (!result.ok()) {
    ERROR("Failed to get partition info with status: %s\n", result.FormatDescription().c_str());
    return zx::error(result.status());
  }
  fit::result response = result.value();
  if (response.is_error()) {
    ERROR("Failed to get partition info with status: %s\n",
          zx_status_get_string(response.error_value()));
    return response.take_error();
  }
  return zx::ok(std::reference_wrapper(block_info_.emplace(response.value()->info)));
}

zx::result<size_t> BlockPartitionClient::GetBlockSize() {
  zx::result block_info = ReadBlockInfo();
  if (block_info.is_error()) {
    return block_info.take_error();
  }
  return zx::ok(block_info.value().get().block_size);
}

zx::result<size_t> BlockPartitionClient::GetPartitionSize() {
  zx::result block_info_result = ReadBlockInfo();
  if (block_info_result.is_error()) {
    return block_info_result.take_error();
  }
  const fuchsia_storage_block::wire::BlockInfo& block_info = block_info_result.value().get();
  return zx::ok(block_info.block_size * block_info.block_count);
}

zx::result<PartitionMetadata> BlockPartitionClient::GetMetadata() const {
  const fidl::WireResult result = partition_->GetMetadata();
  if (!result.ok() || result->is_error()) {
    ERROR("Failed to get partition metadata with status: %s\n", result.FormatDescription().c_str());
    return zx::error(result.status());
  }
  const auto& value = result.value();
  if (!value->has_name() || !value->has_type_guid() || !value->has_instance_guid()) {
    ERROR("Called GetMetadata on a partition that doesn't support required fields.\n");
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }
  return zx::ok(PartitionMetadata{
      .name = std::string(value->name().cbegin(), value->name().cend()),
      .type_guid = uuid::Uuid(value->type_guid().value.data()),
      .instance_guid = uuid::Uuid(value->instance_guid().value.data()),
      .start_block_offset = value->start_block_offset(),
      .num_blocks = value->num_blocks(),
      .flags = value->flags(),
  });
}

zx::result<> BlockPartitionClient::RegisterFastBlockIo() {
  if (client_) {
    return zx::ok();
  }
  auto [client, server] = fidl::Endpoints<block::Session>::Create();
  if (fidl::Status result = partition_->OpenSession(std::move(server)); !result.ok()) {
    return zx::error(result.status());
  }
  const fidl::WireResult result = fidl::WireCall(client)->GetFifo();
  if (!result.ok()) {
    return zx::error(result.status());
  }
  fit::result response = result.value();
  if (response.is_error()) {
    return response.take_error();
  }
  client_ =
      std::make_unique<block_client::Client>(std::move(client), std::move(response.value()->fifo));
  return zx::ok();
}

zx::result<storage::OwnedVmoid> BlockPartitionClient::RegisterVmoid(const zx::vmo& vmo) {
  auto status = RegisterFastBlockIo();
  if (status.is_error()) {
    return status.take_error();
  }

  storage::OwnedVmoid vmoid(client_.get());
  if (zx_status_t status = vmoid.AttachVmo(vmo); status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(std::move(vmoid));
}

zx::result<> BlockPartitionClient::Read(const zx::vmo& vmo, size_t size) {
  return Read(vmo, size, 0, 0);
}

zx::result<> BlockPartitionClient::Read(const zx::vmo& vmo, size_t size, size_t dev_offset,
                                        size_t vmo_offset) {
  zx::result vmoid = RegisterVmoid(vmo);
  if (vmoid.is_error()) {
    return vmoid.take_error();
  }
  return Read(vmoid->get(), size, dev_offset, vmo_offset);
}

zx::result<> BlockPartitionClient::Read(vmoid_t vmoid, size_t vmo_size, size_t dev_offset,
                                        size_t vmo_offset) {
  zx::result block_size = GetBlockSize();
  if (block_size.is_error()) {
    return block_size.take_error();
  }
  const uint64_t length = vmo_size / block_size.value();
  if (length > UINT32_MAX) {
    ERROR("Error reading partition data: Too large\n");
    return zx::error(ZX_ERR_OUT_OF_RANGE);
  }

  BlockFifoRequest request = {
      .command = {.opcode = BLOCK_OPCODE_READ, .flags = 0},
      .group = 0,
      .vmoid = vmoid,
      .length = static_cast<uint32_t>(length),
      .vmo_offset = vmo_offset,
      .dev_offset = dev_offset,
  };

  if (auto status = zx::make_result(client_->Transaction(&request, 1)); status.is_error()) {
    ERROR("Error reading partition data: %s\n", status.status_string());
    return status.take_error();
  }

  return zx::ok();
}

zx::result<> BlockPartitionClient::Write(const zx::vmo& vmo, size_t vmo_size) {
  return Write(vmo, vmo_size, 0, 0);
}

zx::result<> BlockPartitionClient::Write(const zx::vmo& vmo, size_t vmo_size, size_t dev_offset,
                                         size_t vmo_offset) {
  zx::result vmoid = RegisterVmoid(vmo);
  if (vmoid.is_error()) {
    return vmoid.take_error();
  }
  return Write(vmoid->get(), vmo_size, dev_offset, vmo_offset);
}

zx::result<> BlockPartitionClient::Write(vmoid_t vmoid, size_t vmo_size, size_t dev_offset,
                                         size_t vmo_offset) {
  zx::result block_size = GetBlockSize();
  if (block_size.is_error()) {
    return block_size.take_error();
  }
  uint64_t length = vmo_size / block_size.value();
  if (length > UINT32_MAX) {
    ERROR("Error writing partition data: Too large\n");
    return zx::error(ZX_ERR_OUT_OF_RANGE);
  }

  BlockFifoRequest request = {
      .command = {.opcode = BLOCK_OPCODE_WRITE, .flags = 0},
      .group = 0,
      .vmoid = vmoid,
      .length = static_cast<uint32_t>(length),
      .vmo_offset = vmo_offset,
      .dev_offset = dev_offset,
  };

  if (auto status = zx::make_result(client_->Transaction(&request, 1)); status.is_error()) {
    ERROR("Error writing partition data: %s\n", status.status_string());
    return status.take_error();
  }
  return zx::ok();
}

zx::result<> BlockPartitionClient::Trim() {
  zx::result block_info = ReadBlockInfo();
  if (block_info.is_error()) {
    return block_info.take_error();
  }
  uint64_t block_count = block_info.value().get().block_count;

  if (zx::result status = RegisterFastBlockIo(); status.is_error()) {
    return status.take_error();
  }

  BlockFifoRequest request = {
      .command = {.opcode = BLOCK_OPCODE_TRIM, .flags = 0},
      .group = 0,
      .vmoid = BLOCK_VMOID_INVALID,
      .length = static_cast<uint32_t>(block_count),
      .vmo_offset = 0,
      .dev_offset = 0,
  };

  return zx::make_result(client_->Transaction(&request, 1));
}

zx::result<> BlockPartitionClient::Flush() {
  auto status = RegisterFastBlockIo();
  if (status.is_error()) {
    return status.take_error();
  }

  BlockFifoRequest request = {
      .command = {.opcode = BLOCK_OPCODE_FLUSH, .flags = 0},
      .group = 0,
      .vmoid = BLOCK_VMOID_INVALID,
      .length = 0,
      .vmo_offset = 0,
      .dev_offset = 0,
  };

  return zx::make_result(client_->Transaction(&request, 1));
}

zx::result<std::unique_ptr<FixedOffsetBlockPartitionClient>>
FixedOffsetBlockPartitionClient::Create(std::unique_ptr<VolumeConnector> connector,
                                        size_t offset_partition_in_blocks,
                                        size_t offset_buffer_in_blocks) {
  zx::result partition_client_end = connector->Connect();
  if (partition_client_end.is_error()) {
    return partition_client_end.take_error();
  }
  fidl::WireSyncClient<fuchsia_storage_block::Block> block(
      fidl::ClientEnd<fuchsia_storage_block::Block>(partition_client_end->TakeChannel()));
  return zx::ok(std::make_unique<FixedOffsetBlockPartitionClient>(
      BlockPartitionClient(std::move(connector), std::move(block)), offset_partition_in_blocks,
      offset_buffer_in_blocks));
}

// The partition size does not account for the offset.
zx::result<size_t> FixedOffsetBlockPartitionClient::GetPartitionSize() {
  auto status_or_block_size = GetBlockSize();
  if (status_or_block_size.is_error()) {
    return status_or_block_size.take_error();
  }
  const size_t block_size = status_or_block_size.value();

  auto status_or_part_size = BlockPartitionClient::GetPartitionSize();
  if (status_or_part_size.is_error()) {
    return status_or_part_size.take_error();
  }
  const size_t full_size = status_or_part_size.value();

  if (full_size < block_size * offset_partition_in_blocks_) {
    ERROR("Inconsistent partition size with block counts and block size\n");
    return zx::error(ZX_ERR_INTERNAL);
  }

  return zx::ok(full_size - block_size * offset_partition_in_blocks_);
}

zx::result<> FixedOffsetBlockPartitionClient::Read(const zx::vmo& vmo, size_t size) {
  return BlockPartitionClient::Read(vmo, size, offset_partition_in_blocks_,
                                    offset_buffer_in_blocks_);
}

zx::result<> FixedOffsetBlockPartitionClient::Read(vmoid_t vmoid, size_t vmo_size,
                                                   size_t dev_offset, size_t vmo_offset) {
  return BlockPartitionClient::Read(vmoid, vmo_size, offset_partition_in_blocks_ + dev_offset,
                                    offset_buffer_in_blocks_ + vmo_offset);
}

zx::result<> FixedOffsetBlockPartitionClient::Write(const zx::vmo& vmo, size_t vmo_size) {
  return BlockPartitionClient::Write(vmo, vmo_size, offset_partition_in_blocks_,
                                     offset_buffer_in_blocks_);
}

zx::result<> FixedOffsetBlockPartitionClient::Write(vmoid_t vmoid, size_t vmo_size,
                                                    size_t dev_offset, size_t vmo_offset) {
  return BlockPartitionClient::Write(vmoid, vmo_size, offset_partition_in_blocks_ + dev_offset,
                                     offset_buffer_in_blocks_ + vmo_offset);
}

zx::result<size_t> FixedOffsetBlockPartitionClient::GetBufferOffsetInBytes() {
  auto status_or_block_size = GetBlockSize();
  if (status_or_block_size.is_error()) {
    return status_or_block_size.take_error();
  }
  const size_t block_size = status_or_block_size.value();
  return zx::ok(block_size * offset_buffer_in_blocks_);
}

zx::result<size_t> PartitionCopyClient::GetBlockSize() {
  // Choose the lowest common multiple of all block sizes.
  size_t lcm = 1;
  for (auto& partition : partitions_) {
    if (auto status = partition->GetBlockSize(); status.is_ok()) {
      lcm = std::lcm(lcm, status.value());
    }
  }
  if (lcm == 0 || lcm == 1) {
    return zx::error(ZX_ERR_IO);
  }
  return zx::ok(lcm);
}

zx::result<size_t> PartitionCopyClient::GetPartitionSize() {
  // Return minimum size of all partitions.
  bool one_succeed = false;
  size_t minimum_size = UINT64_MAX;
  for (auto& partition : partitions_) {
    if (auto status = partition->GetPartitionSize(); status.is_ok()) {
      one_succeed = true;
      minimum_size = std::min(minimum_size, status.value());
    }
  }
  if (!one_succeed) {
    return zx::error(ZX_ERR_IO);
  }
  return zx::ok(minimum_size);
}

zx::result<> PartitionCopyClient::Read(const zx::vmo& vmo, size_t size) {
  // Read until one is successful.
  for (auto& partition : partitions_) {
    if (auto status = partition->Read(vmo, size); status.is_ok()) {
      return zx::ok();
    }
  }
  return zx::error(ZX_ERR_IO);
}

zx::result<> PartitionCopyClient::Write(const zx::vmo& vmo, size_t size) {
  // Guaranatee at least one write was successful.
  bool one_succeed = false;
  for (auto& partition : partitions_) {
    if (auto status = partition->Write(vmo, size); status.is_ok()) {
      one_succeed = true;
    } else {
      // Best effort trim the partition.
      partition->Trim().status_value();
    }
  }
  if (one_succeed) {
    return zx::ok();
  }
  return zx::error(ZX_ERR_IO);
}

zx::result<> PartitionCopyClient::Trim() {
  // All must trim successfully.
  for (auto& partition : partitions_) {
    if (auto status = partition->Trim(); status.is_error()) {
      return status.take_error();
    }
  }
  return zx::ok();
}

zx::result<> PartitionCopyClient::Flush() {
  // All must flush successfully.
  for (auto& partition : partitions_) {
    if (auto status = partition->Flush(); status.is_error()) {
      return status.take_error();
    }
  }
  return zx::ok();
}

}  // namespace paver
