// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_UTIL_RINGBUFFER_H_
#define SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_UTIL_RINGBUFFER_H_

#include <lib/magma/util/dlog.h>
#include <lib/magma/util/macros.h>

#include "accessor.h"
#include "address_space.h"
#include "instruction_writer.h"

class TestRingbuffer;

namespace magma {

// Template class containing a ringbuffer of instructions, which can be mapped
// onto both the CPU and GPU.
template <typename GpuMapping>
class Ringbuffer : public InstructionWriter {
 public:
  // If specified, |size| must be less than the buffer size.
  Ringbuffer(std::unique_ptr<typename GpuMapping::BufferType>&& buffer, uint32_t size = 0);

  uint32_t size() { return size_; }

  uint32_t tail() { return tail_; }

  uint32_t head() { return head_; }

  void update_head(uint32_t head) {
    MAGMA_DASSERT((head & (sizeof(*vaddr_) - 1)) == 0);
    MAGMA_DASSERT(head < size_);
    MAGMA_DLOG("updating head 0x%x", head);
    head_ = head;
  }

  void Reset(uint32_t offset) {
    MAGMA_DASSERT((offset & (sizeof(*vaddr_) - 1)) == 0);
    MAGMA_DASSERT(offset < size_);
    update_head(offset);
    update_tail(offset);
  }

  void Write32(uint32_t value) override;

  bool HasSpace(uint32_t bytes);

  // Maps to both CPU and GPU.
  bool Map(std::shared_ptr<AddressSpace<GpuMapping>> address_space, uint64_t* gpu_addr_out);
  // Thread-safe variant of |Map|. The created GPU mapping is returned in |out_gpu_mapping|.
  bool MultiMap(std::shared_ptr<AddressSpace<GpuMapping>> address_space, uint64_t gpu_addr,
                std::shared_ptr<GpuMapping>* out_gpu_mapping);
  bool MapCpu();
  bool Unmap();

 protected:
  uint32_t* vaddr() { return vaddr_; }

  void update_tail(uint32_t tail) {
    MAGMA_DASSERT((tail & (sizeof(*vaddr_) - 1)) == 0);
    MAGMA_DASSERT(tail < size_);
    MAGMA_DLOG("updating tail 0x%x", tail);
    tail_ = tail;
  }

 private:
  std::shared_ptr<typename GpuMapping::BufferType> buffer_;
  std::shared_ptr<GpuMapping> gpu_mapping_;
  uint32_t size_;
  uint32_t head_;
  uint32_t tail_;
  uint32_t* vaddr_{};  // mapped virtual address

  friend class ::TestRingbuffer;
};

template <typename GpuMapping>
Ringbuffer<GpuMapping>::Ringbuffer(std::unique_ptr<typename GpuMapping::BufferType>&& buffer,
                                   uint32_t size)
    : buffer_(std::move(buffer)), size_(size) {
  uint64_t buffer_size =
      BufferAccessor<typename GpuMapping::BufferType>::platform_buffer(buffer_.get())->size();
  if (size_ == 0) {
    size_ = magma::to_uint32(buffer_size);
  }
  MAGMA_DASSERT(size_ <= buffer_size);
  MAGMA_DASSERT((size_ & (sizeof(*vaddr_) - 1)) == 0);

  tail_ = 0;
  head_ = tail_;
}

template <typename GpuMapping>
void Ringbuffer<GpuMapping>::Write32(uint32_t value) {
  MAGMA_DASSERT(vaddr_);
  // Note vaddr_ is an array of 32-bit=4 byte values
  vaddr_[tail_ >> 2] = value;
  tail_ += sizeof(value);
  if (tail_ >= size_) {
    MAGMA_DLOG("ringbuffer tail wrapped");
    tail_ = 0;
  }
  MAGMA_DASSERT(tail_ != head_);
}

template <typename GpuMapping>
bool Ringbuffer<GpuMapping>::HasSpace(uint32_t bytes) {
  // Can't fill completely such that tail_ == head_
  int32_t space = head_ - tail_ - sizeof(uint32_t);
  if (space <= 0)
    space += size_;
  bool ret = static_cast<uint32_t>(space) >= bytes;
  return MAGMA_DRETF(ret, "Insufficient space: bytes 0x%x space 0x%x", bytes, space);
}

template <typename GpuMapping>
bool Ringbuffer<GpuMapping>::Map(std::shared_ptr<AddressSpace<GpuMapping>> address_space,
                                 uint64_t* gpu_addr_out) {
  MAGMA_DASSERT(!vaddr_);

  auto gpu_mapping = AddressSpace<GpuMapping>::MapBufferGpu(address_space, buffer_);
  if (!gpu_mapping)
    return MAGMA_DRETF(false, "MapBufferGpu failed");

  void* addr;
  if (!BufferAccessor<typename GpuMapping::BufferType>::platform_buffer(buffer_.get())
           ->MapCpu(&addr)) {
    return MAGMA_DRETF(false, "MapCpu failed");
  }

  vaddr_ = reinterpret_cast<uint32_t*>(addr);

  *gpu_addr_out = gpu_mapping->gpu_addr();
  gpu_mapping_ = std::move(gpu_mapping);

  return true;
}

template <typename GpuMapping>
bool Ringbuffer<GpuMapping>::MultiMap(std::shared_ptr<AddressSpace<GpuMapping>> address_space,
                                      uint64_t gpu_addr,
                                      std::shared_ptr<GpuMapping>* out_gpu_mapping) {
  uint64_t page_count =
      BufferAccessor<typename GpuMapping::BufferType>::platform_buffer(buffer_.get())->size() /
      magma::page_size();

  std::shared_ptr<GpuMapping> gpu_mapping;
  magma::Status status = AddressSpace<GpuMapping>::MapBufferGpu(
      address_space, buffer_, gpu_addr, 0 /* page_offset */, page_count, &gpu_mapping);
  if (!status.ok()) {
    return MAGMA_DRET_MSG(status.get(), "MapBufferGpu failed");
  }
  MAGMA_DASSERT(gpu_mapping);

  *out_gpu_mapping = std::move(gpu_mapping);

  return true;
}

template <typename GpuMapping>
bool Ringbuffer<GpuMapping>::MapCpu() {
  MAGMA_DASSERT(!vaddr_);

  void* addr;
  if (!BufferAccessor<typename GpuMapping::BufferType>::platform_buffer(buffer_.get())
           ->MapCpu(&addr)) {
    return MAGMA_DRETF(false, "MapCpu failed");
  }

  vaddr_ = reinterpret_cast<uint32_t*>(addr);

  return true;
}

template <typename GpuMapping>
bool Ringbuffer<GpuMapping>::Unmap() {
  MAGMA_DASSERT(vaddr_);

  if (!buffer_->platform_buffer()->UnmapCpu())
    return MAGMA_DRETF(false, "UnmapCpu failed");

  gpu_mapping_.reset();

  return true;
}

}  // namespace magma

#endif  // SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_UTIL_RINGBUFFER_H_
