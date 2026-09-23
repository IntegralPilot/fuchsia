// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_PAGE_MAP_INCLUDE_LIB_PAGE_MAP_H_
#define ZIRCON_KERNEL_LIB_PAGE_MAP_INCLUDE_LIB_PAGE_MAP_H_

#include <lib/fit/defer.h>
#include <lib/object_cache.h>
#include <lib/page-map/accessor.h>
#include <lib/page-map/entry.h>
#include <lib/user_copy/internal.h>
#include <lib/zx/result.h>
#include <stdio.h>
#include <trace.h>
#include <zircon/errors.h>

#include <fbl/alloc_checker.h>
#include <fbl/intrusive_wavl_tree.h>
#include <fbl/ref_ptr.h>
#include <vm/vm_address_region.h>
#include <vm/vm_object_paged.h>

namespace page_map::internal {
class Entry;
}  // namespace page_map::internal

namespace page_map {

// PageMap provides safe and efficient kernel access to objects within VMOs that are logically owned
// by usermode.  PageMap automatically handles the mapping, committing, and pinning of the
// underlying pages.
//
// There are limitations on the kinds of objects that may be accessed via PageMap.  They:
//   - must be trivially copyable, have standard layout, no implicit padding, etc.
//   - must not span page boundaries
//   - must have an alignment requirement of less than or equal to one page
//   - must be naturally aligned within the VMO
//
// An object, or a field of an object, may be read/written using an Accessor instance (see
// |MakeAccessor|).  An Accessor provides access to a single object, at a given offset within a VMO.
// The offset, together with the VMO's address in memory identify a page, allowing PageMap to
// internally ref-count mappings so that all Accessor instances for the same VMO and offset will
// reference a single mapping.  See also |Entry::Key|.
//
// Accessor is designed to avoid TOCTOU hazards by providing Read/Write methods for access rather
// than a pointer.
//
// Instances of PageMap are safe for concurrent use (thread-safe).
//
// TODO(maniscalco): To save memory, explore using the existing physmap mapping instead of creating
// a new one for each Entry.
//
// TODO(maniscalco): Once it's available, use the btree instead of a WAVL tree for storing Entry's
//
class PageMap {
 public:
  PageMap() = default;

  // Constructs an Accessor for the object at |object_offset_in_vmo| in |vmo|.
  //
  // Maps, commits, pins as necessary.
  //
  // Accessors act as ref-counted pointers for the underlying VMO and mapping.  Two Accessors for
  // the same offset in the same VMO will result in only one mapping.  When the last Accessor for a
  // given page is destroyed, the underlying mapping, and possibly its VMO, will be destroyed.  To
  // avoid unnecessary lock coupling, do not allow an Accessor to be destroyed while holding another
  // lock.
  //
  // For the purposes of mapping reuse, VMO "sameness" is determined by the VMO's address in
  // memory (i.e. |vmo.get()|).
  //
  // The object must have an alignment requirements of one page or less, and must be naturally
  // aligned within the VMO.
  //
  // The object may not straddle a page boundary.
  //
  // On success, returns a valid Accessor.
  //
  // Returns ZX_ERR_INVALID_ARGS if the object straddles a page boundary or the object is not
  // properly aligned.
  //
  // Returns ZX_ERR_OUT_OF_RANGE if the provided offset triggers arithmetic overflow.
  template <typename Object>
  zx::result<Accessor<Object>> MakeAccessor(fbl::RefPtr<VmObjectPaged> vmo,
                                            size_t object_offset_in_vmo);

  // Returns a reference to the global PageMap.
  static PageMap& Get() { return gPageMap_; }

 private:
  // So it can call |Release|.
  friend internal::Entry;

  // Make an entry for the given VMO and offset.  Commits, pins, and maps the specified page.
  zx::result<object_cache::UniquePtr<internal::Entry>> MakeEntry(fbl::RefPtr<VmObjectPaged> vmo,
                                                                 size_t page_offset_in_vmo);

  // Wraps an Entry with an Accessor.
  template <typename Object>
  static Accessor<Object> WrapEntry(internal::Entry& entry, size_t object_offset_in_page) {
    vaddr_t object_address;
    ASSERT(!add_overflow(entry.mapping_base(), object_offset_in_page, &object_address));
    Object* object = reinterpret_cast<Object*>(object_address);
    entry.IncrementAccessorCount();
    return Accessor<Object>(&entry, object);
  }

  // May destroy |entry|.
  void Release(internal::Entry& entry);

