// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/edid/internal/iterators.h"

#include <iterator>

#include "src/graphics/display/lib/edid/edid.h"

namespace edid::internal {

descriptor_iterator& descriptor_iterator::operator++() {
  if (!edid_) {
    return *this;
  }

  if (block_idx_ == 0) {
    descriptor_idx_++;

    if (descriptor_idx_ < std::size(edid_->base_edid().detailed_descriptors)) {
      descriptor_ = edid_->base_edid().detailed_descriptors + descriptor_idx_;
      if (descriptor_->timing.pixel_clock_10khz != 0 || descriptor_->monitor.type != 0x10) {
        return *this;
      }
    }

    block_idx_++;
    descriptor_idx_ = UINT32_MAX;
  }

  const int num_blocks = static_cast<int>(edid_->bytes_.size() / kBlockSize);
  while (block_idx_ < num_blocks) {
    auto cea_extn_block = edid_->GetBlock<CeaEdidTimingExtension>(block_idx_);
    size_t offset = sizeof(CeaEdidTimingExtension::payload);
    if (cea_extn_block &&
        cea_extn_block->dtd_start_idx > offsetof(CeaEdidTimingExtension, payload)) {
      offset = cea_extn_block->dtd_start_idx - offsetof(CeaEdidTimingExtension, payload);
    }

    descriptor_idx_++;
    offset += sizeof(Descriptor) * descriptor_idx_;

    // Return if the descriptor is within bounds and either a timing descriptor or not
    // a placeholder monitor descriptor, otherwise advance to the next block
    if (offset + sizeof(DetailedTimingDescriptor) <= sizeof(CeaEdidTimingExtension::payload)) {
      descriptor_ = reinterpret_cast<const Descriptor*>(cea_extn_block->payload + offset);
      if (descriptor_->timing.pixel_clock_10khz != 0 ||
          descriptor_->monitor.type != Descriptor::Monitor::kDummyType) {
        return *this;
      }
    }

    block_idx_++;
    descriptor_idx_ = UINT32_MAX;
  }

  edid_ = nullptr;
  return *this;
}

data_block_iterator::data_block_iterator(const Edid* edid) : edid_(edid) {
  ++(*this);
  if (is_valid()) {
    cea_revision_ = edid_->GetBlock<CeaEdidTimingExtension>(block_idx_)->revision_number;
  }
}

data_block_iterator& data_block_iterator::operator++() {
  if (!edid_) {
    return *this;
  }

  const int num_blocks = static_cast<int>(edid_->bytes_.size() / kBlockSize);
  while (block_idx_ < num_blocks) {
    auto cea_extn_block = edid_->GetBlock<CeaEdidTimingExtension>(block_idx_);
    size_t dbc_end = 0;
    if (cea_extn_block &&
        cea_extn_block->dtd_start_idx > offsetof(CeaEdidTimingExtension, payload)) {
      dbc_end = cea_extn_block->dtd_start_idx - offsetof(CeaEdidTimingExtension, payload);
    }

    db_idx_++;
    uint32_t db_to_skip = db_idx_;

    uint32_t offset = 0;
    while (offset < dbc_end) {
      auto* dblk = reinterpret_cast<const DataBlock*>(cea_extn_block->payload + offset);
      if (db_to_skip == 0) {
        db_ = dblk;
        return *this;
      }
      db_to_skip--;
      offset += (dblk->length() + 1);  // length doesn't include the data block header byte
    }

    block_idx_++;
    db_idx_ = UINT32_MAX;
  }

  edid_ = nullptr;
  return *this;
}

}  // namespace edid::internal
