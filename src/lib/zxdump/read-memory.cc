// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zxdump/task.h>

#include <algorithm>
#include <cstdio>
#include <memory>
#include <new>

#include "dump-file.h"

namespace zxdump {
namespace {

class AlignedBufferImpl final : public internal::BufferImpl {
 public:
  AlignedBufferImpl(std::align_val_t alignment, size_t size)
      : alignment_{alignment},
        size_{size},
        ptr_(static_cast<std::byte*>(operator new[](size_, alignment_))) {}

  ~AlignedBufferImpl() override { operator delete[](ptr_, size_, alignment_); }

  std::byte* get() const { return ptr_; }

 private:
  std::align_val_t alignment_;
  std::size_t size_;
  std::byte* ptr_;
};

}  // namespace

fit::result<Error, Buffer<>> Process::ReadMemoryImpl(uint64_t vaddr, size_t size,
                                                     ReadMemorySize size_mode, size_t alignment) {
  auto read_dump_memory = [this, vaddr, size, size_mode]() -> fit::result<Error, Buffer<>> {
    auto possible = [vaddr](const auto& elt) -> bool {
      const auto& [segment_vaddr, segment] = elt;
      return segment_vaddr + segment.memsz <= vaddr;
    };

    auto found = std::ranges::partition_point(memory_, possible);
    if (found != memory_.end()) {
      const auto& [segment_vaddr, segment] = *found;
      if (vaddr >= segment_vaddr && vaddr - segment_vaddr < segment.memsz) {
        // This segment covers the address.

        if (segment.filesz <= vaddr - segment_vaddr || !dump_) {
          // The portion of this segment we need was elided, or the whole dump
          // was inserted with read_memory=false,
          return fit::ok(Buffer<>{});
        }

        // Map the segment to a region of the file.
        internal::FileRange where = {
            .offset = segment.offset,
            .size = segment.filesz,
        };
        where %= vaddr - segment_vaddr;

        // If the next segment is contiguous we can fetch across them.
        ++found;
        while (found != memory_.end() && found->first == vaddr + where.size &&
               found->second.offset == where.offset + where.size) {
          where.size += found->second.filesz;
        }

        auto result = dump_->ReadMemory(where);
        if (result.is_error()) {
          return result.take_error();
        }
        Buffer<> buffer = *std::move(result);
        ZX_ASSERT(buffer->size_bytes() >= size);
        if (size_mode == ReadMemorySize::kExact) {
          buffer.data_ = buffer.data_.subspan(0, size);
        }

        return fit::ok(std::move(buffer));
      }
    }

    return fit::error(Error{"no such memory", ZX_ERR_NO_MEMORY});
  };

  auto result = live() ? ReadLiveMemory(vaddr, size, size_mode) : read_dump_memory();

  // When reading a job archive, the segment.offset has been adjusted by
  // the process dump file's position in the archive, so file offsets may
  // no longer be aligned as they were in the ELF file.  So even a request
  // at a properly-aligned vaddr might yield a file buffer that's not
  // aligned.  It's also possible that the live memory cache's pages didn't
  // actually get allocated locally as page-aligned, and the requested
  // alignment exceeds the alignment of the page buffer.  In these cases,
  // just copy to an aligned buffer.
  if (alignment > 1 && result.is_ok() && !result->empty()) {
    // Keep the Buffer<> holder and just update what it holds (if necessary).
    Buffer<>& buffer = *result;

    // Only copy the requested data even in ReadMemorySize::kMore mode.
    const size_t result_size = std::min(size, buffer->size_bytes());
    ZX_DEBUG_ASSERT(result_size > 0);

    // Check if it's not already as aligned as it needs to be.
    void* align_ptr = const_cast<std::byte*>(buffer->data());
    size_t align_space = result_size;
    if (!std::align(alignment, result_size, align_ptr, align_space)) {
      // Copy the contents into a fresh, aligned buffer.
      auto impl = std::make_unique<AlignedBufferImpl>(std::align_val_t{alignment}, size);
      memcpy(impl->get(), buffer->data(), result_size);
      buffer.data_ = {impl->get(), result_size};
      // Note this may free the old buffer, so it must be after the copying.
      buffer.impl_ = std::move(impl);
    }
  }

  return result;
}

}  // namespace zxdump
