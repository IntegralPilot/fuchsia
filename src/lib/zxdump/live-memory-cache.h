// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ZXDUMP_LIVE_MEMORY_CACHE_H_
#define SRC_LIB_ZXDUMP_LIVE_MEMORY_CACHE_H_

// Process::read_memory calls on live processes use a shared cache of past
// reads from any Process in the TaskHolder.  Actual reads from the live
// process are always done one whole page at a time.  The Buffer<> objects
// that are returned refer to portions of pages held in the cache.
//
// Each page read from a process is held in a LiveMemoryCache::Page object.
// There are three kinds of references to Page objects:
//  * The TaskHolder maintains a shared list of cached pages in LRU order.
//  * The Process has a map of vaddrs to cached pages read from that process.
//  * Buffer<> objects returned to Process::read_memory callers point to pages.
//
// The TaskHolder::LiveMemoryCache is the ultimate owner of Page objects.
// Every live Page object for any Process in the TaskHolder is on its shared
// cache list.  While live in the shared cache, each Page is also present in
// the Process::LiveMemory's local cache, which is indexed by vaddr.
//
// Each Buffer object returned by Process::read_memory on a live process owns a
// PageRef that keeps a Page live in the cache.  Page has a reference count of
// all live PageRef (i.e. Buffer) objects.  Only Page objects with zero
// references are eligible to be evicted from the cache.

#include <lib/zxdump/task.h>

#include <memory>

#include <fbl/intrusive_double_list.h>
#include <fbl/intrusive_wavl_tree.h>

namespace zxdump {

// This is embedded in the TaskHolder.
class TaskHolder::LiveMemoryCache {
 public:
  using PageContentsPtr = std::unique_ptr<std::byte[]>;

  class PageRef;  // Forward declaration.

  // The Page just holds a PageContentsPtr and its vaddr, plus a whole bunch of
  // bookkeeping necessary to be a reference-counted object in both an LRU list
  // and a vaddr index.
  class Page : public fbl::ContainableBaseClasses<
                   fbl::TaggedWAVLTreeContainable<Page*, Process,
                                                  fbl::NodeOptions::AllowMultiContainerUptr>,
                   fbl::TaggedDoublyLinkedListable<std::unique_ptr<Page>, TaskHolder,
                                                   fbl::NodeOptions::AllowMultiContainerUptr>> {
   public:
    Page(const Page&) = delete;

    Page(Process::LiveMemory& owner, uint64_t vaddr, PageContentsPtr contents)
        : vaddr_(vaddr), owner_(owner), contents_(std::move(contents)) {}

    ~Page();

    std::span<const std::byte> contents() const;

    // fbl::TaggedWAVLTree uses this to get the sort key.
    uint64_t GetKey() const { return vaddr_; }

    bool has_refs() const { return refs_ > 0; }

   private:
    friend PageRef;

    uint64_t vaddr_ = 0;
    Process::LiveMemory& owner_;
    PageContentsPtr contents_;
    unsigned int refs_ = 0;
  };

  // The Buffer<> objects returned by read_memory will own PageRef objects.
  // Creation and deletion of PageRef objects is the only way that a Page
  // object's ref count is changed.
  class PageRef final : public internal::BufferImpl {
   public:
    explicit PageRef(Page& page) : page_(page) { ++page_.refs_; }

    ~PageRef() override;

    Page& operator*() const { return page_; }

    Page* operator->() const { return &page_; }

   private:
    Page& page_;
  };

  using PageRefPtr = std::unique_ptr<PageRef>;

  using PageCache = fbl::TaggedDoublyLinkedList<std::unique_ptr<Page>, TaskHolder>;

  static constexpr size_t kDefaultCacheLimit = 2 << 20;  // 2 MiB

  size_t cache_limit() const { return cache_limit_; }

  void set_cache_limit(size_t limit) {
    cache_limit_ = limit;
    PruneCache();
  }

  // Insert a new Page into the cache, becoming the MRU page.
  // The returned PageRef owns its only ref.
  PageRefPtr NewPage(Process::LiveMemory& owner, uint64_t vaddr, PageContentsPtr contents);

  // Note the reuse of a page found in the cache, i.e. make it the MRU page.
  PageRefPtr Reuse(Page& page);

  // Account one page as contributing to the limited cache size.  This is
  // called when any Page reaches zero references.  (Pages with refs don't
  // count towards the cache limit, since the Process::read_memory caller is
  // still actively using them.)  Then discard LRU pages with no refs while the
  // cache size is above the limit.
  void OnCachedPage();

  // Trim down the cache as needed to fit under the limit.
  void PruneCache();

 private:
  PageCache cache_;
  size_t cache_limit_ = kDefaultCacheLimit;
  size_t cache_size_ = 0;
};

// The LiveMemory object just contains the local cache index.  It's small
// enough to live directly in the Process object, but it's separately allocated
// on demand just to keep the class definition hidden from the public API.
class Process::LiveMemory {
 public:
  using Page = TaskHolder::LiveMemoryCache::Page;

  using PageRef = TaskHolder::LiveMemoryCache::PageRef;

  using PageRefPtr = TaskHolder::LiveMemoryCache::PageRefPtr;

  using CacheIndex = fbl::TaggedWAVLTree<uint64_t, Page*, Process>;

  explicit LiveMemory(TaskHolder::LiveMemoryCache& shared_cache) : shared_cache_(shared_cache) {}

  fit::result<Error, Buffer<>> ReadLiveMemory(uint64_t vaddr, size_t size, ReadMemorySize size_mode,
                                              const LiveHandle& handle,
                                              TaskHolder::LiveMemoryCache& shared_cache);

  TaskHolder::LiveMemoryCache& shared_cache() { return shared_cache_; }

  CacheIndex& cache_index() { return cache_index_; }

 private:
  TaskHolder::LiveMemoryCache& shared_cache_;
  CacheIndex cache_index_;
};

}  // namespace zxdump

#endif  // SRC_LIB_ZXDUMP_LIVE_MEMORY_CACHE_H_
