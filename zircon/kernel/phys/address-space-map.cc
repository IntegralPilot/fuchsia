// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <phys/address-space.h>
#include <phys/allocation.h>

// This method is in a separate file and separate GN phys:address-space-map
// static_library() so that its users don't need to link in the bootstrapping
// dependencies if they only need to Map into an existing AddressSpace as in a
// physload module.

fit::result<AddressSpace::MapError> AddressSpace::Map(uint64_t vaddr, uint64_t size, uint64_t paddr,
                                                      AddressSpace::MapSettings settings) {
  ZX_ASSERT_MSG(vaddr < kLowerVirtualAddressRangeEnd || vaddr >= kUpperVirtualAddressRangeStart,
                "virtual address %#" PRIx64 " must be < %#" PRIx64 " or >= %#" PRIx64, vaddr,
                kLowerVirtualAddressRangeEnd, kUpperVirtualAddressRangeStart);

  bool upper = vaddr >= kUpperVirtualAddressRangeStart;

  // Fix-up settings per documented behavior.
  if constexpr (!kExecuteOnlyAllowed) {
    settings.access.readable |= !settings.access.writable && settings.access.executable;
  }

  settings.global = upper;

  if constexpr (kDualSpaces) {
    if (upper) {
      return UpperPaging::Map(upper_root_paddr_, paddr_to_io_, permanent_allocator(), state_, vaddr,
                              size, paddr, settings);
    }
  }

  // See allocator descriptions for the appropriate use-cases.
  auto lower_allocator = upper ? permanent_allocator() : temporary_allocator();
  return LowerPaging::Map(lower_root_paddr_, paddr_to_io_, lower_allocator, state_, vaddr, size,
                          paddr, settings);
}

ktl::optional<uint64_t> AddressSpace::AllocatePageTable(memalloc::Type type, uint64_t size,
                                                        uint64_t alignment) {
  auto result = Allocation::GetPool().Allocate(type, size, alignment, pt_allocation_lower_bound_,
                                               pt_allocation_upper_bound_);
  if (result.is_error()) {
    return ktl::nullopt;
  }
  auto addr = static_cast<uintptr_t>(result.value());
  memset(reinterpret_cast<void*>(addr), 0, static_cast<size_t>(size));
  return addr;
}
