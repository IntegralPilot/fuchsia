// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_BLOCK_CLIENT_CPP_READER_WRITER_H_
#define SRC_STORAGE_LIB_BLOCK_CLIENT_CPP_READER_WRITER_H_

#include <lib/fzl/owned-vmo-mapper.h>

#include "src/storage/lib/block_client/cpp/block_device.h"
#include "src/storage/lib/buffer/owned_vmoid.h"

namespace block_client {

// ReaderWriter provides a simple wrapper around a block device that permits reading/writing from a
// device without having to worry about VMOs. It should not be used if performance is a concern and
// it is *not* thread-safe.
class ReaderWriter {
 public:
  explicit ReaderWriter(BlockDevice& device) : device_(device) {}

  // Reads `count` bytes from the device at offset `offset`.  Both `count` and `offset` must be
  // aligned to the device block size.
  zx_status_t Read(uint64_t offset, size_t count, void* buf);

  // Reads `count` bytes from the device at offset `offset`.  Both `count` and `offset` must be
  // aligned to the device block size.
  zx_status_t Read(uint64_t offset, size_t count, const zx::vmo& vmo, uint64_t vmo_offset);

  // Writes `count` bytes to the device at offset `offset`.  Both `count` and `offset` must be
  // aligned to the device block size.
  zx_status_t Write(uint64_t offset, size_t count, void* buf);

  // Writes `count` bytes to the device at offset `offset`.  Both `count` and `offset` must be
  // aligned to the device block size.
  zx_status_t Write(uint64_t offset, size_t count, const zx::vmo& vmo, uint64_t vmo_offset);

 private:
  zx_status_t EnsureBufferInitialized();
  zx_status_t QueryAndValidateBlockSize();
  // If `buf` is set, we copy data out to it and keep overwriting the same range in the VMO.
  // Otherwise, we stream into the VMO from `vmo_offset`.
  zx_status_t DoIo(uint64_t offset, size_t count, vmoid_t vmoid, uint64_t vmo_offset, bool write,
                   std::optional<void*> buf);
  BlockDevice& device_;
  uint64_t block_size_ = 0;
  fzl::OwnedVmoMapper buffer_;
  storage::OwnedVmoid vmoid_;
};

}  // namespace block_client

#endif  // SRC_STORAGE_LIB_BLOCK_CLIENT_CPP_READER_WRITER_H_
