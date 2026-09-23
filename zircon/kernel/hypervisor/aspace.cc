// Copyright 2017 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <align.h>
#include <lib/page/size.h>

#include <fbl/alloc_checker.h>
#include <hypervisor/aspace.h>
#include <kernel/range_check.h>
#include <ktl/utility.h>
#include <vm/fault.h>
#include <vm/page_source.h>
#include <vm/physmap.h>
#include <vm/vm_object_physical.h>

#include <ktl/enforce.h>

namespace {

constexpr arch_mmu_flags_t kInterruptMmuFlags = ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE;
constexpr arch_mmu_flags_t kGuestMmuFlags =
    ARCH_MMU_FLAG_CACHED | ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE;

}  // namespace

namespace hypervisor {

zx::result<GuestPhysicalAspace> GuestPhysicalAspace::Create() {
  auto physical_aspace = VmAspace::Create(VmAspace::Type::GuestPhysical, "guest_physical");
  if (!physical_aspace) {
    return zx::error(ZX_ERR_NO_MEMORY);
  }
  GuestPhysicalAspace gpa;
  gpa.physical_aspace_ = ktl::move(physical_aspace);
  return zx::ok(ktl::move(gpa));
}

GuestPhysicalAspace::~GuestPhysicalAspace() {
  if (physical_aspace_ != nullptr) {
    // VmAspace maintains a circular reference with it's root VMAR. We need to
    // destroy the VmAspace in order to break that reference and allow the
    // VmAspace to be destructed.
    physical_aspace_->Destroy();
  }
}

bool GuestPhysicalAspace::IsMapped(zx_gpaddr_t guest_paddr) const {
  Guard<CriticalMutex> guard(physical_aspace_->lock());
  return FindMapping(guest_paddr) != nullptr;
}

zx::result<> GuestPhysicalAspace::MapInterruptController(zx_gpaddr_t guest_paddr,
                                                         zx_paddr_t host_paddr, size_t len) {
  fbl::RefPtr<VmObjectPhysical> vmo;
  zx_status_t status = VmObjectPhysical::Create(host_paddr, len, &vmo);
  if (status != ZX_OK) {
    return zx::error(status);
  }

  status = vmo->SetMappingCachePolicy(ARCH_MMU_FLAG_UNCACHED_DEVICE);
  if (status != ZX_OK) {
    return zx::error(status);
  }

  // The root VMAR will maintain a reference to the VmMapping internally so
  // we don't need to maintain a long-lived reference to the mapping here.
  zx::result<VmAddressRegion::MapResult> mapping_result = RootVmar()->CreateVmMapping(
      guest_paddr, vmo->size(), /* align_pow2*/ 0, VMAR_FLAG_SPECIFIC, vmo, /* vmo_offset */ 0,
      kInterruptMmuFlags, "guest_interrupt_vmo");
  if (mapping_result.is_error()) {
    return mapping_result.take_error();
  }

  // Write mapping to page table.
  status = mapping_result->mapping->MapRange(0, vmo->size(), true);
  if (status != ZX_OK) {
    mapping_result->mapping->Destroy();
    return zx::error(status);
  }

  return zx::ok();
}

zx::result<> GuestPhysicalAspace::UnmapRange(zx_gpaddr_t guest_paddr, size_t len) {
  // Walk from the root down through any child VMARs for this range so we can do an unmap.
  fbl::RefPtr<VmAddressRegion> vmar = RootVmar();
  while (true) {
    fbl::RefPtr<VmAddressRegionOrMapping> next = vmar->FindRegion(guest_paddr);
    if (!next || next->is_mapping()) {
      break;
    }
    vmar = next->as_vm_address_region();
  }
  zx_status_t status = vmar->Unmap(guest_paddr, len, VmAddressRegionOpChildren::No);
  return zx::make_result(status);
}

zx::result<> GuestPhysicalAspace::PageFault(zx_gpaddr_t guest_paddr) {
  __UNINITIALIZED MultiPageRequest page_request;

  guest_paddr = RoundDownPageSize(guest_paddr);

  zx_status_t status;
  do {
    {
      Guard<CriticalMutex> guard(physical_aspace_->lock());
      fbl::RefPtr<VmMapping> mapping = FindMapping(guest_paddr);
      if (!mapping) {
        return zx::error(ZX_ERR_NOT_FOUND);
      }

      // In order to avoid re-faulting if the guest changes how it accesses
      // guest physical memory, and to avoid the need for invalidation of the
      // guest physical address space on x86 (through the use of INVEPT), we
      // fault the page with the maximum allowable permissions of the mapping.
      AssertHeld(mapping->lock_ref());
      const arch_mmu_flags_t mmu_flags = mapping->arch_mmu_flags_locked(guest_paddr);
      uint pf_flags = VMM_PF_FLAG_GUEST | VMM_PF_FLAG_HW_FAULT;
      if (mmu_flags & ARCH_MMU_FLAG_PERM_WRITE) {
        pf_flags |= VMM_PF_FLAG_WRITE;
      }
      if (mmu_flags & ARCH_MMU_FLAG_PERM_EXECUTE) {
        pf_flags |= VMM_PF_FLAG_INSTRUCTION;
      }
      auto [fault_status, _] = mapping->PageFaultLocked(guest_paddr, pf_flags, 0, &page_request);
      status = fault_status;
    }

    if (status == ZX_ERR_SHOULD_WAIT) {
      zx_status_t st = page_request.Wait();
      if (st != ZX_OK) {
        return zx::error(st);
      }
    }
  } while (status == ZX_ERR_SHOULD_WAIT);

  return zx::make_result(status);
}

zx::result<GuestPtr> GuestPhysicalAspace::CreateGuestPtr(zx_gpaddr_t guest_paddr, size_t len,
                                                         const char* name) {
  const zx_gpaddr_t begin = ROUNDDOWN(guest_paddr, kPageSize);
  const zx_gpaddr_t end = ROUNDUP(guest_paddr + len, kPageSize);
  const zx_gpaddr_t mapping_len = end - begin;
  if (begin > end || !InRange(begin, mapping_len, size())) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  uint64_t intra_mapping_offset;
  uint64_t mapping_object_offset;
  fbl::RefPtr<VmObject> vmo;
  {
    Guard<CriticalMutex> guard(physical_aspace_->lock());
    fbl::RefPtr<VmMapping> guest_mapping = FindMapping(begin);
    if (!guest_mapping) {
      return zx::error(ZX_ERR_NOT_FOUND);
    }
    AssertHeld(guest_mapping->lock_ref());
    intra_mapping_offset = begin - guest_mapping->base();
    if (!InRange(intra_mapping_offset, mapping_len, guest_mapping->size())) {
      // The address range is not contained within a single mapping.
      return zx::error(ZX_ERR_OUT_OF_RANGE);
    }
    mapping_object_offset = guest_mapping->object_offset();
    vmo = guest_mapping->vmo_locked();
  }

  // Pin the range of the guest VMO to ensure the user cannot manipulate it to cause our kernel
  // mapping to become invalidate and generate faults.
  PinnedVmObject pinned_vmo;
  zx_status_t status = PinnedVmObject::Create(vmo, mapping_object_offset + intra_mapping_offset,
                                              mapping_len, true, &pinned_vmo);
  if (status != ZX_OK) {
    return zx::error(status);
  }

  zx::result<VmAddressRegion::MapResult> host_mapping_result =
      VmAspace::kernel_aspace()->RootVmar()->CreateVmMapping(
          /* mapping_offset */ 0, mapping_len,
          /* align_pow2 */ false,
          /* vmar_flags */ 0, vmo, mapping_object_offset + intra_mapping_offset, kGuestMmuFlags,
          name);
  if (host_mapping_result.is_error()) {
    return host_mapping_result.take_error();
  }
  // Pre-populate the page tables so there's no need for kernel page faults.
  status = host_mapping_result->mapping->MapRange(0, mapping_len, true);
  if (status != ZX_OK) {
    return zx::error(status);
  }

  return zx::ok(GuestPtr(ktl::move(host_mapping_result->mapping), ktl::move(pinned_vmo),
                         guest_paddr - begin));
}

fbl::RefPtr<VmMapping> GuestPhysicalAspace::FindMapping(zx_gpaddr_t guest_paddr) const {
  fbl::RefPtr<VmAddressRegion> region = physical_aspace_->RootVmarLocked();
  AssertHeld(region->lock_ref());
  for (fbl::RefPtr<VmAddressRegionOrMapping> next; (next = region->FindRegionLocked(guest_paddr));
       region = next->as_vm_address_region()) {
    if (next->is_mapping()) {
      return next->as_vm_mapping();
    }
  }
  return nullptr;
}

}  // namespace hypervisor
