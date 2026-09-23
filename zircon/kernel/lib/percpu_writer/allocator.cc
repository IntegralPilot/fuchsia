// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/percpu_writer/kernel_aspace_allocator.h>

#include <fbl/alloc_checker.h>
#include <vm/vm_address_region.h>
#include <vm/vm_aspace.h>
#include <vm/vm_object_paged.h>

namespace percpu_writer {

ktl::byte* KernelAspaceAllocator::Allocate(uint32_t size, const char* buffer_name) {
  uintptr_t rounded_size = RoundUpPageSize(size);
  if (size == 0) {
    return nullptr;
  }
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status =
      VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY | PMM_ALLOC_FLAG_CAN_WAIT, 0, rounded_size, &vmo);
  if (status != ZX_OK) {
    return nullptr;
  }
  status = vmo->CommitRangePinned(0, rounded_size, true);
  if (status != ZX_OK) {
    return nullptr;
  }
  VmAspace* kaspace = VmAspace::kernel_aspace();

  zx::result<VmAddressRegion::MapResult> r = kaspace->RootVmar()->CreateVmMapping(
      0, rounded_size, 0, 0, vmo, 0, ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE,
      buffer_name);
  if (r.is_error()) {
    vmo->Unpin(0, rounded_size);
    return nullptr;
  }
  status = r->mapping->MapRange(0, rounded_size, true, true);
  if (status != ZX_OK) {
    r->mapping->Destroy();
    vmo->Unpin(0, rounded_size);
    return nullptr;
  }
  return reinterpret_cast<ktl::byte*>(r->base);
}

void KernelAspaceAllocator::Free(ktl::byte* ptr) {
  if (ptr != nullptr) {
    VmAspace* kaspace = VmAspace::kernel_aspace();
    kaspace->FreeRegion(reinterpret_cast<vaddr_t>(ptr));
  }
}
}  // namespace percpu_writer
