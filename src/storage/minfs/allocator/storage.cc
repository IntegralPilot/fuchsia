// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/minfs/allocator/storage.h"

#include <lib/syslog/cpp/macros.h>

#include <cstdint>
#include <utility>

#include "src/storage/lib/trace/trace.h"
#include "src/storage/minfs/allocator/allocator.h"
#include "src/storage/minfs/format.h"

namespace minfs {

// On the stress builders, the out of space log messages can take up a lot of space, so we only log
// every 60 seconds.
class OutOfSpaceLogSite {
 public:
  bool ShouldLog() {
#ifdef __Fuchsia__
    zx::ticks now = zx::ticks::now();
    if (now < ignore_before_)
      return false;
    zx::ticks ignore_duration = zx::ticks::per_second();
    ignore_duration *= 60;  // 60 seconds
    ignore_before_ = now + ignore_duration;
#endif
    return true;
  }

#ifdef __Fuchsia__
 private:
  zx::ticks ignore_before_;
#endif
};

PersistentStorage::PersistentStorage(block_client::BlockDevice* device, SuperblockManager* sb,
                                     size_t unit_size, GrowHandler grow_cb,
                                     AllocatorMetadata metadata, uint32_t block_size)
    : device_(device),
      unit_size_(unit_size),
      sb_(sb),
      grow_cb_(std::move(grow_cb)),
      metadata_(std::move(metadata)),
      block_size_(block_size) {}

zx::result<> PersistentStorage::AttachVmo(const zx::vmo& vmo, storage::OwnedVmoid* out) {
  return zx::make_result(device_->BlockAttachVmo(vmo, &out->GetReference(device_)));
}

zx::result<> PersistentStorage::Extend(PendingWork* write_transaction, WriteData data,
                                       GrowMapCallback grow_map) {
  TRACE_DURATION("minfs", "Minfs::PersistentStorage::Extend");
  ZX_DEBUG_ASSERT(write_transaction != nullptr);
  if (!metadata_.UsingFvm()) {
    FX_LOGS_FIRST_N(WARNING, 10) << "PersistentStorage::Extent can't extend on non-FVM devices.";
    return zx::error(ZX_ERR_NO_SPACE);
  }
  uint32_t data_slices_diff = 1;

  // Determine if we will have enough space in the bitmap slice
  // to grow |data_slices_diff| data slices.

  // How large is the bitmap right now?
  uint32_t bitmap_slices = metadata_.Fvm().MetadataSlices();
  uint32_t bitmap_blocks = metadata_.Fvm().UnitsPerSlices(bitmap_slices, BlockSize());

  // How large does the bitmap need to be?
  uint32_t data_slices = metadata_.Fvm().DataSlices();
  uint32_t data_slices_new = data_slices + data_slices_diff;

  uint32_t pool_size =
      metadata_.Fvm().UnitsPerSlices(data_slices_new, static_cast<uint32_t>(unit_size_));
  uint32_t bitmap_blocks_new = BitmapBlocksForSize(pool_size);

  if (bitmap_blocks_new > bitmap_blocks) {
    FX_LOGS(ERROR) << "Minfs allocator needs to increase bitmap size";
    return zx::error(ZX_ERR_NO_SPACE);
  }

  // Make the request to the FVM.
  uint64_t length = data_slices_diff;
  uint64_t offset = metadata_.Fvm().BlocksToSlices(metadata_.DataStartBlock()) + data_slices;

  zx_status_t status = device_->VolumeExtend(offset, length);
  if (status != ZX_OK) {
    static OutOfSpaceLogSite site;
    if (site.ShouldLog()) {
      FX_LOGS(WARNING) << "Failed to extend volume from " << data_slices << " slices to "
                       << data_slices_new << " slices, error " << status;
    }
    return zx::error(status);
  }

  if (grow_cb_) {
    if ((status = grow_cb_(pool_size)) != ZX_OK) {
      FX_LOGS(ERROR) << "Allocator grow callback failure: " << status;
      return zx::error(status);
    }
  }

  // Extend the in memory representation of our allocation pool -- it grew!
  auto old_pool_size_or = grow_map(pool_size);
  if (old_pool_size_or.is_error()) {
    return old_pool_size_or.take_error();
  }

  metadata_.Fvm().SetDataSlices(data_slices_new);
  metadata_.SetPoolTotal(pool_size);
  sb_->Write(write_transaction, UpdateBackupSuperblock::kUpdate);

  // Update the block bitmap.
  PersistRange(write_transaction, data, old_pool_size_or.value(),
               pool_size - old_pool_size_or.value());
  return zx::ok();
}

}  // namespace minfs
