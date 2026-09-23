// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/firmware/paver/gpt.h"

#include <dirent.h>
#include <fidl/fuchsia.device/cpp/wire.h>
#include <fidl/fuchsia.fshost/cpp/wire.h>
#include <fidl/fuchsia.storage.partitions/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/incoming/cpp/service.h>
#include <lib/fdio/directory.h>
#include <lib/fidl/cpp/wire/status.h>
#include <lib/fit/defer.h>
#include <lib/zx/result.h>

#include <algorithm>
#include <cinttypes>
#include <string_view>

#include <fbl/algorithm.h>
#include <fbl/unique_fd.h>
#include <gpt/c/gpt.h>

#include "src/firmware/paver/block-devices.h"
#include "src/firmware/paver/pave-logging.h"
#include "src/firmware/paver/utils.h"
#include "zircon/status.h"

namespace paver {

namespace {

using uuid::Uuid;

constexpr size_t ReservedHeaderBlocks(size_t blk_size) {
  constexpr size_t kReservedEntryBlocks{static_cast<size_t>(16) * 1024};
  return (kReservedEntryBlocks + 2 * blk_size) / blk_size;
}

zx::result<GptPartitionMetadata> QueryGptPartitionMetadata(
    fidl::UnownedClientEnd<fuchsia_storage_block::Block> volume) {
  using fuchsia_storage_block::Block;
  GptPartitionMetadata metadata;

  fidl::WireResult result = fidl::WireCall<Block>(volume)->GetMetadata();
  if (!result.ok()) {
    return zx::error(result.status());
  }
  if (result->is_error()) {
    return result->take_error();
  }
  if (!result.value()->has_name() || !result.value()->has_type_guid() ||
      !result.value()->has_instance_guid()) {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }
  return zx::ok(GptPartitionMetadata{
      .name = std::string(result.value()->name().cbegin(), result.value()->name().cend()),
      .type_guid = Uuid(result.value()->type_guid().value.data()),
      .instance_guid = Uuid(result.value()->instance_guid().value.data()),
  });
}

}  // namespace

using PartitionInitSpec = GptDevicePartitioner::PartitionInitSpec;

PartitionInitSpec PartitionInitSpec::ForKnownPartition(Partition partition, PartitionScheme scheme,
                                                       size_t size_bytes) {
  const char* name = PartitionName(partition, scheme);
  std::optional<Uuid> type = PartitionTypeGuid(partition, scheme);
  ZX_ASSERT(name && type);
  return PartitionInitSpec{
      .name = name,
      .type = *type,
      .start_block = 0,
      .size_bytes = size_bytes,
  };
}

bool FilterByName(const GptPartitionMetadata& part, std::string_view name) {
  if (name.length() != part.name.length()) {
    return false;
  }
  // We use a case-insensitive comparison to be compatible with the previous naming scheme.
  // On a ChromeOS device, all of the kernel partitions share a common GUID type, so we
  // distinguish Zircon kernel partitions based on name.
  return strncasecmp(part.name.data(), name.data(), name.length()) == 0;
}

bool FilterByTypeAndName(const GptPartitionMetadata& part, const Uuid& type,
                         std::string_view name) {
  return type == part.type_guid && FilterByName(part, name);
}

bool IsFuchsiaSystemPartition(const PaverConfig& config, const GptPartitionMetadata& part) {
  if (IsFvmPartition(part)) {
    return true;
  }
  for (const auto& name : config.system_partition_names) {
    if (FilterByName(part, name)) {
      return true;
    }
  }
  return false;
}

zx::result<GptDevicePartitioner::InitializeGptResult> GptDevicePartitioner::InitializeGpt(
    const paver::BlockDevices& devices, fidl::UnownedClientEnd<fuchsia_io::Directory> svc_root,
    const PaverConfig& config) {
  // Fshost takes care of finding the GPT block device.
  zx::result gpt_device_source = BlockDevices::CreateFromPartitionService(svc_root);
  if (gpt_device_source.is_error()) {
    ERROR("Failed to connect to GPT: %s\n", gpt_device_source.status_string());
    return gpt_device_source.take_error();
  }

  zx::result manager =
      component::ConnectAt<fuchsia_storage_partitions::PartitionsManager>(svc_root);
  if (manager.is_error()) {
    return manager.take_error();
  }
  fidl::WireResult info = fidl::WireCall(*manager)->GetBlockInfo();
  if (!info.ok()) {
    ERROR("Warning: Could not acquire GPT block info: %s\n", info.FormatDescription().c_str());
    return zx::error(info.status());
  }
  if (info.value().is_error()) {
    ERROR("Warning: Could not acquire GPT block info: %s\n",
          zx_status_get_string((info.value().error_value())));
    return info.value().take_error();
  }
  auto partitioner =
      WrapUnique(new GptDevicePartitioner(std::move(*gpt_device_source), svc_root,
                                          info->value()->block_count, info->value()->block_size));

  bool initialize_partition_tables = false;
  // If the GPT is missing the necessary partitions, bubble that up so the caller can decide
  // whether to reset the partition tables.
  if (zx::result find = partitioner->FindPartition([&config](const GptPartitionMetadata& part) {
        return IsFuchsiaSystemPartition(config, part);
      });
      find.is_error()) {
    if (find.status_value() != ZX_ERR_NOT_FOUND) {
      ERROR("Failed to look up FVM partition in GPT: %s\n", find.status_string());
    }
    ERROR(
        "Unable to find a GPT on this device with the expected partitions.\n"
        "Attempting to reinitialize partition tables; this only works on recovery builds!\n"
        "If this fails, please run init-partition-tables to re-initialize the device.\n");
    initialize_partition_tables = true;
  }

  return zx::ok(InitializeGptResult{
      .gpt = std::move(partitioner),
      .initialize_partition_tables = initialize_partition_tables,
  });
}

struct PartitionPosition {
  size_t start;   // Block, inclusive
  size_t length;  // In Blocks
};

zx::result<std::unique_ptr<BlockPartitionClient>> GptDevicePartitioner::FindPartition(
    FilterCallback filter) const {
  zx::result result = devices_.OpenPartition([&](const zx::channel& chan) -> bool {
    auto client = fidl::UnownedClientEnd<fuchsia_storage_block::Block>(chan.borrow());
    zx::result metadata = QueryGptPartitionMetadata(client);
    if (metadata.is_error()) {
      if (metadata.status_value() != ZX_ERR_NOT_SUPPORTED) {
        ERROR("Failed to query GPT partition metadata: %s\n", metadata.status_string());
      }
      return false;
    }
    return filter(*metadata);
  });
  if (result.is_error()) {
    return result.take_error();
  }
  return BlockPartitionClient::Create(std::move(*result));
}

zx::result<std::vector<std::unique_ptr<BlockPartitionClient>>>
GptDevicePartitioner::FindAllPartitions(GptDevicePartitioner::FilterCallback filter) const {
  zx::result result = devices_.OpenAllPartitions([&](const zx::channel& chan) -> bool {
    auto client = fidl::UnownedClientEnd<fuchsia_storage_block::Block>((chan.borrow()));
    zx::result metadata = QueryGptPartitionMetadata(client);
    if (metadata.is_error()) {
      if (metadata.status_value() != ZX_ERR_NOT_SUPPORTED) {
        ERROR("Failed to query GPT partition metadata: %s\n", metadata.status_string());
      }
      return false;
    }
    return filter(*metadata);
  });
  if (result.is_error()) {
    return result.take_error();
  }
  std::vector<std::unique_ptr<BlockPartitionClient>> clients;
  for (auto& connector : *result) {
    zx::result result = BlockPartitionClient::Create(std::move((connector)));
    if (result.is_error()) {
      return result.take_error();
    }
    clients.push_back(std::move(*result));
  }
  return zx::ok(std::move(clients));
}

zx::result<> GptDevicePartitioner::ResetPartitionTables(
    std::vector<GptDevicePartitioner::PartitionInitSpec> partitions) const {
  // Assign offsets and instance GUIDs as needed.
  uint64_t metadata_blocks = ReservedHeaderBlocks(block_size_);
  uint64_t last_available_block = block_count_ - metadata_blocks;
  struct Range {
    uint64_t start;
    uint64_t end;
  };
  std::vector<Range> allocations = {
      Range{.start = 0, .end = metadata_blocks},
      Range{.start = last_available_block, .end = block_count_},
  };

  // Returns the position to insert at, and the block offset to use.
  auto find_first_fit = [&](uint64_t num_blocks) -> zx::result<std::tuple<size_t, uint64_t>> {
    for (size_t i = 1; i < allocations.size(); ++i) {
      const auto& prev = allocations[i - 1];
      const auto& next = allocations[i];
      if (next.start - prev.end >= num_blocks) {
        return zx::ok(std::make_tuple(i, prev.end));
      }
    }
    return zx::error(ZX_ERR_NO_SPACE);
  };

  for (auto& partition : partitions) {
    if (partition.size_bytes == 0) {
      continue;
    }
    if (partition.size_bytes % block_size_ > 0) {
      ERROR("Misaligned partition\n");
      return zx::error(ZX_ERR_INVALID_ARGS);
    }
    uint64_t num_blocks = partition.size_bytes / block_size_;
    constexpr const Uuid kNilGuid;
    if (partition.instance == kNilGuid) {
      partition.instance = Uuid::Generate();
    }
    auto pos = allocations.end();
    if (partition.start_block == 0) {
      zx::result result = find_first_fit(num_blocks);
      if (result.is_error()) {
        return result.take_error();
      }
      auto [index, off] = *result;
      partition.start_block = off;
      pos = std::next(allocations.begin(), static_cast<int64_t>(index));
      LOG("Allocated partition %s @ %" PRIu64 "\n", partition.name.c_str(), off);
    } else {
      pos = std::lower_bound(
          allocations.begin(), allocations.end(), partition.start_block,
          [](const Range& range, uint64_t offset) -> bool { return range.start < offset; });
      if (pos == allocations.end()) {
        return zx::error(ZX_ERR_INVALID_ARGS);
      }
    }
    allocations.insert(pos, Range{
                                .start = partition.start_block,
                                .end = partition.start_block + num_blocks,
                            });
  }

  fidl::Arena arena;
  fidl::VectorView<fuchsia_storage_partitions::wire::PartitionEntry> infos(arena,
                                                                           partitions.size());
  for (size_t i = 0; i < partitions.size(); ++i) {
    const auto& partition = partitions[i];
    if (partition.size_bytes == 0) {
      continue;
    }
    fuchsia_storage_partitions::wire::PartitionEntry info{
        .name = fidl::StringView::FromExternal(partition.name),
        .start_block = partition.start_block,
        .num_blocks = partition.size_bytes / block_size_,
        .flags = partition.flags,
    };
    std::copy(partition.type.cbegin(), partition.type.cend(), info.type_guid.value.data());
    std::copy(partition.instance.cbegin(), partition.instance.cend(),
              info.instance_guid.value.data());
    infos[i] = info;
  }

  zx::result recovery = component::ConnectAt<fuchsia_fshost::Recovery>(svc_root_.borrow());
  if (recovery.is_error()) {
    return recovery.take_error();
  }
  fidl::WireResult result = fidl::WireCall(*recovery)->InitSystemPartitionTable(infos);
  if (result.status() != ZX_OK) {
    ERROR("Failed to reset partitions table: %s\n", result.FormatDescription().c_str());
    return zx::error(result.status());
  }
  if (result->is_error()) {
    ERROR("Failed to reset partitions table: %s\n", zx_status_get_string(result->error_value()));
    return zx::error(result->error_value());
  }
  return zx::ok();
}

}  // namespace paver
