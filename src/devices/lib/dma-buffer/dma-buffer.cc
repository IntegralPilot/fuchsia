// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zx/vmar.h>

#include <cstring>

#include "include/lib/dma-buffer/buffer.h"

namespace dma_buffer {

namespace internal {

// Helper for writing to uncached DMA buffer memory without triggering ARM64 alignment faults.
// Using volatile pointers prevents the compiler from optimizing the loop into unaligned vector
// instructions or std::memcpy calls that fail on device/uncached memory mappings.
void UncachedMemoryWrite(volatile void* dest, const void* src, size_t size) {
  auto dest_ptr = reinterpret_cast<uintptr_t>(dest);
  auto src_ptr = reinterpret_cast<uintptr_t>(src);

  // 1. Align dest_ptr to uint64_t boundary using single byte writes.
  while ((dest_ptr & (sizeof(uint64_t) - 1)) && size > 0) {
    *reinterpret_cast<volatile uint8_t*>(dest_ptr) = *reinterpret_cast<const uint8_t*>(src_ptr);
    --size;
    ++dest_ptr;
    ++src_ptr;
  }

  // 2. Perform aligned uint64_t writes.
  while (size >= sizeof(uint64_t)) {
    uint64_t tmp;
    std::memcpy(&tmp, reinterpret_cast<const void*>(src_ptr), sizeof(uint64_t));
    *reinterpret_cast<volatile uint64_t*>(dest_ptr) = tmp;
    size -= sizeof(uint64_t);
    dest_ptr += sizeof(uint64_t);
    src_ptr += sizeof(uint64_t);
  }

  // 3. Write remaining tail bytes.
  while (size > 0) {
    *reinterpret_cast<volatile uint8_t*>(dest_ptr) = *reinterpret_cast<const uint8_t*>(src_ptr);
    --size;
    ++dest_ptr;
    ++src_ptr;
  }
}

// Helper for reading from uncached DMA buffer memory without triggering ARM64 alignment faults.
void UncachedMemoryRead(void* dest, const volatile void* src, size_t size) {
  auto dest_ptr = reinterpret_cast<uintptr_t>(dest);
  auto src_ptr = reinterpret_cast<uintptr_t>(src);

  // 1. Align src_ptr to uint64_t boundary using single byte reads.
  while ((src_ptr & (sizeof(uint64_t) - 1)) && size > 0) {
    *reinterpret_cast<uint8_t*>(dest_ptr) = *reinterpret_cast<const volatile uint8_t*>(src_ptr);
    --size;
    ++dest_ptr;
    ++src_ptr;
  }

  // 2. Perform aligned uint64_t reads.
  while (size >= sizeof(uint64_t)) {
    uint64_t tmp = *reinterpret_cast<const volatile uint64_t*>(src_ptr);
    std::memcpy(reinterpret_cast<void*>(dest_ptr), &tmp, sizeof(uint64_t));
    size -= sizeof(uint64_t);
    dest_ptr += sizeof(uint64_t);
    src_ptr += sizeof(uint64_t);
  }

  // 3. Read remaining tail bytes.
  while (size > 0) {
    *reinterpret_cast<uint8_t*>(dest_ptr) = *reinterpret_cast<const volatile uint8_t*>(src_ptr);
    --size;
    ++dest_ptr;
    ++src_ptr;
  }
}

}  // namespace internal

// I/O buffer for managing physical memory associated with contiguous DMA buffers.
class ContiguousBufferImpl : public ContiguousBuffer {
 public:
  ContiguousBufferImpl(size_t size, zx::vmo vmo, void* virt, zx_paddr_t phys, zx::pmt pmt,
                       CacheOptions cache_options)
      : buffer_(virt, size, cache_options),
        phys_(phys),
        vmo_(std::move(vmo)),
        pmt_(std::move(pmt)) {}

  const Buffer& buffer() const override { return buffer_; }
  zx_paddr_t phys() const override { return phys_; }
  ~ContiguousBufferImpl() {
    [[maybe_unused]] auto status =
        zx::vmar::root_self()->unmap(reinterpret_cast<zx_vaddr_t>(buffer_.virt()), buffer_.size());
    ZX_DEBUG_ASSERT(status == ZX_OK);
    pmt_.unpin();
  }

  zx::unowned_vmo vmo() const override { return vmo_.borrow(); }

 private:
  Buffer buffer_;
  zx_paddr_t phys_;
  zx::vmo vmo_;
  zx::pmt pmt_;
};

// A paged buffer consisting of 1 or more pages pinned in memory
// which are contiguous in virtual memory, but may be discontiguous
// in physical memory.
class PagedBufferImpl : public PagedBuffer {
 public:
  PagedBufferImpl(size_t size, zx::vmo vmo, void* virt, std::vector<zx_paddr_t> phys, zx::pmt pmt,
                  CacheOptions cache_options)
      : buffer_(virt, size, cache_options),
        phys_(std::move(phys)),
        vmo_(std::move(vmo)),
        pmt_(std::move(pmt)) {}

  const Buffer& buffer() const override { return buffer_; }
  const zx_paddr_t* phys() const override { return phys_.data(); }
  ~PagedBufferImpl() {
    [[maybe_unused]] auto status =
        zx::vmar::root_self()->unmap(reinterpret_cast<zx_vaddr_t>(buffer_.virt()), buffer_.size());
    ZX_DEBUG_ASSERT(status == ZX_OK);
    pmt_.unpin();
  }

