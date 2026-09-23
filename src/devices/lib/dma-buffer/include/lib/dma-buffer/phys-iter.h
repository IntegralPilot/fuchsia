// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_LIB_DMA_BUFFER_INCLUDE_LIB_DMA_BUFFER_PHYS_ITER_H_
#define SRC_DEVICES_LIB_DMA_BUFFER_INCLUDE_LIB_DMA_BUFFER_PHYS_ITER_H_

#include <zircon/assert.h>
#include <zircon/syscalls.h>
#include <zircon/types.h>

#include <algorithm>
#include <utility>

namespace dma_buffer {

// PhysIter -- An iterator yielding physical address and size pairs of a buffer, given as a
// (zx_paddr_t, size_t) pair. These iterated pairs are suitable for hardware DMA transactions that
// consume a physical address and length of bytes. The buffer need not be backed by physically
// contiguous chunks (more below), or be chunk-aligned. The iterator will ensure chunk boundaries
// are not crossed in given (addr, size) pairs unless the origin VMO was pinned with
// ZX_BTI_CONTIGUOUS in effect.
//
// The chunk list is given as a list of zx_paddr_t values, which is chunk-count in size. These
// values are typically taken from the result of zx_bti_pin().
//
// Chunks -- When pinned, a VMO may be subject to the effects of either ZX_BTI_CONTIGUOUS or
// ZX_BTI_COMPRESS. These flags affect the size of contiguous bytes pointed at by the returned
// zx_paddr_t list. There are three possibilities:
//   1. If neither flag was supplied, the returned list entries will point to page-sized chunks.
//   2. If ZX_BTI_CONTIGUOUS was given, the list will contain a single entry, pointing at a sequence
//      of (physically) contiguous pages. The chunk size isn't used in this case.
//   3. If ZX_BTI_COMPRESS was given, the list entries will point to minimum-contiguity-sized
//      chunks. This is an internal parameter, chosen by the kernel, which may be looked up via
//      zx_object_get_info().
// Because this implementation seeks to not cross applicable chunk boundaries, it needs to know what
// the chunk sizing actually is. The chunk size is a parameter given during construction, and
// should be commensurate with how the origin VMO was actually pinned.
class PhysIter {
 private:
  class iterator_impl;

 public:
  PhysIter() = delete;

  PhysIter(const zx_paddr_t* chunk_list, uint64_t chunk_count, size_t chunk_size,
           zx_off_t vmo_offset, size_t buf_length, size_t max_length)
      : chunk_list_{chunk_list},
        chunk_count_{chunk_count},
        vmo_offset_{vmo_offset},
        buf_length_{buf_length},
        max_length_{max_length == 0 ? UINT64_MAX : max_length},
        chunk_size_{chunk_size} {
    ZX_ASSERT(chunk_list_ != nullptr);
    ZX_ASSERT(chunk_count_ > 0);
    ZX_ASSERT(buf_length_ > 0);

    // Chunk size must be a power-of-2 to support the chunk mask calculation.
    ZX_ASSERT(!(chunk_size_ & (chunk_size - 1)));
  }

  // This overload defaults to page-sized chunks.
  PhysIter(const zx_paddr_t* chunk_list, uint64_t chunk_count, zx_off_t vmo_offset,
           size_t buf_length, size_t max_length = UINT64_MAX)
      : PhysIter(chunk_list, chunk_count, zx_system_get_page_size(), vmo_offset, buf_length,
                 max_length) {}

  using iterator = iterator_impl;

  iterator begin() { return iterator{this, false}; }
  iterator cbegin() { return iterator{this, false}; }
  iterator end() { return iterator(this, true); }
  iterator cend() { return iterator(this, true); }

 private:
  class iterator_impl {
   public:
    iterator_impl() = delete;
    iterator_impl(const iterator_impl&) = default;
    iterator_impl(PhysIter* iter, bool is_end) : iter_{iter} {
      if (is_end) {
        current_ = {0, 0};
      } else {
        current_.first = iter_->chunk_list_[0] + iter_->vmo_offset_;
        current_.second = BoundaryTruncate(current_.first);
      }
    }

