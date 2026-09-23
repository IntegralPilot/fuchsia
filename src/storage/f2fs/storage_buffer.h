// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_F2FS_STORAGE_BUFFER_H_
#define SRC_STORAGE_F2FS_STORAGE_BUFFER_H_

#include <condition_variable>

#include "src/storage/f2fs/layout.h"
#include "src/storage/lib/buffer/vmo_buffer.h"

namespace f2fs {

class BcacheMapper;
// StorageBuffer implements an allocator for vmo buffers attached to |bc|.
class StorageBuffer {
 public:
  explicit StorageBuffer(BcacheMapper *bc, size_t num_pages, std::string_view label);
  StorageBuffer() = delete;
  StorageBuffer(const StorageBuffer &) = delete;
  StorageBuffer &operator=(const StorageBuffer &) = delete;
  StorageBuffer(StorageBuffer &&) = delete;
  StorageBuffer &operator=(StorageBuffer &&) = delete;
  ~StorageBuffer() = default;

  zx::result<size_t> Reserve(size_t num_blocks = 1);

  void *Data(const size_t offset);

  storage::VmoBuffer &GetVmoBuffer() { return buffer_; }

  size_t Size() const { return current_index_; }
  size_t Capacity() const { return allocated_blocks_; }
  bool IsFull() const { return current_index_ >= allocated_blocks_; }
  void Reset() { current_index_ = 0; }

 private:
  const size_t allocated_blocks_;
  size_t current_index_;
  storage::VmoBuffer buffer_;
};

using ReleaseBuffer = fit::function<void(std::unique_ptr<StorageBuffer>)>;
// A wrapper class for users of StorageBuffer.
// It owns |buffer_| exclusively, and transfers its ownership to |release_| on destructor.
class OwnedStorageBuffer {
 public:
  explicit OwnedStorageBuffer(std::unique_ptr<StorageBuffer> buffer, ReleaseBuffer cb) {
    ZX_ASSERT(buffer);
    ZX_ASSERT(cb);
    buffer_ = std::move(buffer);
    buffer_->Reset();
    release_ = std::move(cb);
  }
  ~OwnedStorageBuffer() { release_(std::move(buffer_)); }

  StorageBuffer *operator->() const { return buffer_.get(); }

 private:
  std::unique_ptr<StorageBuffer> buffer_;
  ReleaseBuffer release_ = nullptr;
};

class StorageBufferPool {
 public:
  explicit StorageBufferPool(BcacheMapper *bcache_mapper, size_t default_size = kMaxReadaheadSize,
                             size_t large_size = kDefaultBlocksPerSegment,
                             int num_pager_threads = 1);
  ~StorageBufferPool();

  OwnedStorageBuffer Get(size_t size) __TA_EXCLUDES(mutex_);
  BcacheMapper *Block() { return bcache_mapper_; }

  // for tests
  size_t GetLargeBufferSize() const { return large_buffer_size_; }

 private:
  std::vector<std::unique_ptr<StorageBuffer>> &buffers(size_t size) __TA_REQUIRES(mutex_) {
    if (size <= buffer_size_) {
      return buffers_;
    }
    return large_buffers_;
  }

  // Every user should transfer the ownership of borrowed buffers to this method after use.
  void Return(std::unique_ptr<StorageBuffer> buffer);
  std::unique_ptr<StorageBuffer> Retrieve(size_t size) __TA_REQUIRES(mutex_);

  const size_t buffer_size_;
  const size_t large_buffer_size_;
  const size_t num_buffers_;

  std::vector<std::unique_ptr<StorageBuffer>> large_buffers_ __TA_GUARDED(mutex_);
  std::vector<std::unique_ptr<StorageBuffer>> buffers_ __TA_GUARDED(mutex_);

  BcacheMapper *const bcache_mapper_ = nullptr;
  std::mutex mutex_;
  std::condition_variable_any var_ __TA_GUARDED(mutex_);
  std::unique_ptr<StorageBuffer> pool_ __TA_GUARDED(mutex_);
};

}  // namespace f2fs

#endif  // SRC_STORAGE_F2FS_STORAGE_BUFFER_H_
