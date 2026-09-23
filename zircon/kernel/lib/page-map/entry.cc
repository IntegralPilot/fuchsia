// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "lib/page-map/entry.h"

#include <lib/page-map.h>

#include <vm/vm_address_region.h>
#include <vm/vm_object_paged.h>

namespace page_map::internal {

Entry::Entry(PageMap& page_map, fbl::RefPtr<VmObjectPaged> vmo, fbl::RefPtr<VmMapping> mapping)
    : page_map_(page_map), vmo_{ktl::move(vmo)}, mapping_{ktl::move(mapping)} {}

Entry::~Entry() {
  DEBUG_ASSERT_MSG(accessor_count_ == 0, "%lu", accessor_count_);

  const uint64_t page_offset_in_vmo = mapping_->object_offset();
  zx_status_t status = mapping_->Destroy();
  DEBUG_ASSERT_MSG(status == ZX_OK, "%d", status);

  const size_t kMappingSize = kPageSize;
  vmo_->Unpin(page_offset_in_vmo, kMappingSize);
}

void Entry::Release() { page_map_.Release(*this); }

}  // namespace page_map::internal
