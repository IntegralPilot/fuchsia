// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/blobfs/allocator/allocator.h"

#include <inttypes.h>
#include <lib/fzl/resizeable-vmo-mapper.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <zircon/assert.h>
#include <zircon/compiler.h>
#include <zircon/errors.h>
#include <zircon/syscalls.h>
#include <zircon/types.h>

#include <cstddef>
#include <cstdint>
#include <memory>
#include <mutex>
#include <utility>
#include <vector>

#include <fbl/algorithm.h>
#include <id_allocator/id_allocator.h>

#include "src/storage/blobfs/allocator/base_allocator.h"
#include "src/storage/blobfs/allocator/node_reserver.h"
#include "src/storage/blobfs/common.h"
#include "src/storage/blobfs/format.h"
#include "src/storage/blobfs/node_finder.h"
#include "src/storage/lib/buffer/owned_vmoid.h"
#include "src/storage/lib/operation/operation.h"
#include "src/storage/lib/trace/trace.h"
#include "src/storage/lib/vfs/cpp/transaction/device_transaction_handler.h"

namespace blobfs {

Allocator::Allocator(SpaceManager* space_manager, RawBitmap block_map,
                     fzl::ResizeableVmoMapper node_map,
                     std::unique_ptr<id_allocator::IdAllocator> node_bitmap)
    : BaseAllocator(std::move(block_map), std::move(node_bitmap)),
      space_manager_(space_manager),
      node_map_(std::move(node_map)) {}

zx::result<InodePtr> Allocator::GetNode(uint32_t node_index) __TA_NO_THREAD_SAFETY_ANALYSIS {
  if (node_index >= node_map_.size() / kBlobfsInodeSize) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  // TODO(https://fxbug.dev/42160756): Calling lock_shared from a thread that already holds the lock
  // is undefined behaviour.
  node_map_grow_mutex_.lock_shared();
  return zx::ok(
      InodePtr(&reinterpret_cast<Inode*>(node_map_.start())[node_index], InodePtrDeleter(this)));
}

zx_status_t Allocator::ResetFromStorage(fs::DeviceTransactionHandler& transaction_handler) {
  ZX_DEBUG_ASSERT(ReservedBlockCount() == 0);
  ZX_DEBUG_ASSERT(ReservedNodeCount() == 0);

  // Ensure the block and node maps are up-to-date with changes in size that
  // might have happened.
  if (zx_status_t status = ResetBlockMapSize(); status != ZX_OK) {
    return status;
  }
  if (zx_status_t status = ResetNodeMapSize(); status != ZX_OK) {
    return status;
  }

  const auto info = space_manager_->Info();
  uint64_t block_map_blocks = BlockMapBlocks(info);
  uint64_t node_map_blocks = NodeMapBlocks(info);
  uint64_t block_map_size = block_map_blocks * kBlobfsBlockSize;
  uint64_t node_map_size = node_map_blocks * kBlobfsBlockSize;

  // The block map VMO and node map VMO are both resizable VMOs which can't be registered with the
  // block device. A new VMO is created and registered to read in the block map and node map blocks.
  // The pages are then transferred from the transfer VMO to the block map and node map VMOs.
  zx::vmo transfer_vmo;
  if (zx_status_t status = zx::vmo::create(block_map_size + node_map_size, 0, &transfer_vmo);
      status != ZX_OK) {
    return status;
  }

  storage::OwnedVmoid transfer_vmo_vmoid;
  if (zx_status_t status = space_manager_->BlockAttachVmo(
          transfer_vmo, &transfer_vmo_vmoid.GetReference(space_manager_));
      status != ZX_OK) {
    return status;
  }

  std::vector<storage::BufferedOperation> operations;
  operations.push_back({
      .vmoid = transfer_vmo_vmoid.get(),
      .op =
          {
              .type = storage::OperationType::kRead,
              .vmo_offset = 0,
              .dev_offset = BlockMapStartBlock(info),
              .length = block_map_blocks,
          },
  });
  operations.push_back({
      .vmoid = transfer_vmo_vmoid.get(),
      .op =
          {
              .type = storage::OperationType::kRead,
              .vmo_offset = block_map_blocks,
              .dev_offset = NodeMapStartBlock(info),
              .length = node_map_blocks,
          },
  });

  if (zx_status_t status = transaction_handler.RunRequests(operations); status != ZX_OK) {
    return status;
  }

  if (zx_status_t status = GetBlockMapVmo().transfer_data(0, 0, block_map_size, &transfer_vmo, 0);
      status != ZX_OK) {
    return status;
  }
  if (zx_status_t status =
          GetNodeMapVmo().transfer_data(0, 0, node_map_size, &transfer_vmo, block_map_size);
      status != ZX_OK) {
    return status;
  }

  return ZX_OK;
}

const zx::vmo& Allocator::GetBlockMapVmo() const {
  return GetBlockBitmap().StorageUnsafe()->GetVmo();
}

const zx::vmo& Allocator::GetNodeMapVmo() const { return node_map_.vmo(); }

zx::result<ReservedNode> Allocator::ReserveNode() {
  TRACE_DURATION("blobfs", "ReserveNode");
  return BaseAllocator::ReserveNode();
}

zx_status_t Allocator::ResetBlockMapSize() {
  ZX_DEBUG_ASSERT(ReservedBlockCount() == 0);
  const auto info = space_manager_->Info();
  uint64_t new_size = info.data_block_count;
  RawBitmap& block_bitmap = GetBlockBitmap();
  if (new_size != block_bitmap.size()) {
    uint64_t rounded_size = BlockMapBlocks(info) * kBlobfsBlockBits;
    if (zx_status_t status = block_bitmap.Reset(rounded_size); status != ZX_OK) {
      return status;
    }

    if (new_size < rounded_size) {
      // In the event that the requested block count is not a multiple of the nearest block size,
      // shrink down to the actual block count.
      if (zx_status_t status = block_bitmap.Shrink(new_size); status != ZX_OK) {
        return status;
      }
    }
  }
  return ZX_OK;
}

zx_status_t Allocator::ResetNodeMapSize() {
  ZX_DEBUG_ASSERT(ReservedNodeCount() == 0);
  const auto info = space_manager_->Info();
  uint64_t nodemap_size = kBlobfsInodeSize * info.inode_count;
  zx_status_t status = ZX_OK;
  if (fbl::round_up(nodemap_size, kBlobfsBlockSize) != nodemap_size) {
    return ZX_ERR_BAD_STATE;
  }
  ZX_DEBUG_ASSERT(nodemap_size / kBlobfsBlockSize == NodeMapBlocks(info));

  if (nodemap_size > node_map_.size()) {
    status = GrowNodeMap(nodemap_size);
  } else if (nodemap_size < node_map_.size()) {
    // It is safe to shrink node_map_ without a lock because the mapping won't change in that case.
    status = node_map_.Shrink(nodemap_size);
  }
  if (status != ZX_OK) {
    return status;
  }
  return GetNodeBitmap().Reset(info.inode_count);
}

void Allocator::LogAllocationFailure(uint64_t num_blocks) const {
  const Superblock& info = space_manager_->Info();
  const uint64_t requested_bytes = num_blocks * info.block_size;
  const uint64_t total_bytes = info.data_block_count * info.block_size;
  const uint64_t persisted_used_bytes = info.alloc_block_count * info.block_size;
  const uint64_t pending_used_bytes = ReservedBlockCount() * info.block_size;
  const uint64_t used_bytes = persisted_used_bytes + pending_used_bytes;
  ZX_ASSERT_MSG(used_bytes <= total_bytes,
                "blobfs using more bytes than available: %" PRIu64 " > %" PRIu64 "\n", used_bytes,
                total_bytes);
  const uint64_t free_bytes = total_bytes - used_bytes;

  if (!log_allocation_failure_) {
    return;
  }

  static OutOfSpaceLogSite site;
  if (site.ShouldLog()) {
    FX_LOGS(ERROR) << "Blobfs has run out of space on persistent storage.";
    FX_LOGS(ERROR) << "    Could not allocate " << requested_bytes << " bytes";
    FX_LOGS(ERROR) << "    Total data bytes  : " << total_bytes;
    FX_LOGS(ERROR) << "    Used data bytes   : " << persisted_used_bytes;
    FX_LOGS(ERROR) << "    Preallocated bytes: " << pending_used_bytes;
    FX_LOGS(ERROR) << "    Free data bytes   : " << free_bytes;
    FX_LOGS(ERROR) << "    This allocation failure is the result of "
                   << (requested_bytes <= free_bytes ? "fragmentation" : "over-allocation");
  }
}

zx_status_t Allocator::GrowNodeMap(size_t size) {
  std::scoped_lock lock(node_map_grow_mutex_);
  return node_map_.Grow(size);
}

// TODO(https://fxbug.dev/42080556): change this to __TA_RELEASE_SHARED(node_map_grow_mutex_) after
// clang roll.
void Allocator::DropInodePtr() __TA_NO_THREAD_SAFETY_ANALYSIS {
  node_map_grow_mutex_.unlock_shared();
}

zx::result<> Allocator::AddBlocks(uint64_t block_count) {
  if (zx_status_t status = space_manager_->AddBlocks(block_count, &GetBlockBitmap());
      status != ZX_OK) {
    LogAllocationFailure(block_count);
    return zx::make_result(status);
  }
  return zx::ok();
}

zx::result<> Allocator::AddNodes() {
  zx_status_t status = space_manager_->AddInodes(this);
  if (status != ZX_OK) {
    return zx::make_result(status);
  }

  auto inode_count = space_manager_->Info().inode_count;
  status = GetNodeBitmap().Grow(inode_count);
  // This is an awkward situation where we could secure storage but potentially ran out of [virtual]
  // memory. There is nothing much we can do. The filesystem might fail soon from other alloc
  // failures. It is better to turn the fs-mount into read-only instance or panic to safe-guard
  // against any damage rather than propagating these errors.
  //
  // One alternative considered was to reorder memory allocation first and then allocate disk.
  // Reordering just delays the problem and also to reorder this layer needs to know details like
  // what is fvm slice size is. We decided against that route.
  if (status != ZX_OK) {
    FX_LOGS(ERROR) << "Failed to grow bitmap for inodes";
  }
  return zx::make_result(status);
}

void Allocator::Decommit() {
  const auto& bitmap = GetNodeBitmap();
  const size_t blobs_per_page = zx_system_get_page_size() / sizeof(Inode);
  size_t start = 0, count = 0;
  size_t decommit_total = 0;
  for (;;) {
    if (start + count >= bitmap.Size() || bitmap.IsBusy(start + count)) {
      if (start + count >= bitmap.Size()) {
        // Round up at the end.
        count += blobs_per_page - 1;
      }
      count -= count % blobs_per_page;
      if (count >= blobs_per_page) {
        const uint64_t size = sizeof(Inode) * count;
        if (zx_status_t status = node_map_.vmo().op_range(ZX_VMO_OP_DECOMMIT, start * sizeof(Inode),
                                                          size, nullptr, 0);
            status != ZX_OK) {
          FX_PLOGS(WARNING, status) << "Failed to decommit unused node map";
          return;
        }
        decommit_total += size;
      }

      start += count + blobs_per_page;
      if (start >= bitmap.Size())
        break;
      count = 0;
    } else {
      ++count;
    }
  }
  if (decommit_total > 0) {
    FX_LOGS(DEBUG) << "Decommitted " << decommit_total << " bytes from the node map";
  }
}

}  // namespace blobfs
