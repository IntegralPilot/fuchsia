// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include "buffer.h"

#include <zircon/assert.h>

size_t DurableBuffer::BytesAllocated() const {
  return bytes_allocated_.load(std::memory_order_relaxed);
}

void DurableBuffer::Set(std::span<uint8_t> data) {
  data_ = data;
  bytes_allocated_.store(0, std::memory_order_relaxed);
}

size_t DurableBuffer::MaxSize() const { return data_.size(); }

void DurableBuffer::Reset() { bytes_allocated_.store(0, std::memory_order_relaxed); }

uint64_t* DurableBuffer::AllocRecord(size_t num_bytes) {
  ZX_DEBUG_ASSERT((num_bytes & 7) == 0);
  // We need to use a cmpxchg loop here instead of a fetch_add. Since we don't have a
  // commit pointer and rely on the writer to actually write the bytes, we don't want to modify the
  // bytes_allocated unless we know the writer is also going to be able to write to the allocated
  // space.
  uint64_t allocated = bytes_allocated_.load(std::memory_order_relaxed);
  while (true) {
    const uint64_t desired = allocated + num_bytes;
    if (unlikely(desired > data_.size())) {
      return nullptr;
    }
    const bool success = bytes_allocated_.compare_exchange_weak(
        allocated, desired, std::memory_order_relaxed, std::memory_order_relaxed);
    if (success) {
      uint8_t* ptr = data_.data() + allocated;
      return reinterpret_cast<uint64_t*>(ptr);
    }
  }
}
