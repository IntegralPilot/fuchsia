// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_LIB_DMA_BUFFER_INCLUDE_LIB_DMA_BUFFER_BUFFER_H_
#define SRC_DEVICES_LIB_DMA_BUFFER_INCLUDE_LIB_DMA_BUFFER_BUFFER_H_

#include <lib/zx/bti.h>
#include <lib/zx/pmt.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <zircon/process.h>
#include <zircon/syscalls.h>

#include <cstring>
#include <optional>
#include <type_traits>
#include <vector>

#include <fbl/intrusive_double_list.h>

namespace dma_buffer {

// Cache options for DMA buffers.
enum class CacheOptions {
  kDisabled,
  kEnabled,
};

// Concrete class for DMA buffers providing safe cache management and memory access.
class Buffer {
 public:
  Buffer(void* virt, size_t size, CacheOptions cache_options)
      : virt_(virt), size_(size), cache_options_(cache_options) {}

  size_t size() const { return size_; }
  void* virt() const { return virt_; }

  bool enable_cache() const { return cache_options_ == CacheOptions::kEnabled; }
  CacheOptions cache_options() const { return cache_options_; }

  // Flushes CPU data cache lines (clean to memory) for the given byte range.
  // Use after CPU writes to buffer so that DMA hardware can read from physical RAM
  // (DMA_TO_DEVICE).
  zx::result<> CacheFlush(size_t offset, size_t length) const;

  // Flushes and invalidates CPU data cache lines for the given byte range.
  // Use before CPU reads memory after DMA hardware writes to physical RAM (DMA_FROM_DEVICE).
  zx::result<> CacheFlushInvalidate(size_t offset, size_t length) const;

  // Safely writes memory to the buffer from the CPU and automatically flushes CPU cache so DMA
  // hardware can read from RAM (DMA_TO_DEVICE).
  zx::result<> Write(const void* src, size_t offset, size_t length) const;

  // Safely invalidates CPU cache and reads memory from the buffer after DMA hardware writes to RAM
  // (DMA_FROM_DEVICE).
  zx::result<> Read(size_t offset, size_t length, void* dest) const;

  // Helper for writing a trivially copyable struct/value with automatic cache flush.
  template <typename T>
  zx::result<> WriteStruct(const T& val, size_t offset = 0) const {
    static_assert(!std::is_pointer_v<T>, "WriteStruct requires a non-pointer type");
    static_assert(std::is_trivially_copyable_v<T>, "WriteStruct requires trivially copyable type");
    return Write(&val, offset, sizeof(T));
  }

  // Helper for reading a trivially copyable struct/value with automatic cache invalidation.
  template <typename T>
  zx::result<T> ReadStruct(size_t offset = 0) const {
    static_assert(!std::is_pointer_v<T>, "ReadStruct requires a non-pointer type");
    static_assert(std::is_trivially_copyable_v<T>, "ReadStruct requires trivially copyable type");
    T val{};
    zx::result<> status = Read(offset, sizeof(T), &val);
    if (status.is_error()) {
      return status.take_error();
    }
    return zx::ok(val);
  }

  // Executes a read closure on the given byte range after automatically invalidating CPU cache.
  template <typename Fn>
  zx::result<> ExecuteReadOps(size_t offset, size_t length, Fn&& fn) const {
    if (length == 0) {
      return zx::ok();
    }
    if (!virt()) {
      return zx::error(ZX_ERR_INVALID_ARGS);
    }
    if (offset + length < offset || offset + length > size()) {
      return zx::error(ZX_ERR_OUT_OF_RANGE);
    }
    if (enable_cache()) {
      zx::result<> status = CacheFlushInvalidate(offset, length);
      if (status.is_error()) {
        return status;
      }
    }
    auto ptr = reinterpret_cast<const uint8_t*>(virt()) + offset;
    fn(ptr);
    return zx::ok();
  }

  // Executes a write closure on the given byte range and automatically flushes CPU cache.
  template <typename Fn>
  zx::result<> ExecuteWriteOps(size_t offset, size_t length, Fn&& fn) const {
    if (length == 0) {
      return zx::ok();
    }
    if (!virt()) {
      return zx::error(ZX_ERR_INVALID_ARGS);
    }
    if (offset + length < offset || offset + length > size()) {
      return zx::error(ZX_ERR_OUT_OF_RANGE);
    }
    auto ptr = reinterpret_cast<uint8_t*>(virt()) + offset;
    fn(ptr);
    if (enable_cache()) {
      return CacheFlush(offset, length);
    }
    return zx::ok();
  }