    bool operator==(const iterator_impl& rhs) const { return current_ == rhs.current_; }
    bool operator!=(const iterator_impl& rhs) const { return !(*this == rhs); }  // Symmetry.

    using PhysPair = std::pair<zx_paddr_t, size_t>;

    const PhysPair& operator*() { return current_; }

    // Prefix.
    iterator_impl& operator++() {
      if (current_.first == 0) {  // Terminal criteria.
        return *this;
      }

      iterated_bytes_ += current_.second;
      if (iterated_bytes_ == iter_->buf_length_) {
        current_ = {0, 0};
        return *this;
      }

      zx_paddr_t cur_chunk{current_.first & iter_->chunk_mask_};
      zx_paddr_t next_chunk{(current_.first + current_.second) & iter_->chunk_mask_};

      if (cur_chunk != next_chunk) {
        current_.first = iter_->chunk_list_[++chunk_index_];
      } else {
        current_.first += current_.second;
      }

      current_.second = BoundaryTruncate(current_.first);
      return *this;
    }

    // Postfix.
    iterator_impl operator++(int) {
      iterator_impl impl{*this};
      ++(*this);
      return impl;
    }

   private:
    // This function returns the count of bytes from the given address to the next applicable
    // boundary. The definition of a boundary is one of:
    //   1. The requested maximum length (i.e. iter_->max_length_).
    //   2. The distance to the end of the current chunk if the pinned chunks are not guaranteed
    //      contiguous.
    //   3. The distance to the end of the overall buffer.
    // For any given address, the nearest applicable boundary will be returned.
    size_t BoundaryTruncate(zx_paddr_t addr) {
      // Distance to end of the buffer, which may overrun a chunk.
      size_t end_of_buffer{iter_->buf_length_ - iterated_bytes_};

      if (iter_->chunk_count_ == 1) {
        // If the chunk count is 1, it means one of two things. Either the pinned VMO was created
        // with ZX_BTI_CONTIGUOUS in effect, or the VMO was backed by only a single chunk. In the
        // case where a contiguous VMO was created, the single zx_paddr_t points to the start of
        // some number of contiguous chunks backing the VMO.
        return std::min(iter_->max_length_, end_of_buffer);
      }

      // If the chunk count was not equal to one, we must concern ourselves with chunk transitions.
      // It's possible this sequence is contiguous by happenstance, which gives the potential for a
      // coalesce operation. At present, this implementation doesn't do that. We will break at the
      // next applicable transition even in the case of coincidental contiguity.
      zx_paddr_t next_chunk{(addr & iter_->chunk_mask_) + iter_->chunk_size_};

      // Distance to end of current chunk, which may overrun the buffer's length.
      size_t end_of_chunk{next_chunk - addr};

      return std::min(iter_->max_length_, std::min(end_of_chunk, end_of_buffer));
    }

    PhysIter* iter_;

    uint64_t chunk_index_{0};     // Index of the current chunk, relative to the chunk list.
    PhysPair current_;            // Current segment pointer.
    uint64_t iterated_bytes_{0};  // Total iterated bytes, updated at each iteration.
  };

  const zx_paddr_t* chunk_list_;  // List of chunk(s) backing buffer.
  const uint64_t chunk_count_;    // Length of chunk list.
  const zx_off_t vmo_offset_;     // Offset from VMO-start (i.e. first chunk) buffer is located.
  const size_t buf_length_;       // Total buffer length.
  const size_t max_length_;       // Max length of requested iterated bytes.

  // The chunk size represents the number of contiguous bytes pointed at by each entry in the chunk
  // list. This implementation can only account for chunk sizes that are a power of 2.
  const size_t chunk_size_;

  // The chunk mask, when applied, yields the address of the chunk a given address resides in.
  const zx_paddr_t chunk_mask_{UINT64_MAX ^ (chunk_size_ - 1)};
};

}  // namespace dma_buffer

#endif  // SRC_DEVICES_LIB_DMA_BUFFER_INCLUDE_LIB_DMA_BUFFER_PHYS_ITER_H_