 private:
  Buffer buffer_;
  std::vector<zx_paddr_t> phys_;
  zx::vmo vmo_;
  zx::pmt pmt_;
};

class BufferFactoryImpl : public BufferFactory {
  zx_status_t CreateContiguous(const zx::bti& bti, size_t size, uint32_t alignment_log2,
                               CacheOptions cache_options,
                               std::unique_ptr<ContiguousBuffer>* out) const override {
    zx::vmo vmo;
    zx_status_t status;
    status = zx::vmo::create_contiguous(bti, size, alignment_log2, &vmo);
    if (status != ZX_OK) {
      return status;
    }
    if (cache_options == CacheOptions::kDisabled) {
      status = vmo.set_cache_policy(ZX_CACHE_POLICY_UNCACHED_DEVICE);
    }
    if (status != ZX_OK) {
      return status;
    }
    void* virt;
    zx_paddr_t phys;
    status = zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0, size,
                                        reinterpret_cast<zx_vaddr_t*>(&virt));
    if (status != ZX_OK) {
      return status;
    }
    zx::pmt pmt;
    status = bti.pin(ZX_BTI_PERM_READ | ZX_BTI_PERM_WRITE | ZX_BTI_CONTIGUOUS, vmo, 0, size, &phys,
                     1, &pmt);
    if (status != ZX_OK) {
      return status;
    }
    auto buffer = std::make_unique<ContiguousBufferImpl>(size, std::move(vmo), virt, phys,
                                                         std::move(pmt), cache_options);
    *out = std::move(buffer);
    return ZX_OK;
  }
  zx_status_t CreatePaged(const zx::bti& bti, size_t size, CacheOptions cache_options,
                          std::unique_ptr<PagedBuffer>* out) const override {
    zx::vmo vmo;
    zx_status_t status;
    status = zx::vmo::create(size, 0, &vmo);
    if (status != ZX_OK) {
      return status;
    }
    if (cache_options == CacheOptions::kDisabled) {
      status = vmo.set_cache_policy(ZX_CACHE_POLICY_UNCACHED_DEVICE);
    }
    if (status != ZX_OK) {
      return status;
    }
    if (size % zx_system_get_page_size()) {
      size = ((size / zx_system_get_page_size()) + 1) * zx_system_get_page_size();
    }
    void* virt;
    std::vector<zx_paddr_t> phys;
    phys.resize(size / zx_system_get_page_size());
    status = zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0, size,
                                        reinterpret_cast<zx_vaddr_t*>(&virt));
    if (status != ZX_OK) {
      return status;
    }
    zx::pmt pmt;
    status =
        bti.pin(ZX_BTI_PERM_READ | ZX_BTI_PERM_WRITE, vmo, 0, size, phys.data(), phys.size(), &pmt);
    if (status != ZX_OK) {
      return status;
    }
    auto buffer = std::make_unique<PagedBufferImpl>(size, std::move(vmo), virt, std::move(phys),
                                                    std::move(pmt), cache_options);
    *out = std::move(buffer);
    return ZX_OK;
  }
};

zx::result<> Buffer::CacheFlush(size_t offset, size_t length) const {
  if (length == 0 || !enable_cache()) {
    return zx::ok();
  }
  if (!virt()) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  if (offset + length < offset || offset + length > size()) {
    return zx::error(ZX_ERR_OUT_OF_RANGE);
  }
  auto ptr = reinterpret_cast<const uint8_t*>(virt()) + offset;
  return zx::make_result(zx_cache_flush(ptr, length, ZX_CACHE_FLUSH_DATA));
}

zx::result<> Buffer::CacheFlushInvalidate(size_t offset, size_t length) const {
  if (length == 0 || !enable_cache()) {
    return zx::ok();
  }
  if (!virt()) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  if (offset + length < offset || offset + length > size()) {
    return zx::error(ZX_ERR_OUT_OF_RANGE);
  }
  auto ptr = reinterpret_cast<const uint8_t*>(virt()) + offset;
  return zx::make_result(
      zx_cache_flush(ptr, length, ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE));
}

zx::result<> Buffer::Write(const void* src, size_t offset, size_t length) const {
  if (length == 0) {
    return zx::ok();
  }
  if (!src || !virt()) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  if (offset + length < offset || offset + length > size()) {
    return zx::error(ZX_ERR_OUT_OF_RANGE);
  }
  auto ptr = reinterpret_cast<uint8_t*>(virt()) + offset;
  if (enable_cache()) {
    std::memcpy(ptr, src, length);
    return CacheFlush(offset, length);
  } else {
    internal::UncachedMemoryWrite(ptr, src, length);
    return zx::ok();
  }
}

zx::result<> Buffer::Read(size_t offset, size_t length, void* dest) const {
  if (length == 0) {
    return zx::ok();
  }
  if (!dest || !virt()) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  if (offset + length < offset || offset + length > size()) {
    return zx::error(ZX_ERR_OUT_OF_RANGE);
  }
  auto ptr = reinterpret_cast<const uint8_t*>(virt()) + offset;
  if (enable_cache()) {
    zx::result<> status = CacheFlushInvalidate(offset, length);
    if (status.is_error()) {
      return status;
    }
    std::memcpy(dest, ptr, length);
  } else {
    internal::UncachedMemoryRead(dest, ptr, length);
  }
  return zx::ok();
}

std::unique_ptr<BufferFactory> CreateBufferFactory() {
  return std::make_unique<BufferFactoryImpl>();
}

}  // namespace dma_buffer
