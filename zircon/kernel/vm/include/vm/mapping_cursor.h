// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_VM_INCLUDE_VM_MAPPING_CURSOR_H_
#define ZIRCON_KERNEL_VM_INCLUDE_VM_MAPPING_CURSOR_H_

#include <assert.h>
#include <sys/types.h>

// Helper that tracks a virtual address range for performing MMU operations. This can be used
// directly, or by as part of a MappingCursor.
class VirtualAddressCursor {
 public:
  VirtualAddressCursor(vaddr_t vaddr, size_t size)
      : start_vaddr_(vaddr), vaddr_(vaddr), size_(size) {}

  // Sets offset used for |vaddr_rel| and returns true if the cursor lies within that offset and
  // some specified maximum. Should only be called before the cursor has started to be used.
  bool SetVaddrRelativeOffset(vaddr_t vaddr_rel_offset, size_t vaddr_rel_max) {
    DEBUG_ASSERT(start_vaddr_ == vaddr_);
    vaddr_t vaddr_rel = start_vaddr_ - vaddr_rel_offset;

    if (vaddr_rel > vaddr_rel_max - size_ || size_ > vaddr_rel_max) {
      return false;
    }
    vaddr_rel_offset_ = vaddr_rel_offset;
    return true;
  }

  // Update the cursor to skip over a not-present page table entry.
  void SkipEntry(size_t ps) {
    // Cannot just increase by given size as the both the current or final vaddr may not be aligned
    // to this given size. These cases only happen as the very first or very last entry we will
    // examine respectively, but still must be handled here.

    // Calculate the amount the cursor should skip to get to the next entry at
    // this page table level.
    const size_t next_entry_offset = ps - (vaddr_ & (ps - 1));
    // If our endpoint was in the middle of this range, clamp the
    // amount we remove from the cursor
    const size_t consume = (size_ > next_entry_offset) ? next_entry_offset : size_;

    DEBUG_ASSERT(size_ >= consume);
    size_ -= consume;
    vaddr_ += consume;
  }

  void Consume(size_t ps) {
    DEBUG_ASSERT(size_ >= ps);
    vaddr_ += ps;
    size_ -= ps;
  }

  // Returns a new cursor to the, possibly empty, virtual range that has already been processed by
  // this cursor. The returned cursor will always be a subset of the original cursors range.
  VirtualAddressCursor ProcessedRange() const {
    VirtualAddressCursor ret(start_vaddr_, vaddr_ - start_vaddr_);
    // As our new cursor is a subrange we know the relative offset will always be valid.
    ret.vaddr_rel_offset_ = vaddr_rel_offset_;
    return ret;
  }

  vaddr_t vaddr() const { return vaddr_; }

  vaddr_t vaddr_rel() const { return vaddr_ - vaddr_rel_offset_; }

  size_t size() const { return size_; }

 private:
  vaddr_t start_vaddr_;
  vaddr_t vaddr_;
  vaddr_t vaddr_rel_offset_ = 0;
  size_t size_;
};

// Helper class for MMU implementations to track physical address ranges when installing mappings.
// If just processing a virtual address range, such as for unmapping, then the VirtualAddressCursor
// can be used instead.
class MappingCursor {
 public:
  MappingCursor(const paddr_t* paddrs, size_t paddr_count, size_t page_size, vaddr_t vaddr)
      : paddrs_(paddrs), page_size_(page_size), vaddr_cursor_(vaddr, page_size * paddr_count) {
#ifdef DEBUG_ASSERT_IMPLEMENTED
    paddr_count_ = paddr_count;
#endif
  }

  // See VirtualAddressCursor::SetVaddrRelativeOffset.
  bool SetVaddrRelativeOffset(vaddr_t vaddr_rel_offset, size_t vaddr_rel_max) {
    return vaddr_cursor_.SetVaddrRelativeOffset(vaddr_rel_offset, vaddr_rel_max);
  }

  void Consume(size_t ps) {
    paddr_consumed_ += ps;
    DEBUG_ASSERT(paddr_consumed_ <= page_size_);
    if (paddr_consumed_ == page_size_) {
      paddrs_++;
      paddr_consumed_ = 0;
#ifdef DEBUG_ASSERT_IMPLEMENTED
      DEBUG_ASSERT(paddr_count_ > 0);
      paddr_count_--;
#endif
    }
    vaddr_cursor_.Consume(ps);
  }

  paddr_t paddr() const {
    DEBUG_ASSERT(paddr_consumed_ < page_size_);
    return (*paddrs_) + paddr_consumed_;
  }

  size_t PageRemaining() const { return page_size_ - paddr_consumed_; }

  // Returns a new cursor to the, possibly empty, virtual range that has already been processed by
  // this cursor. The returned cursor will always be a subset of the original cursors range and
  // does not include the paddrs.
  VirtualAddressCursor ProcessedRange() const { return vaddr_cursor_.ProcessedRange(); }

  vaddr_t vaddr() const { return vaddr_cursor_.vaddr(); }

  vaddr_t vaddr_rel() const { return vaddr_cursor_.vaddr_rel(); }

  size_t size() const { return vaddr_cursor_.size(); }

 private:
  const paddr_t* paddrs_;
#ifdef DEBUG_ASSERT_IMPLEMENTED
  // We have no need to actually track the total number of elements in the paddrs array, as this
  // should be a simple size/paddr_size. To guard against code mistakes though, we separately track
  // this just in debug mode.
  size_t paddr_count_ = 0;
#endif
  size_t paddr_consumed_ = 0;
  size_t page_size_;

  VirtualAddressCursor vaddr_cursor_;
};

#endif  // ZIRCON_KERNEL_VM_INCLUDE_VM_MAPPING_CURSOR_H_
