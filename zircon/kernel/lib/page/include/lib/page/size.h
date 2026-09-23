// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_PAGE_INCLUDE_LIB_PAGE_SIZE_H_
#define ZIRCON_KERNEL_LIB_PAGE_INCLUDE_LIB_PAGE_SIZE_H_

#include <lib/arch/paging-traits.h>
#include <lib/arch/paging.h>

#include <bit>
#include <cstddef>

#ifndef LIB_ARCH_PAGING_CONFIGURATION
#error "LIB_ARCH_PAGING_CONFIGURATION not defined?!"
#endif

namespace internal {

constexpr arch::PagingConfiguration kConfiguration =
    arch::PagingConfigurationFromString(LIB_ARCH_PAGING_CONFIGURATION);

#undef LIB_ARCH_PAGING_CONFIGURATION

using Paging = arch::Paging<arch::UpperPagingTraits<kConfiguration>>;

}  // namespace internal

// The selected page size.
constexpr size_t kPageSize = internal::Paging::kPageSize;

// The shift of the first level of virtual address mask used for page table
// walking.
constexpr size_t kPageShift = std::countr_zero(kPageSize);

constexpr uintptr_t kPageMask = uintptr_t{kPageSize} - 1;

// The size in bytes of a page table.
constexpr size_t kPageTableSize = internal::Paging::kTableSize;

// The number of page table levels.
constexpr size_t kNumPageTableLevels = internal::Paging::kLevels.size();

// The number of entries within a page table.
constexpr size_t kNumPageTableEntries = internal::Paging::kNumTableEntries;

// The additional bit shift of the virtual address mask used for page table
// walking.
constexpr size_t kPageTableLevelShift = internal::Paging::kNumTableEntriesLog2;

// The number of addressable bits in a virtual address.
constexpr size_t kVirtualAddressSize = internal::Paging::kVirtualAddressSize;

// A mask for the addressable bits of a virtual address.
constexpr uintptr_t kVirtualAddressMask = (uintptr_t{1} << kVirtualAddressSize) - 1;

constexpr bool IsPageRounded(uintptr_t x) { return (x & kPageMask) == 0; }

constexpr uintptr_t RoundDownPageSize(uintptr_t x) { return x & -uintptr_t{kPageSize}; }

constexpr uintptr_t RoundUpPageSize(uintptr_t x) {
  return (x + uintptr_t{kPageSize} - 1) & -uintptr_t{kPageSize};
}

#endif  // ZIRCON_KERNEL_LIB_PAGE_INCLUDE_LIB_PAGE_SIZE_H_
