// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_F2FS_READER_H_
#define SRC_STORAGE_F2FS_READER_H_

#include "src/storage/f2fs/common.h"
#include "src/storage/lib/buffer/vmo_buffer.h"
#include "src/storage/lib/vfs/cpp/transaction/buffered_operations_builder.h"

namespace f2fs {

class StorageBufferPool;
class OwnedStorageBuffer;
class BcacheMapper;
class LockedPage;
class Reader {
 public:
  explicit Reader(std::unique_ptr<StorageBufferPool> pool);
  Reader() = delete;
  Reader(const Reader &) = delete;
  Reader &operator=(const Reader &) = delete;
  Reader(Reader &&) = delete;
  Reader &operator=(Reader &&) = delete;
  ~Reader() = default;

  // These methods build read operations from |addrs| and request them to underlying storage.
  // On success, it copy read data to |pages| or |vmo|.
  zx::result<> ReadBlocks(std::vector<LockedPage> &pages, std::vector<block_t> &addrs);
  zx::result<> ReadBlocks(zx::vmo &vmo, std::vector<block_t> &addrs);

 private:
  std::vector<storage::BufferedOperation> BuildBufferedOperations(OwnedStorageBuffer &buffer,
                                                                  const std::vector<block_t> &addrs,
                                                                  size_t &from);
  BcacheMapper *const bcache_mapper_ = nullptr;
  std::unique_ptr<StorageBufferPool> pool_;
};

}  // namespace f2fs

#endif  // SRC_STORAGE_F2FS_READER_H_
