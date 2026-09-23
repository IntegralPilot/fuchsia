// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/virtualization/tests/hypervisor/arch.h"

#include <lib/arch/arm64/memory.h>
#include <lib/arch/arm64/paging-traits.h>
#include <lib/arch/paging.h>
#include <lib/stdcompat/bit.h>
#include <lib/stdcompat/span.h>

#include <cstdint>
#include <optional>

#include <fbl/algorithm.h>
#include <hwreg/array.h>

#include "constants.h"
#include "src/virtualization/tests/hypervisor/hypervisor_tests.h"

namespace {

using PagingTraits = arch::ArmPagingTraits<arch::ArmPagingConfiguration::k4k39Bit, arch::ArmVirtualAddressRange::kLower>;
using Paging = arch::Paging<PagingTraits>;

}  // namespace

void SetUpGuestPageTable(cpp20::span<uint8_t> guest_memory) {
  ZX_ASSERT(!guest_memory.empty());

  constexpr std::optional<arch::ArmMairAttribute> kAttr =
      arch::ArmMemoryAttrIndirectionRegister::AttributeFromValue(MAIR_ATTR_NORMAL_CACHED);
  ZX_ASSERT(kAttr);
  auto mair = arch::ArmMemoryAttrIndirectionRegister::Get().FromValue(0).SetAttribute(0, *kAttr);

  const auto state = arch::ArmSystemPagingState{
      .mair = mair,
      .shareability = arch::ArmShareabilityAttribute::kInner,
      .el1 = true,
  };

  constexpr Paging::MapSettings kMapSettings{
      .access =
          {
              .readable = true,
              .writable = true,
              .executable = true,
          },
      .memory = *kAttr,
  };

  auto paddr_to_io = [memory = guest_memory](uint64_t paddr) {
    using Table = hwreg::AlignedTableStorage<uint64_t, 512>;
    return reinterpret_cast<Table*>(reinterpret_cast<uintptr_t>(&memory[paddr]))->direct_io();
  };

  auto pt_allocator = [next_paddr = uint64_t{PAGE_TABLE_PADDR},
                       paddr_end = uint64_t{PAGE_TABLE_PADDR} + uint64_t{PAGE_TABLE_SIZE}](
                          uint64_t size, uint64_t alignment) mutable -> std::optional<uint64_t> {
    uint64_t allocation_start = fbl::round_up(next_paddr, alignment);
    if (allocation_start >= paddr_end || size > paddr_end - allocation_start) {
      return std::nullopt;
    }
    next_paddr = allocation_start + size;
    return allocation_start;
  };

  // Allocate the root page table.
  pt_allocator(Paging::kTableSize, Paging::kTableAlignment);

  // Map virtual memory 1:1 to physical memory.
  fit::result result = Paging::Map(PAGE_TABLE_PADDR, paddr_to_io, std::move(pt_allocator), state, 0,
                                   cpp20::bit_ceil(guest_memory.size()), 0, kMapSettings);
  ZX_ASSERT(result.is_ok());
}
