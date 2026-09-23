// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "root_resource_filter_ffi.h"

#include <lib/memalloc/range.h>

#include <ktl/optional.h>
#include <phys/handoff.h>
#include <region-alloc/region.h>

extern "C" void cpp_root_resource_filter_populate_finalized_ranges(
    void* context, root_resource_filter_range_cb_t callback) {
  // Add all (normalized) RAM ranges to the set of regions to deny.
  auto filter_for_ram = [](memalloc::Type type) -> ktl::optional<memalloc::Type> {
    // Treat reserved test RAM as MMIO.
    if (type == memalloc::Type::kTestRamReserve || !memalloc::IsRamType(type)) {
      return {};
    }
    return memalloc::Type::kFreeRam;
  };
  auto add_region = [context, callback](const memalloc::Range& range) {
    callback(context, range.addr, range.size);
    return true;
  };
  memalloc::NormalizeRanges(gPhysHandoff->memory.get(), add_region, filter_for_ram);

  // Also handle any special non-RAM ranges mapped by physboot.
  for (ralloc_region_t region : gPhysHandoff->mmio_deny.get()) {
    callback(context, region.base, region.size);
  }
}
