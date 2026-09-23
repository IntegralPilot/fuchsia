// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/f2fs/bitmap.h"

#include <lib/syslog/cpp/macros.h>

#include "src/storage/f2fs/file_cache.h"

namespace f2fs {

size_t CountBits(const RawBitmap &bits, size_t offset, size_t len) {
  if (offset >= bits.size())
    return 0;
  len = std::min(len, bits.size() - offset);
  constexpr size_t kNumBits = kBitsPerByte * sizeof(uint64_t);
  const uint64_t *raw_bits = static_cast<const uint64_t *>(bits.StorageUnsafe()->GetData());
  uint64_t sum = 0;
  uint64_t end = CheckedDivRoundUp(offset + len, kNumBits);
  for (uint64_t i = offset / kNumBits; i < end; ++i) {
    std::bitset<kNumBits> node{raw_bits[i]};
    sum += node.count();
  }
  return sum;
}

PageBitmap::PageBitmap(void *bitmap, size_t size)
    : bits_(static_cast<uint8_t *>(bitmap)), size_(size) {}

PageBitmap::PageBitmap(fbl::RefPtr<Page> page, void *bitmap, size_t size)
    : page_(std::move(page)), bits_(static_cast<uint8_t *>(bitmap)), size_(size) {
  ZX_DEBUG_ASSERT(bitmap >= page_->GetAddress());
  ZX_DEBUG_ASSERT(static_cast<uint8_t *>(bitmap) + CheckedDivRoundUp<size_t>(size, kBitsPerByte) <=
                  &page_->GetAddress<uint8_t>()[page_->Size()]);
}

bool PageBitmap::Set(size_t pos) {
  if (pos >= size_) {
    return false;
  }
  uint8_t &node = GetNode(pos);
  const uint8_t mask = GetMask(uint8_t(1U), pos & kLastNodeMask);
  bool was_set = (node & mask) != 0;
  node |= mask;
  return was_set;
}

bool PageBitmap::Clear(size_t pos) {
  if (pos >= size_) {
    return false;
  }
  uint8_t &node = GetNode(pos);
  const uint8_t mask = GetMask(uint8_t(1U), pos & kLastNodeMask);
  bool was_set = (node & mask) != 0;
  node &= ~mask;
  return was_set;
}

bool PageBitmap::Test(size_t pos) const {
  if (pos >= size_) {
    return false;
  }
  return (GetNode(pos) & GetMask(uint8_t(1U), pos & kLastNodeMask)) != 0;
}

size_t PageBitmap::FindNextZeroBit(size_t pos, size_t max_pos) const {
  max_pos = std::min(max_pos, size_);
  size_t offset_in_iter = pos & kLastNodeMask;
  while (pos < max_pos) {
    const uint8_t mask = GetMask(uint8_t(0xFFU), offset_in_iter);
    const uint8_t &node = GetNode(pos);
    if ((node & mask) != mask) {  // found
      for (pos -= offset_in_iter; offset_in_iter < kBitsPerByte; ++offset_in_iter) {
        if (!(node & GetMask(1U, offset_in_iter))) {
          return std::min(pos + offset_in_iter, max_pos);
        }
      }
    }
    pos = pos + kBitsPerByte - offset_in_iter;
    offset_in_iter = 0;
  }
  return max_pos;
}

size_t PageBitmap::FindNextBit(size_t pos, size_t max_pos) const {
  max_pos = std::min(max_pos, size_);
  size_t offset_in_iter = pos & kLastNodeMask;
  while (pos < max_pos) {
    const uint8_t &node = GetNode(pos);
    if (node & GetMask(uint8_t(0xFFU), offset_in_iter)) {  // found
      for (pos -= offset_in_iter; offset_in_iter < kBitsPerByte; ++offset_in_iter) {
        if (node & GetMask(1U, offset_in_iter)) {
          return std::min(pos + offset_in_iter, max_pos);
        }
      }
    }
    pos += kBitsPerByte - offset_in_iter;
    offset_in_iter = 0;
  }
  return max_pos;
}

size_t PageBitmap::GetSize() const { return size_; }

}  // namespace f2fs
