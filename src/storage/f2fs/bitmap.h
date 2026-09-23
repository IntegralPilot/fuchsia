// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_F2FS_BITMAP_H_
#define SRC_STORAGE_F2FS_BITMAP_H_

#include <limits.h>
#include <stddef.h>

#include <bitmap/raw-bitmap.h>
#include <bitmap/storage.h>
#include <safemath/checked_math.h>

#include "src/storage/f2fs/common.h"

namespace f2fs {

constexpr size_t kLastNodeMask = 0x7;

using RawBitmap = bitmap::RawBitmapGeneric<bitmap::VmoStorage>;
using RawBitmapHeap = bitmap::RawBitmapGeneric<bitmap::DefaultStorage>;

class Page;
class PageBitmap {
 public:
  PageBitmap() = delete;
  PageBitmap(const PageBitmap &bits) = delete;
  PageBitmap &operator=(const PageBitmap &bits) = delete;
  PageBitmap &operator=(PageBitmap &&bits) = delete;

  PageBitmap(fbl::RefPtr<Page> page, void *bitmap, size_t size);
  PageBitmap(void *bitmap, size_t size);
  PageBitmap(PageBitmap &&bits) = default;

  bool Set(size_t pos);
  bool Clear(size_t pos);
  bool Test(size_t pos) const;
  size_t GetSize() const;

  size_t FindNextZeroBit(size_t pos) const { return FindNextZeroBit(pos, size_); }
  size_t FindNextZeroBit(size_t pos, size_t max_pos) const;
  size_t FindNextBit(size_t pos) const { return FindNextBit(pos, size_); }
  size_t FindNextBit(size_t pos, size_t max_pos) const;

 private:
  uint8_t &GetNode(size_t pos) { return bits_[pos >> kShiftForBitSize]; }
  const uint8_t &GetNode(size_t pos) const { return bits_[pos >> kShiftForBitSize]; }

  fbl::RefPtr<Page> page_;
  uint8_t *const bits_ = nullptr;
  const size_t size_ = 0;
};

template <typename T = uint8_t>
inline T GetMask(const T value, const size_t offset) {
  return static_cast<T>(value << offset);
}

inline size_t GetBitSize(size_t size_in_byte) {
  return safemath::CheckLsh(size_in_byte, kShiftForBitSize).ValueOrDie();
}

inline size_t GetByteSize(size_t num_bits) {
  return CheckedDivRoundUp<size_t>(num_bits, kBitsPerByte);
}

inline size_t ToMsbFirst(size_t offset) {
  size_t index_in_last_node = offset & kLastNodeMask;
  return offset + 7 - 2 * index_in_last_node;
}

// It copies bits from [0, len) of |src| to [offset, offset + len) of |dst|.
// |offset| + |len| and |len| must be multiples of 8.
inline zx_status_t bitcpy(uint8_t *dst, const uint8_t *src, size_t offset, size_t len) {
  size_t end = offset + len;
  size_t start_in_byte = offset / kBitsPerByte;
  size_t end_in_byte = GetByteSize(end);
  if (offset & kLastNodeMask || end & kLastNodeMask || end_in_byte <= start_in_byte) {
    return ZX_ERR_INVALID_ARGS;
  }
  memcpy(dst + start_in_byte, src, end_in_byte - start_in_byte);
  return ZX_OK;
}

template <typename T = RawBitmap>
inline zx_status_t CloneBits(T &to, const T &from, size_t offset, size_t len) {
  if (to.size() < offset + len || from.size() < len) {
    return ZX_ERR_INVALID_ARGS;
  }
  uint8_t *dst = static_cast<uint8_t *>(to.StorageUnsafe()->GetData());
  const uint8_t *src = static_cast<const uint8_t *>(from.StorageUnsafe()->GetData());
  bitcpy(dst, src, offset, len);
  return ZX_OK;
}

template <typename T = RawBitmap>
inline zx_status_t CloneBits(T &to, const void *from, size_t offset, size_t len) {
  if (to.size() < offset + len) {
    return ZX_ERR_INVALID_ARGS;
  }
  uint8_t *dst = static_cast<uint8_t *>(to.StorageUnsafe()->GetData());
  const uint8_t *src = static_cast<const uint8_t *>(from);
  bitcpy(dst, src, offset, len);
  return ZX_OK;
}

template <typename T = RawBitmap>
inline zx_status_t CloneBits(void *to, const T &from, size_t offset, size_t len) {
  if (from.size() < len) {
    return ZX_ERR_INVALID_ARGS;
  }
  uint8_t *dst = static_cast<uint8_t *>(to);
  const uint8_t *src = static_cast<const uint8_t *>(from.StorageUnsafe()->GetData());
  bitcpy(dst, src, offset, len);
  return ZX_OK;
}

size_t CountBits(const RawBitmap &bits, size_t offset, size_t len);

}  // namespace f2fs

#endif  // SRC_STORAGE_F2FS_BITMAP_H_
