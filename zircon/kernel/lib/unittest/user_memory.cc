// Copyright 2018 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/fit/defer.h>
#include <lib/page/size.h>
#include <lib/unittest/unittest.h>
#include <lib/unittest/user_memory.h>

namespace testing {

UserMemory::~UserMemory() {
  zx_status_t status = mapping_->Destroy();
  DEBUG_ASSERT(status == ZX_OK);
}

// static
ktl::unique_ptr<UserMemory> UserMemory::CreateInVmar(fbl::RefPtr<VmObject> vmo,
                                                     fbl::RefPtr<VmAddressRegion>& vmar,
                                                     uint8_t tag, uint8_t align_pow2) {
  size_t size = vmo->size();

  DEBUG_ASSERT(vmar);
  DEBUG_ASSERT(vmar->aspace()->is_user());

  constexpr uint32_t vmar_flags =
      VMAR_FLAG_CAN_MAP_READ | VMAR_FLAG_CAN_MAP_WRITE | VMAR_FLAG_CAN_MAP_EXECUTE;
  constexpr arch_mmu_flags_t arch_mmu_flags =
      ARCH_MMU_FLAG_PERM_USER | ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE;
  auto mapping_result = vmar->CreateVmMapping(/* offset= */ 0, size, align_pow2, vmar_flags, vmo, 0,
                                              arch_mmu_flags, "unittest");
  if (mapping_result.is_error()) {
    unittest_printf("CreateVmMapping failed: %d\n", mapping_result.status_value());
    return nullptr;
  }
  auto unmap = fit::defer([&]() {
    if (mapping_result.is_ok()) {
      zx_status_t status = mapping_result->mapping->Destroy();
      DEBUG_ASSERT(status == ZX_OK);
    }
  });

  fbl::AllocChecker ac;
  ktl::unique_ptr<UserMemory> mem(new (&ac) UserMemory(mapping_result->mapping, vmo, tag));
  if (!ac.check()) {
    unittest_printf("failed to allocate from heap\n");
    return nullptr;
  }
  // Unmapping is now UserMemory's responsibility.
  unmap.cancel();

  return mem;
}

// static
ktl::unique_ptr<UserMemory> UserMemory::CreateInAspace(fbl::RefPtr<VmObject> vmo,
                                                       fbl::RefPtr<VmAspace>& aspace, uint8_t tag,
                                                       uint8_t align_pow2) {
  DEBUG_ASSERT(aspace);
  fbl::RefPtr<VmAddressRegion> root_vmar = aspace->RootVmar();
  DEBUG_ASSERT(root_vmar);
  return CreateInVmar(vmo, root_vmar, tag, align_pow2);
}

// static
ktl::unique_ptr<UserMemory> UserMemory::Create(fbl::RefPtr<VmObject> vmo, uint8_t tag,
                                               uint8_t align_pow2) {
  // active_aspace should always return the normal aspace as this is only run in the unittests,
  // which do not run threads in restricted mode. We assert this to be true by checking that the
  // restricted state is not set on this thread.
  DEBUG_ASSERT(!Thread::Current::restricted_state());
  fbl::RefPtr<VmAspace> aspace(Thread::Current::active_aspace());
  DEBUG_ASSERT(aspace);

  return CreateInAspace(ktl::move(vmo), aspace, tag, align_pow2);
}

// static
ktl::unique_ptr<UserMemory> UserMemory::Create(size_t size) {
  size = RoundUpPageSize(size);

  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0u, size, &vmo);
  if (status != ZX_OK) {
    unittest_printf("VmObjectPaged::Create failed: %d\n", status);
    return nullptr;
  }
  return Create(ktl::move(vmo));
}

}  // namespace testing
