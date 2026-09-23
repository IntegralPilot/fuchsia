// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <align.h>
#include <lib/acpi_lite.h>
#include <lib/acpi_lite/zircon.h>
#include <lib/page/size.h>
#include <zircon/compiler.h>

#include <kernel/range_check.h>
#include <ktl/limits.h>
#include <vm/physmap.h>
#include <vm/vm_aspace.h>
#include <vm/vm_object_physical.h>

namespace {
// AcpiParser requires a ZirconPhysmemReader instance that outlives
// it. We share a single global instance for all AcpiParser instances.
acpi_lite::ZirconPhysmemReader g_physmem_reader;
}  // anonymous namespace

namespace acpi_lite {

zx::result<const void *> ZirconPhysmemReader::PhysToPtr(uintptr_t phys, size_t length) {
  // We don't support the 0 physical address or 0-length ranges.
  if (length == 0 || phys == 0) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  // Get the last byte of the specified range, ensuring we don't wrap around the address
  // space.
  if (phys > ktl::numeric_limits<uintptr_t>::max() - length) {
    return zx::error(ZX_ERR_OUT_OF_RANGE);
  }

  // Convert to a page aligned base and size.
  const paddr_t paddr_base = RoundDownPageSize(phys);
  const size_t size = RoundUpPageSize(phys + length - 1) - paddr_base;

  Guard<Mutex> guard{&lock_};

  // Search existing mappings.
  ArchVmAspace &arch_aspace = VmAspace::kernel_aspace()->arch_aspace();
  for (auto &mapping : mappings_) {
    paddr_t map_paddr = 0;
    [[maybe_unused]] arch_mmu_flags_t mmu_flags = 0;
    zx_status_t status = arch_aspace.Query(mapping.mapping->base(), &map_paddr, &mmu_flags);
    if (status != ZX_OK) {
      return zx::error{status};
    }

    DEBUG_ASSERT((mmu_flags & ARCH_MMU_FLAG_PERM_READ) != 0);

    if (InRange(paddr_base, size, map_paddr, map_paddr + mapping.mapping->size())) {
      uintptr_t offset = phys - map_paddr;
      return zx::ok(reinterpret_cast<const void *>(mapping.mapping->base() + offset));
    }
  }

  // Need to create a new mapping to cover this range.
  fbl::AllocChecker ac;
  ktl::unique_ptr<Mapping> pl = ktl::unique_ptr<Mapping>(new (&ac) Mapping());
  if (!ac.check()) {
    return zx::error{ZX_ERR_NO_MEMORY};
  }

  fbl::RefPtr<VmObjectPhysical> vmo;
  zx_status_t status = VmObjectPhysical::Create(paddr_base, size, &vmo);
  if (status != ZX_OK) {
    return zx::error{status};
  }

  zx::result<VmAddressRegion::MapResult> map_result =
      VmAspace::kernel_aspace()->RootVmar()->CreateVmMapping(
          0, size, 0, VMAR_FLAG_CAN_MAP_READ, ktl::move(vmo), 0, ARCH_MMU_FLAG_PERM_READ, "acpi");
  if (map_result.is_error()) {
    return map_result.take_error();
  }

  status = map_result->mapping->MapRange(0, size, true, false);
  if (status != ZX_OK) {
    map_result->mapping->Destroy();
    return zx::error(status);
  }

  pl->mapping = map_result->mapping;
  mappings_.push_front(ktl::move(pl));

  uintptr_t offset = phys - paddr_base;
  return zx::ok(reinterpret_cast<const void *>(map_result->base + offset));
}

// Create a new AcpiParser, starting at the given Root System Description Pointer (RSDP).
zx::result<AcpiParser> AcpiParserInit(zx_paddr_t rsdp_pa) {
  return AcpiParser::Init(g_physmem_reader, rsdp_pa);
}

}  // namespace acpi_lite
