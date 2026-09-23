// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/f2fs/storage_buffer.h"

#include "src/storage/f2fs/bcache.h"
#include "src/storage/f2fs/file_cache.h"

namespace f2fs {

StorageBuffer::StorageBuffer(BcacheMapper *bc, size_t num_blocks, std::string_view label)
    : allocated_blocks_(num_blocks) {
  ZX_ASSERT(num_blocks >= 1);
  ZX_ASSERT(buffer_.Initialize(bc, num_blocks, Page::Size(), label.data()) == ZX_OK);
}

zx::result<size_t> StorageBuffer::Reserve(size_t num_blocks) {
  if (current_index_ + num_blocks > allocated_blocks_) {
    return zx::error(ZX_ERR_UNAVAILABLE);
  }
  size_t ret = current_index_;
  current_index_ += num_blocks;
  return zx::ok(ret);
}

void *StorageBuffer::Data(const size_t offset) {
  ZX_ASSERT(offset < buffer_.capacity());
  return buffer_.Data(offset);
}

StorageBufferPool::StorageBufferPool(BcacheMapper *bcache_mapper, size_t default_size,
                                     size_t large_size, int num_pager_threads)
    : buffer_size_(default_size),
      large_buffer_size_(large_size),
      num_buffers_(num_pager_threads),
      bcache_mapper_(bcache_mapper) {
  ZX_ASSERT(num_pager_threads > 0);
  for (size_t i = 0; i < num_buffers_; ++i) {
    std::string str = "StorageBuffer_" + std::to_string(buffer_size_) + "_" + std::to_string(i);
    buffers(buffer_size_)
        .push_back(std::make_unique<StorageBuffer>(bcache_mapper, buffer_size_, str));
    str = "StorageBuffer_" + std::to_string(large_buffer_size_) + "_" + std::to_string(i);
    buffers(large_buffer_size_)
        .push_back(std::make_unique<StorageBuffer>(bcache_mapper, large_buffer_size_, str));
  }
}

StorageBufferPool::~StorageBufferPool() {
  for (size_t i = 0; i < num_buffers_; ++i) {
    auto buffer = Retrieve(buffer_size_);
    auto large_buffer = Retrieve(large_buffer_size_);
  }
}

std::unique_ptr<StorageBuffer> StorageBufferPool::Retrieve(size_t size) {
  ZX_ASSERT(var_.wait_for(mutex_, std::chrono::seconds(kWriteTimeOut),
                          [this, &size]()
                              TA_NO_THREAD_SAFETY_ANALYSIS { return !buffers(size).empty(); }));
  auto owned_buffer = std::move(buffers(size).back());
  buffers(size).pop_back();
  return owned_buffer;
}

OwnedStorageBuffer StorageBufferPool::Get(size_t size) {
  std::lock_guard lock(mutex_);
  ZX_ASSERT(var_.wait_for(mutex_, std::chrono::seconds(kWriteTimeOut),
                          [this, &size]()
                              TA_NO_THREAD_SAFETY_ANALYSIS { return !buffers(size).empty(); }));

  auto owned_buffer = std::move(buffers(size).back());
  buffers(size).pop_back();
  return OwnedStorageBuffer(std::move(owned_buffer),
                            [this](std::unique_ptr<StorageBuffer> buffer)
                                __TA_EXCLUDES(mutex_) { Return(std::move(buffer)); });
}

void StorageBufferPool::Return(std::unique_ptr<StorageBuffer> buffer) {
  if (buffer) {
    std::lock_guard lock(mutex_);
    buffers(buffer->Capacity()).push_back(std::move(buffer));
    var_.notify_one();
  }
}

}  // namespace f2fs
