// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_EDID_INTERNAL_ITERATORS_H_
#define SRC_GRAPHICS_DISPLAY_LIB_EDID_INTERNAL_ITERATORS_H_

#include <cstdint>

namespace edid {

class Edid;
union Descriptor;
struct DataBlock;

namespace internal {

class descriptor_iterator {
 public:
  explicit descriptor_iterator(const Edid* edid) : edid_(edid) { ++(*this); }

  descriptor_iterator& operator++();
  bool is_valid() const { return edid_ != nullptr; }

  uint8_t block_idx() const { return block_idx_; }
  const Descriptor* operator->() const { return descriptor_; }
  const Descriptor* get() const { return descriptor_; }

 private:
  // Set to null when the iterator is exhausted.
  const Edid* edid_;
  // The block index in which we're looking for descriptors.
  uint8_t block_idx_ = 0;
  // The index of the current descriptor in the current block.
  uint32_t descriptor_idx_ = UINT32_MAX;

  const Descriptor* descriptor_;
};

class data_block_iterator {
 public:
  explicit data_block_iterator(const Edid* edid);

  data_block_iterator& operator++();
  bool is_valid() const { return edid_ != nullptr; }

  // Only valid if |is_valid()| is true
  uint8_t cea_revision() const { return cea_revision_; }

  const DataBlock* operator->() const { return db_; }

 private:
  // Set to null when the iterator is exhausted.
  const Edid* edid_;
  // The block index in which we're looking for descriptors. No dbs in the 1st block.
  uint8_t block_idx_ = 1;
  // The index of the current descriptor in the current block.
  uint32_t db_idx_ = UINT32_MAX;

  const DataBlock* db_;

  uint8_t cea_revision_;
};

}  // namespace internal

}  // namespace edid

#endif  // SRC_GRAPHICS_DISPLAY_LIB_EDID_INTERNAL_ITERATORS_H_