  Lock<CriticalMutex>* get_lock() const { return &lock_; }

  // The global PageMap.
  static PageMap gPageMap_;

  mutable DECLARE_CRITICAL_MUTEX(PageMap) lock_;
  using Map = fbl::WAVLTree<internal::Entry::Key, object_cache::UniquePtr<internal::Entry>>;
  Map map_ TA_GUARDED(get_lock());

  // Semaphore that restricts the number of "unfindable" entries to avoid pin count failures (see
  // usage in MakeAccessor and Release for more).
  //
  // TODO(https://fxbug.dev/498989748): Once the pin count limit is gone this can be removed.
  Semaphore unfindable_entry_count_{VM_PAGE_OBJECT_MAX_PIN_COUNT - 1};

  // The allocator from which Entry's are created.
  object_cache::ObjectCache<internal::Entry> allocator_{/* reserve_slabs */ 1};
};

template <typename Object>
inline zx::result<Accessor<Object>> PageMap::MakeAccessor(fbl::RefPtr<VmObjectPaged> vmo,
                                                          size_t object_offset_in_vmo) {
  static_assert(sizeof(Object) > 0);
  static_assert(sizeof(Object) <= kPageSize);
  static_assert(alignof(Object) <= kPageSize);

  // Make sure the object does not span pages.
  //
  // last is the offset in the vmo of the object's last byte.
  size_t last;
  if (add_overflow(object_offset_in_vmo, sizeof(Object) - 1, &last)) {
    return zx::error(ZX_ERR_OUT_OF_RANGE);
  }
  // Make sure the object's first byte and last byte are in the same page.
  const size_t page_offset_in_vmo = RoundDownPageSize(object_offset_in_vmo);
  if (RoundDownPageSize(last) != page_offset_in_vmo) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  // Make sure the object is properly aligned.
  if (object_offset_in_vmo % alignof(Object) != 0) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  // Form a key for the page.
  //
  size_t object_offset_in_page;
  ASSERT(!sub_overflow(object_offset_in_vmo, page_offset_in_vmo, &object_offset_in_page));
  const internal::Entry::Key key{vmo.get(), page_offset_in_vmo};

  // Do we already have an entry for this page?
  {
    Guard<CriticalMutex> guard{get_lock()};

    // Lookup an entry by its key.
    Map::iterator iter = map_.find(key);

    // If found, make an Accessor and return it.
    if (iter != map_.end()) {
      return zx::ok(WrapEntry<Object>(*iter, object_offset_in_page));
    }
  }

  // Before calling MakeEntry acquire the semaphore to restrict the number of concurrent MakeEntry
  // calls. This ensures that even in the worst case we will not attempt to pin a single page so
  // many times that we get an error. Although the semaphore applies to all allocations across all
  // pages of all vmos, and not just to a single page or even a single vmo, more than 31 threads are
  // unlikely to have meaningful parallelism as they will be serialized through both the PMM and the
  // kernel aspace locks respectively.
  // TODO(https://fxbug.dev/498989748): Once the pin count limit is gone this can be removed.
  zx_status_t status = unfindable_entry_count_.Wait();
  if (status != ZX_OK) {
    return zx::error{status};
  }
  // Make sure we post the semaphore on all exits. This must be done after any new_entry is
  // destroyed to ensure the pin count is reduced prior to potentially letting another thread pin.
  auto post = fit::defer([&]() { unfindable_entry_count_.Post(); });

  // No entry found, make one.  Take care to do so without holding the lock.  If we later find that
  // someone "beat us to it" then make sure we destroy our newly created entry without holding the
  // lock.
  zx::result<Map::PtrType> new_entry = MakeEntry(ktl::move(vmo), page_offset_in_vmo);
  if (new_entry.is_error()) {
    return new_entry.take_error();
  }

  Guard<CriticalMutex> guard{get_lock()};

  Map::iterator iter;
  bool inserted = map_.insert_or_find(ktl::move(new_entry.value()), &iter);
  // See that insert_or_find either inserted new_entry and consumed (moved) it, or that it found an
  // existing entry and left new_entry alone.
  DEBUG_ASSERT_MSG(inserted || new_entry.value().get() != nullptr, "inserted=%d, new_entry=%p",
                   inserted, new_entry.value().get());

  return zx::ok(WrapEntry<Object>(*iter, object_offset_in_page));
}

}  // namespace page_map

#endif  // ZIRCON_KERNEL_LIB_PAGE_MAP_INCLUDE_LIB_PAGE_MAP_H_
