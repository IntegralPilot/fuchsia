// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/f2fs/reader.h"

#include "src/storage/f2fs/bcache.h"
#include "src/storage/f2fs/common.h"
#include "src/storage/f2fs/file_cache.h"
#include "src/storage/f2fs/storage_buffer.h"

namespace f2fs {

static inline bool CopyReadData(zx::vmo &dst, const void *src, size_t from, size_t size) {
  if (!size) {
    return false;
  }
  dst.write(src, from * Page::Size(), size * Page::Size());
  return true;
}

Reader::Reader(std::unique_ptr<StorageBufferPool> pool) : bcache_mapper_(pool->Block()) {
  pool_ = std::move(pool);
}

std::vector<storage::BufferedOperation> Reader::BuildBufferedOperations(
    OwnedStorageBuffer &buffer, const std::vector<block_t> &addrs, size_t &from) {
  ZX_DEBUG_ASSERT(!buffer->Size());
  fs::BufferedOperationsBuilder builder;
  for (; !buffer->IsFull() && from < addrs.size(); ++from) {
    block_t addr = addrs[from];
    if (!IsValidBlockAddr(addr)) {
      continue;
    }
    zx::result index = buffer->Reserve(1);
    ZX_ASSERT(index.is_ok());
    storage::Operation op = {
        .type = storage::OperationType::kRead,
        .vmo_offset = *index,
        .dev_offset = addr,
        .length = 1,
    };
    builder.Add(op, &buffer->GetVmoBuffer());
  }
  return builder.TakeOperations();
}

zx::result<> Reader::ReadBlocks(zx::vmo &vmo, std::vector<block_t> &addrs) {
  // indice for |vmo| and |addrs|.
  size_t from, to = 0;
  while (to < addrs.size()) {
    from = to;
    OwnedStorageBuffer buffer = pool_->Get(addrs.size() - from);
    auto operations = BuildBufferedOperations(buffer, addrs, to);
    // If every addr is either kNullAddr or kNewAddr, |operations| is empty.
    if (operations.empty()) {
      continue;
    }
    zx_status_t io_status = bcache_mapper_->RunRequests(operations);
    if (io_status != ZX_OK) {
      FX_LOGS(ERROR) << "read operations failed" << zx_status_get_string(io_status);
      return zx::error(io_status);
    }
    // index for |buffer|.
    size_t buffer_index = 0;
    size_t num_pages = to - from;
    if (buffer->Size() == num_pages) {
      CopyReadData(vmo, buffer->Data(buffer_index), from, num_pages);
      continue;
    }
    // Copy a valid data chunk to |vmo| at once.
    num_pages = 0;
    for (size_t i = from; i < to; ++i) {
      if (IsValidBlockAddr(addrs[i])) {
        ++num_pages;
        continue;
      }
      if (CopyReadData(vmo, buffer->Data(buffer_index), from, num_pages)) {
        buffer_index += num_pages;
        from += num_pages;
        num_pages = 0;
      }
      ++from;
    }
    CopyReadData(vmo, buffer->Data(buffer_index), from, num_pages);
  }
  return zx::ok();
}

zx::result<> Reader::ReadBlocks(std::vector<LockedPage> &pages, std::vector<block_t> &addrs) {
  // indice for |pages| and |addrs|.
  size_t from, to = 0;
  while (to < addrs.size()) {
    from = to;
    OwnedStorageBuffer buffer = pool_->Get(addrs.size() - from);
    auto operations = BuildBufferedOperations(buffer, addrs, to);
    // If every addr is either kNullAddr or kNewAddr, |operations| is empty.
    if (!operations.empty()) {
      zx_status_t io_status = bcache_mapper_->RunRequests(operations);
      if (io_status != ZX_OK) {
        FX_LOGS(ERROR) << "read operations failed" << zx_status_get_string(io_status);
        return zx::error(io_status);
      }
    }
    // index for |buffer|.
    size_t buffer_index = 0;
    for (size_t i = from; i < to; ++i) {
      block_t addr = addrs[i];
      LockedPage &page = pages[i];
      if (!IsValidBlockAddr(addr)) {
        ZX_ASSERT(addr == kNullAddr || page->SetUptodate());
        continue;
      }
      if (!page->IsUptodate()) {
        page->Write(buffer->Data(buffer_index));
        page->SetUptodate();
      }
      ++buffer_index;
    }
  }
  return zx::ok();
}

}  // namespace f2fs

// namespace f2fs