 private:
  void* virt_;
  size_t size_;
  CacheOptions cache_options_ = CacheOptions::kEnabled;
};

// I/O buffer for managing physical memory associated with contiguous DMA buffers.
class ContiguousBuffer : public fbl::DoublyLinkedListable<std::unique_ptr<ContiguousBuffer>> {
 public:
  virtual const Buffer& buffer() const = 0;
  virtual zx_paddr_t phys() const = 0;
  virtual zx::unowned_vmo vmo() const = 0;

  virtual ~ContiguousBuffer() = default;

  // Re-export Buffer methods.
  size_t size() const { return buffer().size(); }
  void* virt() const { return buffer().virt(); }
  bool enable_cache() const { return buffer().enable_cache(); }
  CacheOptions cache_options() const { return buffer().cache_options(); }

  zx::result<> CacheFlush(size_t offset, size_t length) const {
    return buffer().CacheFlush(offset, length);
  }
  zx::result<> CacheFlushInvalidate(size_t offset, size_t length) const {
    return buffer().CacheFlushInvalidate(offset, length);
  }
  zx::result<> Write(const void* src, size_t offset, size_t length) const {
    return buffer().Write(src, offset, length);
  }
  zx::result<> Read(size_t offset, size_t length, void* dest) const {
    return buffer().Read(offset, length, dest);
  }

  template <typename T>
  zx::result<> WriteStruct(const T& val, size_t offset = 0) const {
    return buffer().WriteStruct(val, offset);
  }

  template <typename T>
  zx::result<T> ReadStruct(size_t offset = 0) const {
    return buffer().ReadStruct<T>(offset);
  }

  template <typename Fn>
  zx::result<> ExecuteReadOps(size_t offset, size_t length, Fn&& fn) const {
    return buffer().ExecuteReadOps(offset, length, std::forward<Fn>(fn));
  }

  template <typename Fn>
  zx::result<> ExecuteWriteOps(size_t offset, size_t length, Fn&& fn) const {
    return buffer().ExecuteWriteOps(offset, length, std::forward<Fn>(fn));
  }
};

// A paged buffer consisting of 1 or more pages pinned in memory
// which are contiguous in virtual memory, but may be discontiguous
// in physical memory.
class PagedBuffer : public fbl::DoublyLinkedListable<std::unique_ptr<PagedBuffer>> {
 public:
  virtual const Buffer& buffer() const = 0;
  virtual const zx_paddr_t* phys() const = 0;
  virtual ~PagedBuffer() = default;

  // Re-export Buffer methods.
  size_t size() const { return buffer().size(); }
  void* virt() const { return buffer().virt(); }
  bool enable_cache() const { return buffer().enable_cache(); }
  CacheOptions cache_options() const { return buffer().cache_options(); }

  zx::result<> CacheFlush(size_t offset, size_t length) const {
    return buffer().CacheFlush(offset, length);
  }
  zx::result<> CacheFlushInvalidate(size_t offset, size_t length) const {
    return buffer().CacheFlushInvalidate(offset, length);
  }
  zx::result<> Write(const void* src, size_t offset, size_t length) const {
    return buffer().Write(src, offset, length);
  }
  zx::result<> Read(size_t offset, size_t length, void* dest) const {
    return buffer().Read(offset, length, dest);
  }

  template <typename T>
  zx::result<> WriteStruct(const T& val, size_t offset = 0) const {
    return buffer().WriteStruct(val, offset);
  }

  template <typename T>
  zx::result<T> ReadStruct(size_t offset = 0) const {
    return buffer().ReadStruct<T>(offset);
  }

  template <typename Fn>
  zx::result<> ExecuteReadOps(size_t offset, size_t length, Fn&& fn) const {
    return buffer().ExecuteReadOps(offset, length, std::forward<Fn>(fn));
  }

  template <typename Fn>
  zx::result<> ExecuteWriteOps(size_t offset, size_t length, Fn&& fn) const {
    return buffer().ExecuteWriteOps(offset, length, std::forward<Fn>(fn));
  }
};

// Buffer factory -- abstract class used to create DMA buffers.
// Use CreateBufferFactory() to create a default implementation of a buffer factory.
// This class exists to allow for tests to override the behavior of DMA buffers.
// Refer to fake-dma-buffer to create a fake DMA buffer.
class BufferFactory {
 public:
  virtual zx_status_t CreateContiguous(const zx::bti& bti, size_t size, uint32_t alignment_log2,
                                       CacheOptions cache_options,
                                       std::unique_ptr<ContiguousBuffer>* out) const = 0;

  virtual zx_status_t CreatePaged(const zx::bti& bti, size_t size, CacheOptions cache_options,
                                  std::unique_ptr<PagedBuffer>* out) const = 0;

  virtual ~BufferFactory() = default;
};

std::unique_ptr<BufferFactory> CreateBufferFactory();

}  // namespace dma_buffer

#endif  // SRC_DEVICES_LIB_DMA_BUFFER_INCLUDE_LIB_DMA_BUFFER_BUFFER_H_
