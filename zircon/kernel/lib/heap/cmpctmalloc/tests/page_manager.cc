// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "page_manager.h"

#include <lib/page/size.h>

void* PageManager::AllocatePages(size_t num_pages) {
  if (num_calls_to_fail_ > 0) {
    --num_calls_to_fail_;
    return nullptr;
  }

  Block block(num_pages);
  ZX_ASSERT(
      Block::RangeIsCleanFilled(block.contents.get(), block.contents.get() + block.size_bytes));
  char* ptr = block.contents.get();
  ZX_ASSERT(IsPageRounded(ptr));
  blocks_.insert({ptr, std::move(block)});
  return ptr;
}

void PageManager::FreePages(void* p, size_t num_pages) {
  if (num_pages == 0) {
    return;
  }

  char* ptr = static_cast<char*>(p);
  ZX_ASSERT_MSG(IsPageRounded(ptr), "address %p is not page aligned", ptr);

  // lower_bound(ptr) points to the block in which |ptr| lives.
  BlockMap::iterator it = blocks_.lower_bound(ptr);
  ZX_ASSERT_MSG(it != blocks_.end(), "could not find a block containing %p", ptr);

  Block* block = &(it->second);
  ZX_ASSERT_MSG(ptr >= block->available_start && ptr < block->available_end(),
                "address %p is outside available subregion of block", ptr);

  size_t size_to_free = num_pages * kPageSize;
  size_t available_from_ptr = block->available_end() - ptr;
  ZX_ASSERT_MSG(size_to_free <= available_from_ptr,
                "cannot free %zu bytes from address %p; only %zu bytes available", size_to_free,
                ptr, available_from_ptr);

  bool freeing_head = ptr == block->available_start;
  bool freeing_tail = available_from_ptr == size_to_free;
  ZX_ASSERT_MSG(freeing_head || freeing_tail,
                "we only free heads or tails of available pages at any given time.");
  if (freeing_head) {
    block->FreeHead(num_pages);
  } else if (freeing_tail) {
    block->FreeTail(num_pages);
  }
  // If we were to free both a head and a tail, then we freed the whole
  // available subregion, at which point all pages in the block have been
  // freed and we we can stop tracking the block altogether.
  if (freeing_head && freeing_tail) {
    blocks_.erase(it);
    // Now that we've erased the entry, |block| is no longer valid.
    block = nullptr;
  }
}

void PageManager::FailNext(size_t count) { num_calls_to_fail_ = count; }

void PageManager::IncFailuresReported() { ++num_failures_reported_; }

size_t PageManager::GetFailuresReported() const { return num_failures_reported_; }
