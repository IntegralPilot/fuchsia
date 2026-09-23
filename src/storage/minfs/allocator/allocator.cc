// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/minfs/allocator/allocator.h"

#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <stdlib.h>
#include <string.h>
#include <zircon/assert.h>
#include <zircon/types.h>

#include <cstdint>
#include <mutex>
#include <utility>

#include <fbl/vector.h>

#include "src/storage/lib/block_protocol/types.h"
#include "src/storage/lib/buffer/block_buffer.h"
#include "src/storage/lib/buffer/owned_vmoid.h"
#include "src/storage/lib/vfs/cpp/transaction/buffered_operations_builder.h"
#include "src/storage/minfs/pending_work.h"

namespace minfs {

namespace {

// Trivial BlockBuffer that doesn't own the underlying buffer.
// TODO(https://fxbug.dev/42124749): Remove this.
class UnownedBuffer : public storage::BlockBuffer {
 public:
  explicit UnownedBuffer(vmoid_t vmoid) : vmoid_(vmoid) {}
  ~UnownedBuffer() {}

  // BlockBuffer interface:
  size_t capacity() const final { return 0; }
  uint32_t BlockSize() const final { return 0; }
  vmoid_t vmoid() const final { return vmoid_; }
  zx_handle_t Vmo() const final { return ZX_HANDLE_INVALID; }
  void* Data(size_t index) final { return nullptr; }
  const void* Data(size_t index) const final { return nullptr; }

 private:
  vmoid_t vmoid_;
};

}  // namespace

Allocator::~Allocator() {
  std::scoped_lock lock(lock_);
  ZX_ASSERT(pending_changes_.empty());
}

zx::result<> Allocator::LoadStorage(fs::BufferedOperationsBuilder* builder) {
  std::scoped_lock lock(lock_);
  storage::OwnedVmoid vmoid;
  auto status = storage_->AttachVmo(map_.StorageUnsafe()->vmo(), &vmoid);
  if (status.is_error()) {
    return status.take_error();
  }
  UnownedBuffer buffer(vmoid.get());
  builder->AddVmoid(std::move(vmoid));
  storage_->Load(builder, &buffer);
  return zx::ok();
}

size_t Allocator::GetAvailableLocked() const {
  size_t total_reserved = reserved_;
  for (const PendingChange* change : pending_changes_) {
    total_reserved += change->GetReservedCount();
  }
  ZX_DEBUG_ASSERT(storage_->PoolAvailable() >= total_reserved);
  return storage_->PoolAvailable() - total_reserved;
}

WriteData Allocator::GetMapDataLocked() { return map_.StorageUnsafe()->vmo().get(); }

fbl::Vector<BlockRegion> Allocator::GetAllocatedRegions() const {
  std::scoped_lock lock(lock_);
  fbl::Vector<BlockRegion> out_regions;
  uint64_t offset = 0;
  uint64_t end = 0;
  while (!map_.Scan(end, map_.size(), false, &offset)) {
    if (map_.Scan(offset, map_.size(), true, &end)) {
      end = map_.size();
    }
    out_regions.push_back({offset, end - offset});
  }
  return out_regions;
}

}  // namespace minfs
