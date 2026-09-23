// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/firmware/paver/skip-block.h"

#include <fidl/fuchsia.device/cpp/wire.h>
#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <lib/component/incoming/cpp/clone.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fdio/cpp/caller.h>
#include <lib/fdio/directory.h>
#include <libgen.h>

#include <fbl/string_buffer.h>
#include <gpt/gpt.h>

#include "src/firmware/paver/pave-logging.h"
#include "src/firmware/paver/utils.h"
#include "src/lib/uuid/uuid.h"

namespace paver {

namespace {

using uuid::Uuid;

namespace skipblock = fuchsia_hardware_skipblock;

}  // namespace

zx::result<std::unique_ptr<SkipBlockPartitionClient>> SkipBlockDevicePartitioner::FindPartition(
    const Uuid& type) const {
  zx::result partition = OpenSkipBlockPartition(skip_block_devices_, type, ZX_SEC(5));
  if (partition.is_error()) {
    return partition.take_error();
  }

  zx::result connection = partition->Connect();
  if (connection.is_error()) {
    return connection.take_error();
  }
  return zx::ok(new SkipBlockPartitionClient(
      fidl::ClientEnd<fuchsia_hardware_skipblock::SkipBlock>(connection->TakeChannel())));
}

zx::result<std::unique_ptr<PartitionClient>> SkipBlockDevicePartitioner::FindFvmPartition() const {
  // FVM partition is managed so it should expose a normal block device.
  zx::result partition =
      OpenBlockPartition(devices_, std::nullopt, Uuid(GUID_FVM_VALUE), std::nullopt, ZX_SEC(5));
  if (partition.is_error()) {
    return partition.take_error();
  }
  return BlockPartitionClient::Create(std::move(*partition));
}

zx::result<> SkipBlockPartitionClient::ReadPartitionInfo() {
  if (!partition_info_) {
    auto result = partition_->GetPartitionInfo();
    auto status = zx::make_result(result.ok() ? result.value().status : result.status());
    if (status.is_error()) {
      ERROR("Failed to get partition info with status: %s\n", status.status_string());
      return status.take_error();
    }
    partition_info_ = result.value().partition_info;
  }
  return zx::ok();
}

zx::result<size_t> SkipBlockPartitionClient::GetBlockSize() {
  auto status = ReadPartitionInfo();
  if (status.is_error()) {
    return status.take_error();
  }
  return zx::ok(static_cast<size_t>(partition_info_->block_size_bytes));
}

zx::result<size_t> SkipBlockPartitionClient::GetPartitionSize() {
  auto status = ReadPartitionInfo();
  if (status.is_error()) {
    return status.take_error();
  }
  return zx::ok(partition_info_->block_size_bytes * partition_info_->partition_block_count);
}

zx::result<> SkipBlockPartitionClient::Read(const zx::vmo& vmo, size_t size) {
  auto status = SkipBlockPartitionClient::GetBlockSize();
  if (status.is_error()) {
    return status.take_error();
  }
  const size_t block_size = status.value();

  zx::vmo dup;
  if (auto status = zx::make_result(vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup)); status.is_error()) {
    ERROR("Couldn't duplicate buffer vmo\n");
    return status.take_error();
  }

  skipblock::wire::ReadWriteOperation operation = {
      .vmo = std::move(dup),
      .vmo_offset = 0,
      .block = 0,
      .block_count = static_cast<uint32_t>(size / block_size),
  };

  auto result = partition_->Read(std::move(operation));
  {
    auto status = zx::make_result(result.ok() ? result.value().status : result.status());
    if (status.is_error()) {
      ERROR("Error reading partition data: %s\n", status.status_string());
      return status.take_error();
    }
  }
  return zx::ok();
}

zx::result<> SkipBlockPartitionClient::Write(const zx::vmo& vmo, size_t size) {
  auto status = SkipBlockPartitionClient::GetBlockSize();
  if (status.is_error()) {
    return status.take_error();
  }
  size_t block_size = status.value();

  zx::vmo dup;
  if (auto status = zx::make_result(vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup)); status.is_error()) {
    ERROR("Couldn't duplicate buffer vmo\n");
    return status.take_error();
  }

  skipblock::wire::ReadWriteOperation operation = {
      .vmo = std::move(dup),
      .vmo_offset = 0,
      .block = 0,
      .block_count = static_cast<uint32_t>(size / block_size),
  };

  auto result = partition_->Write(std::move(operation));
  {
    auto status = zx::make_result(result.ok() ? result.value().status : result.status());
    if (status.is_error()) {
      ERROR("Error writing partition data: %s\n", status.status_string());
      return status.take_error();
    }
  }
  return zx::ok();
}

zx::result<> SkipBlockPartitionClient::WriteBytes(const zx::vmo& vmo, zx_off_t offset,
                                                  size_t size) {
  zx::vmo dup;
  if (auto status = zx::make_result(vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup)); status.is_error()) {
    ERROR("Couldn't duplicate buffer vmo\n");
    return status.take_error();
  }

  skipblock::wire::WriteBytesOperation operation = {
      .vmo = std::move(dup),
      .vmo_offset = 0,
      .offset = offset,
      .size = size,
      .mode = skipblock::wire::WriteBytesMode::kReadModifyEraseWrite,
  };

  auto result = partition_->WriteBytes(std::move(operation));
  auto status = zx::make_result(result.ok() ? result.value().status : result.status());
  if (status.is_error()) {
    ERROR("Error writing partition data: %s\n", status.status_string());
    return status.take_error();
  }
  return zx::ok();
}

zx::result<> SkipBlockPartitionClient::Trim() { return zx::error(ZX_ERR_NOT_SUPPORTED); }

zx::result<> SkipBlockPartitionClient::Flush() { return zx::ok(); }

}  // namespace paver
