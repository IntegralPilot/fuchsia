// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-options/boot-options.h>
#include <lib/memalloc/pool.h>
#include <lib/zbi-format/memory.h>
#include <lib/zbi-format/zbi.h>
#include <lib/zbitl/view.h>
#include <zircon/assert.h>

#include <ktl/array.h>
#include <ktl/limits.h>
#include <phys/address-space.h>
#include <phys/allocation.h>
#include <phys/early-boot.h>
#include <phys/main.h>
#include <phys/symbolize.h>

#include <ktl/enforce.h>

void ZbiInitMemory(const void* zbi_ptr, EarlyBootZbi zbi, ktl::span<zbi_mem_range_t> mem_config,
                   ktl::optional<memalloc::Range> extra_special_range, AddressSpace* aspace) {
  uint64_t phys_start = reinterpret_cast<uint64_t>(PHYS_LOAD_ADDRESS);
  uint64_t phys_end = reinterpret_cast<uint64_t>(_end);
  memalloc::Range special_memory_ranges[3] = {
      {
          .addr = phys_start,
          .size = phys_end - phys_start,
          .type = memalloc::Type::kPhysKernel,
      },
      {
          .addr = reinterpret_cast<uint64_t>(zbi_ptr),
          .size = zbi.size_bytes(),
          .type = memalloc::Type::kDataZbi,
      },
  };

  ktl::span<memalloc::Range> zbi_ranges(memalloc::AsRanges(mem_config));
  ktl::span<memalloc::Range> special_ranges(special_memory_ranges);
  if (extra_special_range) {
    special_ranges.back() = *extra_special_range;
  } else {
    special_ranges = special_ranges.subspan(0, special_ranges.size() - 1);
  }

  // If the data cache is disabled, its re-enabling will occur within
  // ArchSetUpAddressSpace(), which is gated on providing a non-null
  // AddressSpace. In the case where we intend to re-enable it, we must ensure
  // the coherence of allocations made now with the re-enabled state.
  memalloc::Pool::AccessCallback early_access;
  if (aspace) {
    early_access = [](uint64_t addr, uint64_t size) {
      ArchEarlyBootSyncAllocation(reinterpret_cast<uintptr_t>(addr), static_cast<size_t>(size));
    };
  }

  Allocation::Init(zbi_ranges, special_ranges, ktl::move(early_access));

  // Now that memory is accounted for, truncate the address range before any
  // further allocations.
  memalloc::Pool& pool = Allocation::GetPool();
  if (BootOptions::Get()->memory_limit_mb > 0) {
    constexpr uint64_t kBytesPerMib = 0x100'000;
    uint64_t limit_mb = ktl::min(ktl::numeric_limits<uint64_t>::max() / kBytesPerMib,
                                 BootOptions::Get()->memory_limit_mb);
    ZX_ASSERT(pool.TruncateTotalRam(kBytesPerMib * limit_mb).is_ok());
  }

  // Set up our own address space.
  if (aspace) {
    ArchSetUpAddressSpace(*aspace);

    // Data cache is now re-enabled and all allocation access should be kosher
    // again.
    Allocation::GetPool().set_access_callback([](uint64_t addr, uint64_t size) {});
  }

  if (BootOptions::Get()->phys_verbose) {
    pool.PrintMemoryRanges(ProgramName());
  }
}
