// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/x86/boot-cpuid.h>
#include <lib/fit/defer.h>
#include <lib/memalloc/pool.h>
#include <stdio.h>
#include <zircon/limits.h>

#include <fbl/algorithm.h>
#include <hwreg/x86msr.h>
#include <ktl/array.h>
#include <ktl/byte.h>
#include <ktl/limits.h>
#include <ktl/optional.h>
#include <ktl/span.h>
#include <phys/address-space.h>
#include <phys/allocation.h>
#include <phys/symbolize.h>

#include <ktl/enforce.h>

namespace {

using Paging = AddressSpace::LowerPaging;

// On x86-64, we don't have any guarantee that all the memory in our address
// space is actually mapped in.
//
// We use a bootstrap allocator consisting of memory from ".bss" to construct a
// real page table with.  Unused memory will be returned to the heap after
// initialisation is complete.
//
// Amount of memory reserved in .bss for allocation of page table data
// structures: We reserve 512kiB. On machines which only support at most 2 MiB
// page sizes, we need ~8 bytes per 2 MiB, allowing us to map ~128 GiB of
// RAM. On machines with 1 GiB page sizes, we can support ~64 TiB of RAM.
//
constexpr size_t kBootstrapMemoryBytes = 512 * 1024;

// Bootstrap memory pool.
alignas(ZX_MIN_PAGE_SIZE) ktl::array<ktl::byte, kBootstrapMemoryBytes> gBootstrapMemory;

void SetUpAddressSpace(AddressSpace& aspace) {
  aspace.Init();
  aspace.SetUpIdentityMappings();
  aspace.Install();
}

}  // namespace

void ArchSetUpAddressSpace(AddressSpace& aspace) {
  // Ensure that executable pages are allowed.
  hwreg::X86MsrIo msr;
  arch::X86ExtendedFeatureEnableRegisterMsr::Get().ReadFrom(&msr).set_nxe(1).WriteTo(&msr);

  memalloc::Pool& pool = Allocation::GetPool();

  uint64_t bootstrap_start = reinterpret_cast<uintptr_t>(gBootstrapMemory.data());
  uint64_t bootstrap_end = bootstrap_start + gBootstrapMemory.size();

  // Per the above, we Free() the .bss bootstrap region to be able to allocate
  // from it, and then clamp the global page table allocation bounds to it.
  if (pool.Free(bootstrap_start, gBootstrapMemory.size()).is_error()) {
    ZX_PANIC("Failed to free .bss page table bootstrap region [%#" PRIx64 ", %#" PRIx64 ")",
             bootstrap_start, bootstrap_end);
  }

  // But those allocations won't necessarily use all of it!  Whatever remains
  // will still be free RAM in the pool that can be used for any kind of future
  // allocation.  That includes ElfImage::Load and the like; those allocations'
  // vaddr ranges will become the new ElfImage vaddr ranges.  But those are
  // still inside the .bss region of the main module!  The ElfImage and
  // Symbolize bookkeeping does not expect overlapping modules.  So trim the
  // module's vaddr size (and final segment) to omit what's being taken here.
  gSymbolize->PruneFromMainModule(bootstrap_start, bootstrap_end);

  aspace.SetPageTableAllocationBounds(bootstrap_start, bootstrap_end);
  SetUpAddressSpace(aspace);

  // Our root page table will need to be installed on secondary CPUs in 32-bit
  // protected mode: accordingly we'll want it to be 32-bit addressable. Our
  // root page table though was allocated out of .bss, and this might
  // naturally exceed 4GiB depending on where we were loaded; if so, relocate
  // it to a lower address.
  constexpr uint64_t k4GiB = uint64_t{1} << 32;
  if (aspace.root_paddr() >= k4GiB) {
    auto result = pool.Allocate(memalloc::Type::kKernelPageTables, Paging::kTableSize,
                                Paging::kTableAlignment,
                                /*min_addr=*/ktl::nullopt,
                                /*max_addr=*/k4GiB);
    ZX_ASSERT(result.is_ok());
    uint64_t new_root_paddr = result.value();
    uint64_t bootstrap_root_paddr = aspace.root_paddr();
    memcpy(reinterpret_cast<void*>(new_root_paddr), reinterpret_cast<void*>(bootstrap_root_paddr),
           Paging::kTableSize);
    aspace.InstallNewRootTable(new_root_paddr);

    if (pool.Free(bootstrap_root_paddr, Paging::kTableSize).is_error()) {
      ZX_PANIC("Failed to free the bootstrap root page table at [%#" PRIx64 ", %#" PRIx64 ")",
               bootstrap_root_paddr, bootstrap_root_paddr + Paging::kTableSize);
    }
  }

  // Now that we've bootstrapped, we no longer have any allocation
  // restrictions.
  aspace.SetPageTableAllocationBounds(ktl::nullopt, ktl::nullopt);
}
