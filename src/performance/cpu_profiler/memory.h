// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_CPU_PROFILER_MEMORY_H_
#define SRC_PERFORMANCE_CPU_PROFILER_MEMORY_H_

#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>
#include <lib/zx/process.h>
#include <zircon/syscalls.h>

#include <algorithm>
#include <span>
#include <utility>
#include <vector>

#include <src/lib/unwinder/memory.h>

namespace profiler {

class CachedModuleMemory : public unwinder::Memory {
 public:
  explicit CachedModuleMemory(unwinder::Memory* backend) : backend_(backend) {}

  unwinder::Error ReadBytes(uint64_t addr, uint64_t size, void* dst) override {
    TRACE_DURATION("cpu_profiler", "CachedModuleMemory::ReadBytes", "addr", addr, "size", size);
    if (size > kBlockSize) {
      return backend_->ReadBytes(addr, size, dst);
    }

    uint64_t block_addr = addr & ~(kBlockSize - 1);
    if (!cached_block_valid_ || block_addr != cached_block_addr_) {
      cached_block_.resize(kBlockSize);
      if (auto err = backend_->ReadBytes(block_addr, kBlockSize, cached_block_.data());
          err.has_err()) {
        // Fallback to direct read if block read fails (e.g. end of page)
        return backend_->ReadBytes(addr, size, dst);
      }
      cached_block_addr_ = block_addr;
      cached_block_valid_ = true;
    }

    if (addr + size > block_addr + kBlockSize) {
      // Crosses block boundary, fallback to direct read
      return backend_->ReadBytes(addr, size, dst);
    }

    memcpy(dst, cached_block_.data() + (addr - block_addr), size);
    return unwinder::Success();
  }

 private:
  static constexpr uint64_t kBlockSize = 4096;
  unwinder::Memory* backend_;
  uint64_t cached_block_addr_ = 0;
  bool cached_block_valid_ = false;
  std::vector<uint8_t> cached_block_;
};

struct StackChunk {
  uint64_t base;
  std::vector<uint8_t> data;
};

class BufferedStackMemory : public unwinder::Memory {
 public:
  BufferedStackMemory(zx::unowned_process process, std::vector<StackChunk> chunks,
                      std::span<const zx_info_maps_t> maps)
      : process_(std::move(process)), chunks_(std::move(chunks)), maps_(maps) {}

  unwinder::Error ReadBytes(uint64_t addr, uint64_t size, void* dst) override {
    TRACE_DURATION("cpu_profiler", "BufferedStackMemory::ReadBytes", "addr", addr, "size", size);
    if (ReadFromChunks(addr, size, dst)) {
      return unwinder::Success();
    }

    return unwinder::Error("BufferedStackMemory miss");
  }

  zx_status_t CaptureStack(uint64_t addr, size_t wanted_size = 4096 * 4) {
    TRACE_DURATION("cpu_profiler", "BufferedStackMemory::CaptureStack", "addr", addr, "size",
                   wanted_size);
    uint64_t fetch_addr = addr;

    std::vector<uint8_t> stack_copy(wanted_size);
    size_t actual_read = 0;
    zx_status_t status = zx_process_read_memory(process_->get(), fetch_addr, stack_copy.data(),
                                                wanted_size, &actual_read);

    if (status == ZX_OK) {
      stack_copy.resize(actual_read);
      chunks_.emplace_back(fetch_addr, std::move(stack_copy));
      return ZX_OK;
    }
    return status;
  }

  const std::vector<StackChunk>& GetChunks() const { return chunks_; }

 private:
  bool ReadFromChunks(uint64_t addr, uint64_t size, void* dst) {
    uint8_t* dst_ptr = static_cast<uint8_t*>(dst);
    while (size > 0) {
      bool chunk_found = false;
      for (const StackChunk& chunk : chunks_) {
        if (addr >= chunk.base && addr < chunk.base + chunk.data.size()) {
          size_t offset = addr - chunk.base;
          size_t available = chunk.data.size() - offset;
          size_t copy_size = std::min(static_cast<size_t>(size), available);

          memcpy(dst_ptr, chunk.data.data() + offset, copy_size);

          addr += copy_size;
          size -= copy_size;
          dst_ptr += copy_size;
          chunk_found = true;
          break;
        }
      }
      if (!chunk_found) {
        return false;
      }
    }
    return true;
  }

  bool IsCaptured(uint64_t addr) {
    for (const StackChunk& chunk : chunks_) {
      if (addr >= chunk.base && addr < chunk.base + chunk.data.size()) {
        return true;
      }
    }
    return false;
  }

  zx::unowned_process process_;
  std::vector<StackChunk> chunks_;
  std::span<const zx_info_maps_t> maps_;
};

}  // namespace profiler

#endif  // SRC_PERFORMANCE_CPU_PROFILER_MEMORY_H_
