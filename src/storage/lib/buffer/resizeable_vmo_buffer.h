// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_BUFFER_RESIZEABLE_VMO_BUFFER_H_
#define SRC_STORAGE_LIB_BUFFER_RESIZEABLE_VMO_BUFFER_H_

#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <zircon/types.h>

#include <cstddef>
#include <cstdint>
#include <string_view>

#include "src/storage/lib/buffer/block_buffer.h"
#include "src/storage/lib/buffer/mapped_vmo.h"
#include "src/storage/lib/buffer/vmoid_registry.h"

namespace storage {

// A resizeable VMO buffer. The buffer isn't usable until Attach is called.
class ResizeableVmoBuffer : public BlockBuffer {
 public:
  explicit ResizeableVmoBuffer(uint32_t block_size) : block_size_(block_size) {}

  ResizeableVmoBuffer(ResizeableVmoBuffer&&) = default;
  ResizeableVmoBuffer& operator=(ResizeableVmoBuffer&&) = delete;

  // BlockBuffer interface:
  size_t capacity() const override { return mapped_vmo_.size() / block_size_; }
  uint32_t BlockSize() const override { return block_size_; }
  vmoid_t vmoid() const override { return vmoid_.get(); }
  zx_handle_t Vmo() const override { return mapped_vmo_.vmo().get(); }
  void* Data(size_t index) override {
    return reinterpret_cast<void*>(reinterpret_cast<uintptr_t>(mapped_vmo_.start()) +
                                   index * block_size_);
  }
  const void* Data(size_t index) const override {
    return reinterpret_cast<const void*>(reinterpret_cast<uintptr_t>(mapped_vmo_.start()) +
                                         index * block_size_);
  }

  const zx::vmo& vmo() { return mapped_vmo_.vmo(); }
  zx::result<> Grow(size_t block_count) { return mapped_vmo_.Grow(block_count * block_size_); }
  zx::result<> Shrink(size_t block_count) { return mapped_vmo_.Shrink(block_count * block_size_); }

  zx::result<> Attach(std::string_view name, VmoidRegistry& device);
  zx::result<> Detach(VmoidRegistry& device);

  zx_status_t Zero(size_t index, size_t count) override;

 private:
  ResizeableVmoBuffer();

  uint32_t block_size_;
  MappedVmo mapped_vmo_;
  Vmoid vmoid_;
};

}  // namespace storage

#endif  // SRC_STORAGE_LIB_BUFFER_RESIZEABLE_VMO_BUFFER_H_
