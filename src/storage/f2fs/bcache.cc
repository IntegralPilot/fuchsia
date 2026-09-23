// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/f2fs/bcache.h"

#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/fdio/cpp/caller.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>

#include <fbl/alloc_checker.h>
#include <fbl/ref_ptr.h>

#include "src/storage/f2fs/layout.h"
#include "src/storage/f2fs/segment.h"
#include "src/storage/fvm/client.h"
#include "src/storage/lib/block_client/cpp/remote_block_device.h"
#include "src/storage/lib/buffer/block_buffer.h"
#include "src/storage/lib/buffer/vmo_buffer.h"
#include "src/storage/lib/operation/operation.h"

namespace f2fs {

zx::result<std::unique_ptr<BcacheMapper>> CreateBcacheMapper(
    std::vector<std::unique_ptr<block_client::BlockDevice>> devices, bool allocate) {
  uint64_t total_block_count = 0;
  constexpr uint32_t kMinVolumeSize = kMinVolumeSegments * kDefaultSegmentSize;

  std::vector<std::unique_ptr<Bcache>> bcaches;
  for (auto& device : devices) {
    fuchsia_storage_block::wire::BlockInfo info;
    if (zx_status_t status = device->BlockGetInfo(&info); status != ZX_OK) {
      FX_LOGS(ERROR) << "Could not access device info: " << status;
      return zx::error(status);
    }

    if (info.block_size == 0 || kBlockSize % info.block_size != 0 ||
        info.block_size < kDefaultSectorSize || info.block_size > kBlockSize) {
      FX_LOGS(ERROR) << info.block_size << " of block size is not supported";
      return zx::error(ZX_ERR_INVALID_ARGS);
    }

    uint64_t block_count = info.block_size * info.block_count / kBlockSize;
    fuchsia_storage_block::wire::VolumeManagerInfo manager_info;
    fuchsia_storage_block::wire::VolumeInfo volume_info;
    bool use_fvm = device->VolumeGetInfo(&manager_info, &volume_info) == ZX_OK;
    if (use_fvm) {
      if (allocate) {
        if (zx_status_t status = fvm::ResetAllSlices(device.get()); status != ZX_OK) {
          FX_LOGS(ERROR) << "failed to reset FVM slices: " << zx_status_get_string(status);
          return zx::error(status);
        }
        device->VolumeGetInfo(&manager_info, &volume_info);
      }
      size_t slice_size = manager_info.slice_size;
      size_t slice_count = volume_info.partition_slice_count;
      ZX_ASSERT_MSG(kDefaultSegmentSize % manager_info.slice_size == 0 ||
                        manager_info.slice_size % kDefaultSegmentSize == 0,
                    " slice_size is not aligned with segment boundaries %lu",
                    manager_info.slice_size);

      size_t free = manager_info.slice_count - manager_info.assigned_slice_count;
      size_t max_allowable =
          std::min(free + volume_info.partition_slice_count, manager_info.maximum_slice_count);
      if (volume_info.slice_limit) {
        max_allowable = std::min(volume_info.slice_limit, max_allowable);
      }
      slice_count = max_allowable;

      size_t offset = volume_info.partition_slice_count;
      size_t end = CheckedDivRoundUp<size_t>(kMinVolumeSize, slice_size);
      if (allocate && offset < end) {
        if (zx_status_t status = device->VolumeExtend(offset, end - offset); status != ZX_OK) {
          FX_LOGS(ERROR) << "failed to extend volume from " << offset << " to " << end << ": "
                         << zx_status_get_string(status);
          return zx::error(status);
        }
      }
      FX_LOGS(INFO) << "Total slice count: " << slice_count << "(" << slice_size << "B)";
      block_count = slice_size * slice_count / kBlockSize;
    }

    zx::result bcache = Bcache::Create(std::move(device), block_count, kBlockSize);
    if (bcache.is_error()) {
      return bcache.take_error();
    }
    bcaches.push_back(std::move(bcache.value()));
    total_block_count += block_count;
  }
  if (total_block_count < kMinVolumeSize / kBlockSize) {
    FX_LOGS(ERROR) << "block device is too small";
    return zx::error(ZX_ERR_NO_SPACE);
  }
  // The maximum volume size of f2fs is 16TiB
  if (total_block_count >= std::numeric_limits<uint32_t>::max()) {
    FX_LOGS(ERROR) << "block device is too large (> 16TiB)";
    return zx::error(ZX_ERR_OUT_OF_RANGE);
  }

  return BcacheMapper::Create(std::move(bcaches));
}

zx::result<std::unique_ptr<BcacheMapper>> CreateBcacheMapper(
    std::unique_ptr<block_client::BlockDevice> device, bool allocate) {
  std::vector<std::unique_ptr<block_client::BlockDevice>> devices;
  devices.push_back(std::move(device));
  return CreateBcacheMapper(std::move(devices), allocate);
}

zx::result<std::unique_ptr<BcacheMapper>> CreateBcacheMapper(
    fidl::ClientEnd<fuchsia_storage_block::Block> device_channel, bool allocate) {
  zx::result device = block_client::RemoteBlockDevice::Create(
      fidl::ClientEnd<fuchsia_storage_block::Block>{device_channel.TakeChannel()});
  if (device.is_error()) {
    FX_LOGS(ERROR) << "could not initialize block device";
    return device.take_error();
  }

  zx::result bc = CreateBcacheMapper(std::move(*device), allocate);
  if (bc.is_error()) {
    FX_LOGS(ERROR) << "could not create block cache";
    return bc.take_error();
  }
  return bc.take_value();
}

Bcache::Bcache(std::unique_ptr<block_client::BlockDevice> device, uint64_t max_blocks,
               block_t block_size)
    : max_blocks_(max_blocks), block_size_(block_size), device_(std::move(device)) {
  fuchsia_storage_block::wire::VolumeManagerInfo manager_info;
  fuchsia_storage_block::wire::VolumeInfo volume_info;
  use_fvm_ = device_->VolumeGetInfo(&manager_info, &volume_info) == ZX_OK;
  if (use_fvm_) {
    size_t free = manager_info.slice_count - manager_info.assigned_slice_count;
    max_slice_count_ =
        std::min(free + volume_info.partition_slice_count, manager_info.maximum_slice_count);
    if (volume_info.slice_limit) {
      max_slice_count_ = std::min(volume_info.slice_limit, max_slice_count_);
    }
    slice_size_ = manager_info.slice_size;
    current_slice_count_ = volume_info.partition_slice_count;
    FX_LOGS(INFO) << "bcache has been created: " << max_blocks_ << " blocks(" << block_size_
                  << "B), " << current_slice_count_ << "/" << max_slice_count_ << " slices("
                  << slice_size_ << "B)";
  }
}

zx_status_t Bcache::BlockAttachVmo(const zx::vmo& vmo, storage::Vmoid* out) {
  return GetDevice()->BlockAttachVmo(vmo, out);
}

zx_status_t Bcache::BlockDetachVmo(storage::Vmoid vmoid) {
  return GetDevice()->BlockDetachVmo(std::move(vmoid));
}

zx::result<std::unique_ptr<Bcache>> Bcache::Create(
    std::unique_ptr<block_client::BlockDevice> device, uint64_t max_blocks, block_t block_size) {
  std::unique_ptr<Bcache> bcache(new Bcache(std::move(device), max_blocks, block_size));
  if (zx_status_t status = bcache->GetDevice()->BlockGetInfo(&bcache->info_); status != ZX_OK) {
    FX_LOGS(ERROR) << "cannot get block device information: " << status;
    return zx::error(status);
  }
  return zx::ok(std::move(bcache));
}

zx_status_t Bcache::RunRequests(
    const std::vector<storage::BufferedOperation>& buffered_operations) {
  if (use_fvm_) {
    std::vector<storage::BufferedOperation> fvm_buffered_operations;
    size_t required_count = 0;
    for (const auto& buffered_operation : buffered_operations) {
      fvm_buffered_operations.emplace_back(buffered_operation);
      auto& operation = fvm_buffered_operations.back().op;

      const bool is_read = operation.type == storage::OperationType::kRead;
      const bool is_write = operation.type == storage::OperationType::kWrite;
      const bool is_trim = operation.type == storage::OperationType::kTrim;
      if (!(is_read | is_write | is_trim)) {
        continue;
      }
      const size_t last_slice =
          (operation.dev_offset + operation.length - 1) * block_size_ / slice_size_;
      if (last_slice < current_slice_count_) {
        continue;
      }
      if (is_write || is_read) {
        required_count = std::max(required_count, last_slice + 1 - current_slice_count_);
      } else if (is_trim) {
        const size_t start_slice = operation.dev_offset * block_size_ / slice_size_;
        // Skip purging for unallocated regions.
        if (start_slice >= current_slice_count_) {
          fvm_buffered_operations.pop_back();
          continue;
        }
        operation.length =
            (current_slice_count_ * slice_size_ / block_size_) - operation.dev_offset;
      }
    }
    if (required_count) {
      if (zx_status_t status = device_->VolumeExtend(current_slice_count_, required_count);
          status != ZX_OK) {
        FX_LOGS(ERROR) << "failed to extend volume to (" << current_slice_count_ << ", "
                       << required_count - current_slice_count_ << ") "
                       << zx_status_get_string(status);
        return status;
      }
      current_slice_count_ += required_count;
    }
    return DeviceTransactionHandler::RunRequests(fvm_buffered_operations);
  }
  return DeviceTransactionHandler::RunRequests(buffered_operations);
}

zx_status_t BcacheMapper::Readblk(block_t bno, void* data) {
  if (bno >= Maxblk()) {
    return ZX_ERR_OUT_OF_RANGE;
  }

  std::lock_guard lock(buffer_mutex_);
  zx_status_t status =
      RunRequests({storage::BufferedOperation{.vmoid = buffer_.vmoid(),
                                              .op = {.type = storage::OperationType::kRead,
                                                     .vmo_offset = 0,
                                                     .dev_offset = bno,
                                                     .length = 1}}});
  if (status != ZX_OK) {
    return status;
  }
  std::memcpy(data, buffer_.Data(0), BlockSize());
  return ZX_OK;
}

zx_status_t BcacheMapper::Writeblk(block_t bno, const void* data) {
  if (bno >= Maxblk()) {
    return ZX_ERR_OUT_OF_RANGE;
  }

  std::lock_guard lock(buffer_mutex_);
  std::memcpy(buffer_.Data(0), data, BlockSize());
  return RunRequests({storage::BufferedOperation{.vmoid = buffer_.vmoid(),
                                                 .op = {.type = storage::OperationType::kWrite,
                                                        .vmo_offset = 0,
                                                        .dev_offset = bno,
                                                        .length = 1}}});
}

zx_status_t BcacheMapper::Trim(size_t start, size_t num) {
  if (!(info_.flags & fuchsia_storage_block::wire::DeviceFlag::kTrimSupport)) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  return RunRequests({storage::BufferedOperation{.vmoid = BLOCK_VMOID_INVALID,
                                                 .op = {
                                                     .type = storage::OperationType::kTrim,
                                                     .vmo_offset = 0,
                                                     .dev_offset = start,
                                                     .length = num,
                                                 }}});
}

zx_status_t BcacheMapper::Flush() {
  for (auto& bcache : bcaches_) {
    if (auto err = bcache->Flush(); err != ZX_OK) {
      return err;
    }
  }
  return ZX_OK;
}

zx::result<std::unique_ptr<BcacheMapper>> BcacheMapper::Create(
    std::vector<std::unique_ptr<Bcache>> bcaches) {
  uint64_t total_block_count = 0;
  for (auto& bcache : bcaches) {
    total_block_count += bcache->Maxblk();
  }

  auto bcache = std::unique_ptr<BcacheMapper>(
      new BcacheMapper(std::move(bcaches), total_block_count, kBlockSize));
  return zx::ok(std::move(bcache));
}

zx_status_t BcacheMapper::RunRequests(const std::vector<storage::BufferedOperation>& operations) {
  std::vector<std::vector<storage::BufferedOperation>> new_operations(bcaches_.size());
  for (const auto& operation : operations) {
    if (operation.op.dev_offset + operation.op.length > Maxblk()) {
      return ZX_ERR_OUT_OF_RANGE;
    }

    uint64_t residual = operation.op.length;
    uint64_t offset = operation.op.dev_offset;

    for (size_t i = 0; i < bcaches_.size(); ++i) {
      if (offset >= bcaches_[i]->Maxblk()) {
        offset -= bcaches_[i]->Maxblk();
        continue;
      }

      uint64_t length;
      if (residual <= bcaches_[i]->Maxblk() - offset) {
        length = residual;
      } else {
        length = bcaches_[i]->Maxblk() - offset;
      }

      vmoid_t vmoid;
      if (operation.vmoid == BLOCK_VMOID_INVALID) {
        if (operation.op.type != storage::OperationType::kTrim) {
          return ZX_ERR_INVALID_ARGS;
        }
        vmoid = BLOCK_VMOID_INVALID;
      } else {
        vmoid = vmoid_tree_[operation.vmoid][i].get();
      }

      new_operations[i].push_back(storage::BufferedOperation{
          .vmoid = vmoid,
          .op = storage::Operation{
              .type = operation.op.type,
              .vmo_offset = operation.op.vmo_offset + operation.op.length - residual,
              .dev_offset = offset,
              .length = length,
              .trace_flow_id = operation.op.trace_flow_id,
          }});

      residual -= length;
      offset = 0;
      if (residual == 0) {
        break;
      }
    }
  }

  for (size_t i = 0; i < bcaches_.size(); ++i) {
    if (new_operations[i].empty()) {
      continue;
    }

    if (auto err = bcaches_[i]->RunRequests(new_operations[i]); err != ZX_OK) {
      return err;
    }
  }
  return ZX_OK;
}

zx::result<vmoid_t> BcacheMapper::FindFreeVmoId() {
  for (vmoid_t i = last_id_; i < std::numeric_limits<vmoid_t>::max(); ++i) {
    if (vmoid_tree_.find(i) == vmoid_tree_.end()) {
      last_id_ = static_cast<vmoid_t>(i + 1);
      return zx::ok(i);
    }
  }
  for (vmoid_t i = BLOCK_VMOID_INVALID + 1; i < last_id_; ++i) {
    if (vmoid_tree_.find(i) == vmoid_tree_.end()) {
      last_id_ = static_cast<vmoid_t>(i + 1);
      return zx::ok(i);
    }
  }
  return zx::error(ZX_ERR_NO_RESOURCES);
}

zx_status_t BcacheMapper::BlockAttachVmo(const zx::vmo& vmo, storage::Vmoid* out) {
  zx::result<vmoid_t> vmoid = FindFreeVmoId();
  if (vmoid.is_error()) {
    return vmoid.error_value();
  }

  std::vector<storage::Vmoid> new_vmoids;
  auto cleanup = fit::defer([&] {
    for (size_t i = 0; i < new_vmoids.size(); ++i) {
      bcaches_[i]->BlockDetachVmo(std::move(new_vmoids[i]));
    }
  });

  for (auto& bcache : bcaches_) {
    storage::Vmoid vmoid;
    if (auto ret = bcache->BlockAttachVmo(vmo, &vmoid); ret != ZX_OK) {
      return ret;
    }
    new_vmoids.push_back(std::move(vmoid));
  }

  vmoid_tree_.insert(std::make_pair(vmoid.value(), std::move(new_vmoids)));
  *out = storage::Vmoid(vmoid.value());
  cleanup.cancel();
  return ZX_OK;
}

zx_status_t BcacheMapper::BlockDetachVmo(storage::Vmoid vmoid) {
  auto it = vmoid_tree_.find(vmoid.get());
  if (it == vmoid_tree_.end()) {
    return ZX_ERR_NOT_FOUND;
  }

  ZX_DEBUG_ASSERT(it->second.size() == bcaches_.size());
  auto& vmoids = it->second;
  for (size_t i = 0; i < bcaches_.size(); ++i) {
    if (auto ret = bcaches_[i]->BlockDetachVmo(std::move(vmoids[i])); ret != ZX_OK) {
      return ret;
    }
  }

  vmoid_tree_.erase(it);
  [[maybe_unused]] auto leak = vmoid.TakeId();
  return ZX_OK;
}

BcacheMapper::BcacheMapper(std::vector<std::unique_ptr<Bcache>> bcaches, uint64_t max_blocks,
                           block_t block_size)
    : bcaches_(std::move(bcaches)), block_size_(block_size), max_blocks_(max_blocks) {
  uint32_t transfer_size = fuchsia_storage_block::wire::kMaxTransferUnbounded;
  uint32_t max_block_size = 0;
  fuchsia_storage_block::wire::DeviceFlag flag =
      fuchsia_storage_block::wire::DeviceFlag::kRemovable |
      fuchsia_storage_block::wire::DeviceFlag::kTrimSupport |
      fuchsia_storage_block::wire::DeviceFlag::kFuaSupport;
  for (auto& bcache : bcaches_) {
    fuchsia_storage_block::wire::BlockInfo info;
    bcache->GetDevice()->BlockGetInfo(&info);

    if (max_block_size < info.block_size) {
      max_block_size = info.block_size;
    }

    if (transfer_size > info.max_transfer_size) {
      transfer_size = info.max_transfer_size;
    }

    if (info.flags & fuchsia_storage_block::wire::DeviceFlag::kReadonly) {
      flag |= fuchsia_storage_block::wire::DeviceFlag::kReadonly;
      read_only_ = true;
    }
    if (!(info.flags & fuchsia_storage_block::wire::DeviceFlag::kRemovable)) {
      flag &= (~fuchsia_storage_block::wire::DeviceFlag::kRemovable);
    }
    if (!(info.flags & fuchsia_storage_block::wire::DeviceFlag::kTrimSupport)) {
      flag &= (~fuchsia_storage_block::wire::DeviceFlag::kTrimSupport);
    }
    if (!(info.flags & fuchsia_storage_block::wire::DeviceFlag::kFuaSupport)) {
      flag &= (~fuchsia_storage_block::wire::DeviceFlag::kFuaSupport);
    }
  }
  uint64_t device_block_count = max_blocks * block_size / max_block_size;

  info_ = {
      .block_count = device_block_count,
      .block_size = max_block_size,
      .max_transfer_size = transfer_size,
      .flags = flag,
  };
  buffer_.Initialize(this, 1, info_.block_size, "scratch-block");
}

zx_status_t BcacheMapper::BlockGetInfo(fuchsia_storage_block::wire::BlockInfo* out_info) const {
  *out_info = info_;
  return ZX_OK;
}

}  // namespace f2fs
