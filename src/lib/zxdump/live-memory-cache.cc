// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "live-memory-cache.h"

#include <lib/zx/process.h>
#include <lib/zxdump/buffer.h>
#include <zircon/assert.h>
#include <zircon/syscalls.h>

#include <cinttypes>
#include <span>

#include "buffer-impl.h"

namespace zxdump {
namespace {

const size_t kPagesize = zx_system_get_page_size();

}  // namespace

std::span<const std::byte> TaskHolder::LiveMemoryCache::Page::contents() const {
  return {contents_.get(), kPagesize};
}

TaskHolder::LiveMemoryCache::Page::~Page() {
  ZX_DEBUG_ASSERT(refs_ == 0);

  // Remove this page from the per-process cache index.
  owner_.cache_index().erase(*this);
}

TaskHolder::LiveMemoryCache::PageRef::~PageRef() {
  ZX_DEBUG_ASSERT(page_.refs_ > 0);
  if (--page_.refs_ == 0) {
    // When a page goes from some refs to no refs, it counts as cached.
    page_.owner_.shared_cache().OnCachedPage();
  }
}

TaskHolder::LiveMemoryCache::PageRefPtr TaskHolder::LiveMemoryCache::NewPage(
    Process::LiveMemory& owner, uint64_t vaddr, PageContentsPtr contents) {
  // Page is only constructed here and held in std::unique_ptr<Page>.
  auto page = std::make_unique<Page>(owner, vaddr, std::move(contents));

  // Insert the raw pointer into the owner's cache index.
  // The Page destructor will remove it from the index.
  owner.cache_index().insert(page.get());

  // The new page starts with one ref, owned by the return value.
  PageRefPtr ref = std::make_unique<PageRef>(*page);

  // Move the ownership to the end of the shared cache list, in MRU position.
  // This will be removed and destroyed only be OnCachedPage, below.
  cache_.push_back(std::move(page));

  return ref;
}

TaskHolder::LiveMemoryCache::PageRefPtr TaskHolder::LiveMemoryCache::Reuse(Page& page) {
  // Simply slice it out of the list and move it to the end (MRU position).
  cache_.push_back(cache_.erase(page));

  // Return a new reference to the now-MRU page.
  return std::make_unique<PageRef>(page);
}

void TaskHolder::LiveMemoryCache::OnCachedPage() {
  // One more page is now cached.
  cache_size_ += kPagesize;

  PruneCache();
}

void TaskHolder::LiveMemoryCache::PruneCache() {
  if (cache_size_ <= cache_limit_) {
    return;
  }

  // There are now too many pages cached, so drop the LRU cached page.  The
  // pages with refs don't count towards the cache size, so there must be a
  // page with no refs in the list.  But in LRU order there might be pages
  // with live refs earlier in the list.  They are kept there so that as soon
  // as their last ref is dropped by the past Process::read_memory caller
  // later destroying their Buffer, that page is eligible to be reclaimed
  // before pages from Process::read_memory calls more recent than that one.
  auto head = cache_.begin();
  ZX_DEBUG_ASSERT(head != cache_.end());
  while (head->has_refs()) {
    ++head;
    ZX_DEBUG_ASSERT(head != cache_.end());
  }

  // This returns the std::unique_ptr and so deletes the Page.
  // The Page destructor removes itself from its owner's cache index.
  cache_.erase(head++);
}

fit::result<Error, Buffer<>> Process::LiveMemory::ReadLiveMemory(
    uint64_t vaddr, size_t size, ReadMemorySize size_mode, const LiveHandle& handle,
    TaskHolder::LiveMemoryCache& shared_cache) {
  zx::unowned_process process(handle.get());
  ZX_DEBUG_ASSERT(process->is_valid());

  auto read_one_page = [this, process,
                        &shared_cache](uint64_t page_vaddr) -> fit::result<Error, PageRefPtr> {
    if (auto it = cache_index_.find(page_vaddr); it != cache_index_.end()) {
      // Found a cached page.  Mark it as reused and get a fresh reference.
      return fit::ok(shared_cache.Reuse(*it));
    }

    // This page isn't available in the cache, so read it from the process.
    TaskHolder::LiveMemoryCache::PageContentsPtr contents(new std::byte[kPagesize]);
    size_t bytes_read = 0;
    if (zx_status_t status =
            process->read_memory(page_vaddr, contents.get(), kPagesize, &bytes_read);
        status != ZX_OK) {
      return fit::error{Error{
          .op_ = "zx_process_read_memory",
          .status_ = status,
      }};
    }

    ZX_ASSERT_MSG(bytes_read == kPagesize,
                  "zx_process_read_memory on aligned address %#" PRIx64
                  " returned %#zx bytes vs expected page size %#zx",
                  page_vaddr, bytes_read, kPagesize);

    return fit::ok(shared_cache_.NewPage(*this, page_vaddr, std::move(contents)));
  };

  uint64_t first_page = vaddr & -kPagesize;
  uint64_t last_page = (vaddr + size - 1) & -kPagesize;

  // Most reads will fit inside a single page.
  if (first_page == last_page || size_mode == ReadMemorySize::kLess) {
    auto result = read_one_page(vaddr & -kPagesize);
    if (result.is_error()) {
      return result.take_error();
    }
    Buffer<> buffer;
    buffer.data_ = (**result)->contents().subspan(vaddr & (kPagesize - 1));
    if (size_mode == ReadMemorySize::kExact && buffer.data_.size_bytes() > size) {
      buffer.data_ = buffer.data_.subspan(0, size);
    }
    buffer.impl_ = *std::move(result);
    return fit::ok(std::move(buffer));
  }

  // The request spans multiple pages, so we'll read separate whole
  // pages and then copy out of them into a plain BufferImplVector.
  const size_t page_count = ((last_page - first_page) / kPagesize) + 1;
  std::vector<PageRefPtr> pages;
  pages.reserve(page_count);
  while (first_page <= last_page) {
    PageRefPtr page;
    if (auto result = read_one_page(first_page); result.is_ok()) {
      page = *std::move(result);
    } else {
      return result.take_error();
    }
    pages.push_back(std::move(page));
    first_page += kPagesize;
  }

  auto copy = std::make_unique<internal::BufferImplVector>();
  copy->reserve(size);
  for (PageRefPtr& ref : pages) {
    // Take ownership of the page so it can die as soon as its data is copied.
    PageRefPtr page = std::move(ref);

    auto contents = (*page)->contents().subspan(vaddr & (kPagesize - 1));
    if (contents.size_bytes() > size) {
      contents = contents.subspan(0, size);
    }
    copy->insert(copy->end(), contents.begin(), contents.end());

    vaddr &= -kPagesize;
    vaddr += kPagesize;
    size -= contents.size_bytes();
  }

  Buffer<> buffer;
  buffer.data_ = std::span(*copy);
  buffer.impl_ = std::move(copy);
  return fit::ok(std::move(buffer));
}

}  // namespace zxdump
