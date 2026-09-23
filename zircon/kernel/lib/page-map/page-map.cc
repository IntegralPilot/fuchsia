// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "lib/page-map.h"

namespace page_map {

PageMap PageMap::gPageMap_;

zx::result<object_cache::UniquePtr<internal::Entry>> PageMap::MakeEntry(
    fbl::RefPtr<VmObjectPaged> vmo, size_t page_offset_in_vmo) {
  // Commit and pin.
  const size_t kMappingSize = kPageSize;
  zx_status_t status = vmo->CommitRangePinned(page_offset_in_vmo, kMappingSize, /*write=*/true);
  if (status != ZX_OK) {
    return zx::error(status);
  }
  auto unpin = fit::defer([&]() { vmo->Unpin(page_offset_in_vmo, kMappingSize); });

  // Then map into the kernel aspace.
  fbl::RefPtr<VmAddressRegion> kernel_vmar =
      VmAspace::kernel_aspace()->RootVmar()->as_vm_address_region();
  zx::result<VmAddressRegion::MapResult> mapping_result =
      kernel_vmar->CreateVmMapping(0, kMappingSize, 0, 0, vmo, page_offset_in_vmo,
                                   ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE, "PageMap");
  if (mapping_result.is_error()) {
    return mapping_result.take_error();
  }
  auto unmap = fit::defer([&]() { mapping_result->mapping->Destroy(); });

  // Fault-in the range.
  status = mapping_result->mapping->MapRange(0, kMappingSize, true);
  if (status != ZX_OK) {
    return zx::error_result(status);
  }

  auto new_entry = allocator_.Allocate(*this, ktl::move(vmo), ktl::move(mapping_result->mapping));
  if (new_entry.is_error()) {
    return new_entry.take_error();
  }
  unmap.cancel();
  unpin.cancel();

  return zx::ok(ktl::move(new_entry.value()));
}

void PageMap::Release(internal::Entry& entry) {
  // If we're releasing the last reference we need to destroy the entry.  However, we must limit the
  // number of "unfindable" entries in order to not exceed the max pin count (see
  // unfindable_entry_count_ declaration for more).
  //
  // To ensure that the mutex-guarded critical section remains fast, we employ a two phase approach.
  // We lookup the entry and if the count is greater than one, we simply decrement and return.
  // However, if this was the last reference (i.e. count is one) we must back out, consume an
  // unfindable_entry_count_, and try again.  If this second attempt succeeds in releasing the last
  // reference, we remove the entry from the map and destroy it before posting the semaphore.

  // Last reference?
  {
    Guard<CriticalMutex> guard{get_lock()};
    if (entry.accessor_count() > 1) {
      entry.DecrementAccessorCount();
      // This wasn't the last reference.  We're done.
      return;
    }
  }

  // We're likely releasing the last reference.  Acquire the semaphore before we make the entry
  // unfindable.
  zx_status_t status = unfindable_entry_count_.Wait();
  ASSERT(status == ZX_OK);

  // Make sure we post the semaphore on all exits.  This must be done after any removed entry has
  // been destroyed to ensure the pin count is reduced prior to potentially letting another thread
  // pin.
  auto post = fit::defer([&]() { unfindable_entry_count_.Post(); });

  Map::PtrType entry_destroyer;
  {
    Guard<CriticalMutex> guard{get_lock()};
    if (entry.DecrementAccessorCount()) {
      // This was indeed the last one.  Remove it from the container so we can destroy it without
      // holding the container's lock.
      entry_destroyer = map_.erase(entry);
    }
  }
}

}  // namespace page_map
