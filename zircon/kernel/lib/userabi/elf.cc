// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "elf.h"

#include <lib/page/size.h>
#include <zircon/assert.h>
#include <zircon/features.h>

#include <arch/arch_ops.h>

zx::result<MappedElf> MapHandoffElf(  //
    HandoffEnd::Elf elf, VmAddressRegionDispatcher& parent_vmar) {
  ktl::array<char, ZX_MAX_NAME_LEN> vmo_name_buffer;
  elf.vmo->get_name(vmo_name_buffer.data(), vmo_name_buffer.size());
  ktl::string_view vmo_name{vmo_name_buffer.data(), vmo_name_buffer.size()};
  vmo_name = vmo_name.substr(0, vmo_name.find_first_of('\0'));

  KernelHandle<VmAddressRegionDispatcher> vmar_handle;
  zx_rights_t vmar_rights;
  zx_vm_option_t vmar_flags =
      ZX_VM_CAN_MAP_SPECIFIC | ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_CAN_MAP_EXECUTE;
  zx_status_t status =
      parent_vmar.Allocate(0, elf.vmar_size, vmar_flags, &vmar_handle, &vmar_rights);
  if (status != ZX_OK) {
    dprintf(CRITICAL, "userboot: failed to allocate VMAR of %zu bytes for %.*s ELF image\n",
            elf.vmar_size, static_cast<int>(vmo_name.size()), vmo_name.data());
    return zx::error{status};
  }
  VmAddressRegionDispatcher& vmar = *vmar_handle.dispatcher();

  MappedElf mapped_elf = {
      .vmar = Handle::Make(ktl::move(vmar_handle), vmar_rights),
      .vaddr_start = vmar.vmar()->base(),
      .vaddr_size = vmar.vmar()->size(),
      .entry = vmar.vmar()->base() + elf.info.relative_entry_point,
      .stack_size = elf.info.stack_size,
  };

  // TODO(mcgrathr): emit symbolizer markup for these instead
  dprintf(SPEW, "userboot: %-31s @ [%#" PRIxPTR ",%#" PRIxPTR ")\n", "inside VMAR",
          parent_vmar.vmar()->base(), parent_vmar.vmar()->base() + parent_vmar.vmar()->size());
  dprintf(SPEW, "userboot: %-31.*s @ [%#" PRIxPTR ",%#" PRIxPTR ")\n",
          static_cast<int>(vmo_name.size()), vmo_name.data(), mapped_elf.vaddr_start,
          mapped_elf.vaddr_start + mapped_elf.vaddr_size);

  // Mappings marked with kZeroFill need anonymous VMO pages rather than file
  // VMO pages.  Sum those and make a single VMO for all the pages needed.
  size_t bss_total = 0;
  for (const PhysMapping& mapping : elf.mappings) {
    if (mapping.paddr == PhysElfImage::kZeroFill) {
      bss_total += mapping.size;
    }
  }
  fbl::RefPtr<VmObjectPaged> bss_vmo;
  if (bss_total > 0) {
    DEBUG_ASSERT(bss_total % kPageSize == 0);
    status =
        VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY | PMM_ALLOC_FLAG_CAN_WAIT, 0, bss_total, &bss_vmo);
    if (status != ZX_OK) {
      dprintf(CRITICAL,
              "userboot: failed to allocate VMO of %zu bss bytes for %.*s ELF image: %d\n",
              bss_total, static_cast<int>(vmo_name.size()), vmo_name.data(), status);
      return zx::error{status};
    }
  }

  const char* segment_name = "???";
  for (const PhysMapping& mapping : elf.mappings) {
    zx_vm_option_t map_flags = ZX_VM_SPECIFIC;
    if (mapping.perms.readable()) {
      map_flags |= ZX_VM_PERM_READ;
      segment_name = "rodata";
    }
    if (mapping.perms.writable()) {
      map_flags |= ZX_VM_PERM_WRITE;
      segment_name = "data";
    }
    if (mapping.perms.executable()) {
      ZX_ASSERT(!mapping.perms.writable());
      map_flags |= ZX_VM_PERM_EXECUTE;
      if (!(arch_vm_features() & ZX_VM_FEATURE_CAN_MAP_XOM)) {
        map_flags |= ZX_VM_PERM_READ;
      }
      segment_name = "code";
    }
    if (mapping.paddr == PhysElfImage::kZeroFill) {
      // Map from bss_vmo instead of elf.vmo; consume its pages from the end.
      bss_total -= mapping.size;
      segment_name = "bss";
    }
    zx::result<VmAddressRegion::MapResult> map_result =
        mapping.paddr == PhysElfImage::kZeroFill
            ? vmar.Map(mapping.vaddr, bss_vmo, bss_total, mapping.size, map_flags)
            : vmar.Map(mapping.vaddr, elf.vmo, mapping.paddr, mapping.size, map_flags);
    if (map_result.is_error()) {
      dprintf(CRITICAL, "userboot: %.*s ELF %s mapping %#zx @ %#" PRIxPTR " size %#zx failed %d\n",
              static_cast<int>(vmo_name.size()), vmo_name.data(), segment_name, mapping.paddr,
              vmar.vmar()->base() + mapping.vaddr, mapping.size, map_result.status_value());
      return map_result.take_error();
    }
    DEBUG_ASSERT(map_result->base == vmar.vmar()->base() + mapping.vaddr);
    dprintf(SPEW, "userboot: %-12.*s ELF %-6s %#7zx @ [%#" PRIxPTR ",%#" PRIxPTR ")\n",
            static_cast<int>(vmo_name.size()), vmo_name.data(), segment_name, mapping.paddr,
            map_result->base, map_result->base + mapping.size);
  }
  DEBUG_ASSERT(bss_total == 0);

  return zx::ok(ktl::move(mapped_elf));
}
