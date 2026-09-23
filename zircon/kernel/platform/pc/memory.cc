// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "platform/pc/memory.h"

#include <inttypes.h>
#include <lib/arch/x86/boot-cpuid.h>
#include <lib/memalloc/range.h>
#include <lib/page/size.h>
#include <lib/root_resource_filter.h>
#include <lib/zircon-internal/macros.h>
#include <platform.h>
#include <string.h>
#include <trace.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <arch/x86/bootstrap16.h>
#include <arch/x86/feature.h>
#include <arch/x86/mmu.h>
#include <dev/interrupt.h>
#include <efi/boot-services.h>
#include <fbl/algorithm.h>
#include <kernel/range_check.h>
#include <ktl/iterator.h>
#include <lk/init.h>
#include <object/handle.h>
#include <object/resource_dispatcher.h>
#include <phys/handoff.h>
#include <vm/vm.h>

#include <ktl/enforce.h>

#define LOCAL_TRACE 0

// Discover the basic memory map.
void pc_mem_init(ktl::span<const memalloc::Range> ranges) {
  pmm_checker_init_from_cmdline();

  // We don't want RAM under 1MiB to feature in to any arenas, so normalize that
  // here.
  ktl::span<const memalloc::Range> low_reserved;
  {
    size_t low_reserved_end = 0;
    while (low_reserved_end < ranges.size() &&
           ranges[low_reserved_end].type == memalloc::Type::kReservedLow) {
      ++low_reserved_end;
    }
    low_reserved = ranges.first(low_reserved_end);
    ranges = ranges.last(ranges.size() - low_reserved.size());
  }

  if (zx_status_t status = pmm_init(ranges); status != ZX_OK) {
    TRACEF("Error adding arenas from provided memory tables: error = %d\n", status);
  }

  // Find an area that we can use for 16 bit bootstrapping of other SMP cores.
  constexpr uint64_t kAllocSize = k_x86_bootstrap16_buffer_size;
  constexpr uint64_t kMinBase = 2UL * kPageSize;
  bool bootstrap16 = false;
  for (const memalloc::Range& range : low_reserved) {
    uint64_t base = ktl::max(range.addr, kMinBase);
    if (base >= range.end() || range.end() - base < kAllocSize) {
      continue;
    }

    // We have a valid range.
    LTRACEF("Selected %" PRIxPTR " as bootstrap16 region\n", base);
    x86_bootstrap16_init(base);
    bootstrap16 = true;
    break;
  }
  if (!bootstrap16) {
    TRACEF("WARNING - Failed to assign bootstrap16 region, SMP won't work\n");
  }
}

namespace {

// Initialize the higher level PhysicalAspaceManager after the heap is initialized.
void x86_resource_init_hook(unsigned int rl) {
  // An error is likely fatal if the bookkeeping is broken and driver
  ResourceDispatcher::InitializeAllocator(
      ZX_RSRC_KIND_MMIO, 0,
      (1ull << arch::BootCpuid<arch::CpuidAddressSizeInfo>().phys_addr_bits()));
  ResourceDispatcher::InitializeAllocator(ZX_RSRC_KIND_IOPORT, 0, UINT16_MAX + 1);
  ResourceDispatcher::InitializeAllocator(ZX_RSRC_KIND_SYSTEM, 0, ZX_RSRC_SYSTEM_COUNT);

  // debug_uart.irq needs to be reserved here. See https://fxbug.dev/42109187.
}

LK_INIT_HOOK(x86_resource_init, x86_resource_init_hook, LK_INIT_LEVEL_HEAP)

}  // namespace
