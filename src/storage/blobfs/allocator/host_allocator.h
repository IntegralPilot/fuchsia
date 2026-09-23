// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_BLOBFS_ALLOCATOR_HOST_ALLOCATOR_H_
#define SRC_STORAGE_BLOBFS_ALLOCATOR_HOST_ALLOCATOR_H_

#include <lib/zx/result.h>
#include <zircon/errors.h>

#include <cstdint>
#include <memory>
#include <span>

#include <id_allocator/id_allocator.h>

#include "src/storage/blobfs/allocator/base_allocator.h"
#include "src/storage/blobfs/common.h"
#include "src/storage/blobfs/format.h"
#include "src/storage/blobfs/node_finder.h"

namespace blobfs {

// A simple allocator for manipulating node and block allocations in blobfs images on a host device.
class HostAllocator : public BaseAllocator {
 public:
  // Does not take ownership of |block_bitmap|.
  static zx::result<std::unique_ptr<HostAllocator>> Create(RawBitmap block_bitmap,
                                                           std::span<Inode> node_map);

  // blobfs::NodeFinder interface.
  zx::result<InodePtr> GetNode(uint32_t node_index) final;

  void* GetBlockBitmapData();

 protected:
  // blobfs::BaseAllocator interface.
  zx::result<> AddBlocks(uint64_t block_count) final { return zx::error(ZX_ERR_NOT_SUPPORTED); }
  zx::result<> AddNodes() final { return zx::error(ZX_ERR_NOT_SUPPORTED); }

 private:
  HostAllocator(RawBitmap block_bitmap, std::span<Inode> node_map,
                std::unique_ptr<id_allocator::IdAllocator> node_bitmap);

  std::span<Inode> node_map_;
};

}  // namespace blobfs

#endif  // SRC_STORAGE_BLOBFS_ALLOCATOR_HOST_ALLOCATOR_H_
