// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_BUFFER_ARRAY_BUFFER_H_
#define SRC_STORAGE_LIB_BUFFER_ARRAY_BUFFER_H_

#include <fidl/fuchsia.storage.block/cpp/fidl.h>

#include <vector>

#include "src/storage/lib/buffer/block_buffer.h"

namespace storage {

// Block buffer backed by a heap array.
class ArrayBuffer : public BlockBuffer {
 public:
  explicit ArrayBuffer(size_t capacity, uint32_t block_size);
  ArrayBuffer(const ArrayBuffer&) = delete;
  ArrayBuffer(ArrayBuffer&&) = default;
  ArrayBuffer& operator=(const ArrayBuffer&) = delete;
  ArrayBuffer& operator=(ArrayBuffer&&) = default;
  ~ArrayBuffer() = default;

  // BlockBuffer interface:
  size_t capacity() const final { return buffer_.size() / block_size_; }
  uint32_t BlockSize() const final { return block_size_; }
#ifdef __Fuchsia__
  vmoid_t vmoid() const final { return fuchsia_storage_block::kVmoidInvalid; }
  zx_handle_t Vmo() const final { return ZX_HANDLE_INVALID; }
#endif
  void* Data(size_t index) final;
  const void* Data(size_t index) const final;

 protected:
  std::vector<uint8_t>& buffer() { return buffer_; }

 private:
  std::vector<uint8_t> buffer_;
  uint32_t block_size_ = 0;
};

}  // namespace storage

#endif  // SRC_STORAGE_LIB_BUFFER_ARRAY_BUFFER_H_
