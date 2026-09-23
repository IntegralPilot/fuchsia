// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_PAGE_MAP_INCLUDE_LIB_PAGE_MAP_ENTRY_H_
#define ZIRCON_KERNEL_LIB_PAGE_MAP_INCLUDE_LIB_PAGE_MAP_ENTRY_H_

#include <lib/fit/defer.h>
#include <lib/object_cache.h>
#include <lib/user_copy/internal.h>
#include <lib/zx/result.h>
#include <stdio.h>
#include <trace.h>
#include <zircon/errors.h>

#include <fbl/alloc_checker.h>
#include <fbl/intrusive_wavl_tree.h>
#include <fbl/ref_ptr.h>
#include <ktl/utility.h>
#include <vm/vm_address_region.h>
#include <vm/vm_object_paged.h>

class VmObjectPaged;
class VmMapping;

namespace page_map {
class PageMap;
}  // namespace page_map

namespace page_map::internal {

// Entry is logically private to Accessor and PageMap.
//
// An Entry in a PageMap that refers to a single mapped page of a VMO.
class Entry : public fbl::WAVLTreeContainable<object_cache::UniquePtr<Entry>> {
 public:
  // Identifies a page in a VMO using the VMO's address in memory and the page's offset within the
  // VMO.
  //
  // Why is it OK to use the object's address in the key?  It seems unlikely we'll implement a
  // garbage collection or object forwarding scheme that would result in multiple entries for a
  // single (logically) VMO.  And even if we do, it would likely manifest as an efficiency issue
  // rather than a correctness issue.
  //
  // Required by WAVLTreeContainable.
  struct Key {
    void* vm_object_paged_{};
    size_t page_offset_in_vmo{};

    auto operator<=>(const Key&) const = default;
    bool operator==(const Key&) const = default;
  };
  Key GetKey() const { return Key(vmo_.get(), mapping_->object_offset()); }

  Entry(PageMap& page_map, fbl::RefPtr<VmObjectPaged> vmo, fbl::RefPtr<VmMapping> mapping);

  ~Entry();

  // Increment this entry's accessor count.
  //
  // May only be called while holding the PageMap::get_lock().
  void IncrementAccessorCount() { accessor_count_ += 1; }

  // Decrement this entry's accessor count.  Returns when the ref-count has reached zero.
  //
  // May only be called while holding the PageMap::get_lock().
  bool DecrementAccessorCount() {
    DEBUG_ASSERT(accessor_count_ > 0);
    accessor_count_ -= 1;
    return (accessor_count_ == 0);
  }

  // Return this entry's accessor count.
  //
  // May only be called while holding the PageMap::get_lock().
  uint64_t accessor_count() const { return accessor_count_; }

  // Release an Accessor's reference to this instance.  Intended to be called by Accessor's dtor.
  void Release();

  vaddr_t mapping_base() const { return mapping_->base(); }

 private:
  // Logically TA_GUARDED by |page_map_.get_lock()|.
  //
  // TODO(maniscalco): Explore techniques that would allow us to mark this as TA_GUARDED without
  // paying for a reference to the owning PageMap.
  uint64_t accessor_count_{};
  PageMap& page_map_;
  const fbl::RefPtr<VmObjectPaged> vmo_;
  const fbl::RefPtr<VmMapping> mapping_;
};

}  // namespace page_map::internal

#endif  // ZIRCON_KERNEL_LIB_PAGE_MAP_INCLUDE_LIB_PAGE_MAP_ENTRY_H_
