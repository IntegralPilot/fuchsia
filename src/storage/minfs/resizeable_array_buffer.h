// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_MINFS_RESIZEABLE_ARRAY_BUFFER_H_
#define SRC_STORAGE_MINFS_RESIZEABLE_ARRAY_BUFFER_H_

#include <lib/zx/result.h>

#include <cstddef>
#include <cstdint>
#include <string_view>

#include "src/storage/lib/buffer/array_buffer.h"
#include "src/storage/lib/vfs/cpp/transaction/transaction_handler.h"

namespace minfs {

class ResizeableArrayBuffer : public storage::ArrayBuffer {
 public:
  using Handle = void*;

  explicit ResizeableArrayBuffer(uint32_t block_size) : ArrayBuffer(1, block_size) {}
  explicit ResizeableArrayBuffer(size_t capacity, uint32_t block_size)
      : ArrayBuffer(capacity, block_size) {}

  zx::result<> Attach(std::string_view name, fs::TransactionHandler& device) { return zx::ok(); }
  zx::result<> Detach(fs::TransactionHandler& device) { return zx::ok(); }

  zx::result<> Shrink(size_t block_count);
  zx::result<> Grow(size_t block_count);
};

}  // namespace minfs

#endif  // SRC_STORAGE_MINFS_RESIZEABLE_ARRAY_BUFFER_H_
