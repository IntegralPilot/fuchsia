// Copyright 2017 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/page/size.h>
#include <lib/unittest/unittest.h>
#include <zircon/errors.h>
#include <zircon/syscalls/hypervisor.h>
#include <zircon/types.h>

#include <hypervisor/aspace.h>
#include <hypervisor/interrupt_tracker.h>
#include <hypervisor/trap_map.h>
#include <vm/pmm.h>
#include <vm/scanner.h>
#include <vm/vm.h>
#include <vm/vm_address_region.h>
#include <vm/vm_aspace.h>
#include <vm/vm_object.h>
#include <vm/vm_object_paged.h>

static constexpr arch_mmu_flags_t kMmuFlags =
    ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE | ARCH_MMU_FLAG_PERM_EXECUTE;

static bool hypervisor_supported() {
#ifdef __x86_64__
  return true;
#endif
  return false;
}

static zx::result<hypervisor::GuestPhysicalAspace> create_gpas() {
  auto gpa = hypervisor::GuestPhysicalAspace::Create();
#if ARCH_ARM64
  if (gpa.is_ok()) {
    gpa->arch_aspace().arch_set_asid(1);
  }
#endif
  return gpa;
}

static zx_status_t create_vmo(size_t vmo_size, fbl::RefPtr<VmObjectPaged>* vmo) {
  return VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0u, vmo_size, vmo);
}

static zx_status_t commit_vmo(fbl::RefPtr<VmObjectPaged> vmo) {
  return vmo->CommitRange(0, vmo->size());
}

static zx_status_t create_mapping(fbl::RefPtr<VmAddressRegion> vmar, fbl::RefPtr<VmObjectPaged> vmo,
                                  zx_gpaddr_t addr, arch_mmu_flags_t mmu_flags = kMmuFlags) {
  return vmar
      ->CreateVmMapping(addr, vmo->size(), 0 /* align_pow2 */, VMAR_FLAG_SPECIFIC, vmo,
                        0 /* vmo_offset */, mmu_flags, "vmo")
      .status_value();
}

static zx_status_t create_sub_vmar(fbl::RefPtr<VmAddressRegion> vmar, size_t offset, size_t size,
                                   fbl::RefPtr<VmAddressRegion>* sub_vmar) {
  return vmar->CreateSubVmar(offset, size, 0 /* align_pow2 */, vmar->flags() | VMAR_FLAG_SPECIFIC,
                             "vmar", sub_vmar);
}

static bool guest_physical_aspace_unmap_range() {
  BEGIN_TEST;

  if (!hypervisor_supported()) {
    return true;
  }

  // Setup.
  auto gpa = create_gpas();
  EXPECT_EQ(ZX_OK, gpa.status_value(), "Failed to create GuestPhysicalAspace\n");
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = create_vmo(kPageSize, &vmo);
  EXPECT_EQ(ZX_OK, status, "Failed to create VMO\n");
  status = create_mapping(gpa->RootVmar(), vmo, 0);
  EXPECT_EQ(ZX_OK, status, "Failed to create mapping\n");

  // Unmap page.
  auto result = gpa->UnmapRange(0, kPageSize);
  EXPECT_EQ(ZX_OK, result.status_value(), "Failed to unmap page from GuestPhysicalAspace\n");

  // Verify IsMapped for unmapped address fails.
  EXPECT_FALSE(gpa->IsMapped(0), "Expected address to be unmapped\n");

  END_TEST;
}

static bool guest_physical_aspace_unmap_range_outside_of_mapping() {
  BEGIN_TEST;

  if (!hypervisor_supported()) {
    return true;
  }

  // Setup.
  auto gpa = create_gpas();
  EXPECT_EQ(ZX_OK, gpa.status_value(), "Failed to create GuestPhysicalAspace\n");
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = create_vmo(kPageSize, &vmo);
  EXPECT_EQ(ZX_OK, status, "Failed to create VMO\n");
  status = create_mapping(gpa->RootVmar(), vmo, 0);
  EXPECT_EQ(ZX_OK, status, "Failed to create mapping\n");

  // Unmap page.
  auto result = gpa->UnmapRange(kPageSize * 8, kPageSize);
  EXPECT_EQ(ZX_OK, result.status_value(), "Failed to unmap page from GuestPhysicalAspace\n");

  END_TEST;
}

static bool guest_physical_aspace_unmap_range_multiple_mappings() {
  BEGIN_TEST;

  if (!hypervisor_supported()) {
    return true;
  }

  // Setup.
  auto gpa = create_gpas();
  EXPECT_EQ(ZX_OK, gpa.status_value(), "Failed to create GuestPhysicalAspace\n");

  fbl::RefPtr<VmObjectPaged> vmo1;
  zx_status_t status = create_vmo(kPageSize * 2, &vmo1);
  EXPECT_EQ(ZX_OK, status, "Failed to create VMO\n");
  status = create_mapping(gpa->RootVmar(), vmo1, 0);
  EXPECT_EQ(ZX_OK, status, "Failed to create mapping\n");

  fbl::RefPtr<VmObjectPaged> vmo2;
  status = create_vmo(kPageSize * 2, &vmo2);
  EXPECT_EQ(ZX_OK, status, "Failed to create VMO\n");
  status = create_mapping(gpa->RootVmar(), vmo2, kPageSize * 3);
  EXPECT_EQ(ZX_OK, status, "Failed to create mapping\n");

  // Unmap pages.
  auto result = gpa->UnmapRange(kPageSize, kPageSize * 3);
  EXPECT_EQ(ZX_OK, result.status_value(),
            "Failed to multiple unmap pages from GuestPhysicalAspace\n");

  // Verify IsMapped for unmapped addresses fails.
  for (zx_gpaddr_t addr = kPageSize; addr < kPageSize * 4; addr += kPageSize) {
    EXPECT_FALSE(gpa->IsMapped(addr), "Expected address to be unmapped\n");
  }

  // Verify IsMapped for mapped addresses succeeds.
  EXPECT_TRUE(gpa->IsMapped(0), "Expected address to be mapped\n");
  EXPECT_TRUE(gpa->IsMapped(kPageSize * 4), "Expected address to be mapped\n");

  END_TEST;
}

static bool guest_physical_aspace_unmap_range_sub_region() {
  BEGIN_TEST;

  if (!hypervisor_supported()) {
    return true;
  }

  // Setup.
  auto gpa = create_gpas();
  EXPECT_EQ(ZX_OK, gpa.status_value(), "Failed to create GuestPhysicalAspace\n");
  fbl::RefPtr<VmAddressRegion> root_vmar = gpa->RootVmar();
  // To test partial unmapping within sub-VMAR:
  // Sub-VMAR from [0, kPageSize * 2).
  // Map within sub-VMAR from [kPageSize, kPageSize * 2).
  fbl::RefPtr<VmAddressRegion> sub_vmar1;
  zx_status_t status = create_sub_vmar(root_vmar, 0, kPageSize * 2, &sub_vmar1);
  EXPECT_EQ(ZX_OK, status, "Failed to create sub-VMAR\n");
  EXPECT_TRUE(sub_vmar1->has_parent(), "Sub-VMAR does not have a parent");
  fbl::RefPtr<VmObjectPaged> vmo1;
  status = create_vmo(kPageSize, &vmo1);
  EXPECT_EQ(ZX_OK, status, "Failed to create VMO\n");
  status = create_mapping(sub_vmar1, vmo1, kPageSize);
  EXPECT_EQ(ZX_OK, status, "Failed to create mapping\n");
  // To test destroying of sub-VMAR:
  // Sub-VMAR from [kPageSize * 2, kPageSize * 3).
  // Map within sub-VMAR from [0, kPageSize).
  fbl::RefPtr<VmAddressRegion> sub_vmar2;
  status = create_sub_vmar(root_vmar, kPageSize * 2, kPageSize, &sub_vmar2);
  EXPECT_EQ(ZX_OK, status, "Failed to create sub-VMAR\n");
  EXPECT_TRUE(sub_vmar2->has_parent(), "Sub-VMAR does not have a parent");
  fbl::RefPtr<VmObjectPaged> vmo2;
  status = create_vmo(kPageSize, &vmo2);
  EXPECT_EQ(ZX_OK, status, "Failed to create VMO\n");
  status = create_mapping(sub_vmar2, vmo2, 0);
  EXPECT_EQ(ZX_OK, status, "Failed to create mapping\n");
  // To test partial unmapping within root-VMAR:
  // Map within root-VMAR from [kPageSize * 3, kPageSize * 5).
  fbl::RefPtr<VmObjectPaged> vmo3;
  status = create_vmo(kPageSize * 2, &vmo3);
  EXPECT_EQ(ZX_OK, status, "Failed to create VMO\n");
  status = create_mapping(root_vmar, vmo3, kPageSize * 3);
  EXPECT_EQ(ZX_OK, status, "Failed to create mapping\n");

  // Cannot unmap spanning across two different sub-vmars.
  auto result = gpa->UnmapRange(kPageSize, kPageSize * 3);
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, result.status_value());

  // Verify IsMapped for mapped addresses succeeds.
  EXPECT_TRUE(gpa->IsMapped(kPageSize * 4), "Expected address to be mapped\n");

  // Verify that sub-VMARs still have a parent.
  EXPECT_TRUE(sub_vmar1->has_parent(), "Sub-VMAR does not have a parent");
  EXPECT_TRUE(sub_vmar2->has_parent(), "Sub-VMAR does not have a parent");

  END_TEST;
}

static bool guest_phyiscal_address_space_single_vmo_multiple_mappings() {
  BEGIN_TEST;

  if (!hypervisor_supported()) {
    return true;
  }

  AutoVmScannerDisable scanner_disable;

  // Setup.
  auto gpa = create_gpas();
  EXPECT_EQ(ZX_OK, gpa.status_value(), "Failed to create GuestPhysicalAspace\n");

  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = create_vmo(kPageSize * 4, &vmo);
  EXPECT_EQ(ZX_OK, status, "Failed to create VMO\n");

  // Map a single page of this four page VMO at offset 0x1000 and offset 0x3000.
  auto mapping_result =
      gpa->RootVmar()->CreateVmMapping(kPageSize, kPageSize, 0 /* align_pow2 */, VMAR_FLAG_SPECIFIC,
                                       vmo, kPageSize, kMmuFlags, "vmo");
  EXPECT_EQ(ZX_OK, mapping_result.status_value(), "Failed to create first mapping\n");
  mapping_result =
      gpa->RootVmar()->CreateVmMapping(kPageSize * 3, kPageSize, 0 /* align_pow2 */,
                                       VMAR_FLAG_SPECIFIC, vmo, kPageSize * 3, kMmuFlags, "vmo");
  EXPECT_EQ(ZX_OK, mapping_result.status_value(), "Failed to create second mapping\n");

  status = commit_vmo(vmo);
  EXPECT_EQ(ZX_OK, status, "Failed to commit VMO\n");

  // No mapping at 0x0 or 0x2000.
  EXPECT_FALSE(gpa->IsMapped(0), "Expected address to be unmapped\n");
  EXPECT_FALSE(gpa->IsMapped(kPageSize * 2), "Expected address to be unmapped\n");

  // There is a mapping at 0x1000 and 0x3000.
  EXPECT_TRUE(gpa->IsMapped(kPageSize), "Expected address to be mapped\n");
  EXPECT_TRUE(gpa->IsMapped(kPageSize * 3), "Expected address to be mapped\n");

  END_TEST;
}

static bool guest_physical_aspace_page_fault() {
  BEGIN_TEST;

  if (!hypervisor_supported()) {
    return true;
  }

  // Setup.
  auto gpa = create_gpas();
  EXPECT_EQ(ZX_OK, gpa.status_value(), "Failed to create GuestPhysicalAspace\n");
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = create_vmo(kPageSize, &vmo);
  EXPECT_EQ(ZX_OK, status, "Failed to create VMO\n");
  status = create_mapping(gpa->RootVmar(), vmo, 0);
  EXPECT_EQ(ZX_OK, status, "Failed to create mapping\n");
  status = create_mapping(gpa->RootVmar(), vmo, kPageSize, ARCH_MMU_FLAG_PERM_READ);
  EXPECT_EQ(ZX_OK, status, "Failed to create mapping\n");
  status = create_mapping(gpa->RootVmar(), vmo, kPageSize * 2,
                          ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE);
  EXPECT_EQ(ZX_OK, status, "Failed to create mapping\n");
  status = create_mapping(gpa->RootVmar(), vmo, kPageSize * 3,
                          ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_EXECUTE);
  EXPECT_EQ(ZX_OK, status, "Failed to create mapping\n");

  // Fault in each page.
  for (zx_gpaddr_t addr = 0; addr < kPageSize * 4; addr += kPageSize) {
    auto result = gpa->PageFault(addr);
    EXPECT_EQ(ZX_OK, result.status_value(), "Failed to fault page\n");
  }

  END_TEST;
}

static bool guest_physical_aspace_map_interrupt_controller() {
  BEGIN_TEST;

  if (!hypervisor_supported()) {
    return true;
  }

  // Setup.
  auto gpa = create_gpas();
  EXPECT_EQ(ZX_OK, gpa.status_value(), "Failed to create GuestPhysicalAspace\n");
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = create_vmo(kPageSize, &vmo);
  EXPECT_EQ(ZX_OK, status, "Failed to create VMO\n");
  status = create_mapping(gpa->RootVmar(), vmo, 0);
  EXPECT_EQ(ZX_OK, status, "Failed to create mapping\n");

  // Allocate a page to use as the interrupt controller.
  paddr_t paddr = 0;
  vm_page* vm_page;
  status = pmm_alloc_page(0, &vm_page, &paddr);
  EXPECT_EQ(ZX_OK, status, "Unable to allocate a page\n");

  // Map interrupt controller page in an arbitrary location.
  const vaddr_t kGicvAddress = 0x800001000;
  auto result = gpa->MapInterruptController(kGicvAddress, paddr, kPageSize);
  EXPECT_EQ(ZX_OK, result.status_value(), "Failed to map APIC page\n");

  // Cleanup
  pmm_free_page(vm_page);
  END_TEST;
}

static bool guest_physical_aspace_uncached() {
  BEGIN_TEST;

  if (!hypervisor_supported()) {
    return true;
  }

  // Setup.
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = create_vmo(kPageSize, &vmo);
  EXPECT_EQ(ZX_OK, status, "Failed to create VMO\n");
  status = vmo->SetMappingCachePolicy(ZX_CACHE_POLICY_UNCACHED);
  EXPECT_EQ(ZX_OK, status, "Failed to set cache policy\n");

  auto gpa = create_gpas();
  EXPECT_EQ(ZX_OK, gpa.status_value(), "Failed to create GuestPhysicalAspace\n");
  status = create_mapping(gpa->RootVmar(), vmo, 0);
  EXPECT_EQ(ZX_OK, status, "Failed to create mapping\n");

  END_TEST;
}

static bool guest_physical_aspace_uncached_device() {
  BEGIN_TEST;

  if (!hypervisor_supported()) {
    return true;
  }

  // Setup.
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = create_vmo(kPageSize, &vmo);
  EXPECT_EQ(ZX_OK, status, "Failed to create VMO\n");
  status = vmo->SetMappingCachePolicy(ZX_CACHE_POLICY_UNCACHED_DEVICE);
  EXPECT_EQ(ZX_OK, status, "Failed to set cache policy\n");

  auto gpa = create_gpas();
  EXPECT_EQ(ZX_OK, gpa.status_value(), "Failed to create GuestPhysicalAspace\n");
  status = create_mapping(gpa->RootVmar(), vmo, 0);
  EXPECT_EQ(ZX_OK, status, "Failed to create mapping\n");

  END_TEST;
}

static bool guest_physical_aspace_write_combining() {
  BEGIN_TEST;

  if (!hypervisor_supported()) {
    return true;
  }

  // Setup.
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = create_vmo(kPageSize, &vmo);
  EXPECT_EQ(ZX_OK, status, "Failed to create VMO\n");
  status = vmo->SetMappingCachePolicy(ZX_CACHE_POLICY_WRITE_COMBINING);
  EXPECT_EQ(ZX_OK, status, "Failed to set cache policy\n");

  auto gpa = create_gpas();
  EXPECT_EQ(ZX_OK, gpa.status_value(), "Failed to create GuestPhysicalAspace\n");
  status = create_mapping(gpa->RootVmar(), vmo, 0);
  EXPECT_EQ(ZX_OK, status, "Failed to create mapping\n");

  END_TEST;
}

static bool guest_physical_aspace_protect() {
  BEGIN_TEST;

  if (!hypervisor_supported()) {
    return true;
  }

  // Setup.
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = create_vmo(kPageSize, &vmo);
  EXPECT_EQ(ZX_OK, status, "Failed to create VMO\n");

  auto gpa = create_gpas();
  EXPECT_EQ(ZX_OK, gpa.status_value(), "Failed to create GuestPhysicalAspace\n");
  status = create_mapping(gpa->RootVmar(), vmo, 0);
  EXPECT_EQ(ZX_OK, status, "Failed to create mapping\n");

  status = gpa->RootVmar()->Protect(0, kPageSize, ARCH_MMU_FLAG_PERM_WRITE,
                                    VmAddressRegionOpChildren::Yes);
  EXPECT_EQ(ZX_OK, status, "Failed to enable write access\n");

  END_TEST;
}

static bool guest_physical_aspace_query() {
  BEGIN_TEST;

  if (!hypervisor_supported()) {
    return true;
  }

  // Disable the VM scanner to ensure pages stay committed into the VMO, and hence stay mapped into
  // the aspace before we query.
  AutoVmScannerDisable scanner_disable;

  // This test is arch independent so be conservative with the permission and assume that read is
  // needed for any other permission.
  constexpr arch_mmu_flags_t kMmuFlagTests[] = {
      ARCH_MMU_FLAG_PERM_READ, ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE,
      ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_EXECUTE,
      ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE | ARCH_MMU_FLAG_PERM_EXECUTE};

  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = create_vmo(kPageSize, &vmo);
  ASSERT_OK(status);

  auto gpa = create_gpas();
  ASSERT_OK(gpa.status_value());

  for (const arch_mmu_flags_t flags : kMmuFlagTests) {
    auto mapping_result =
        gpa->RootVmar()->CreateVmMapping(0, kPageSize, 0 /* align_pow2 */, VMAR_FLAG_SPECIFIC, vmo,
                                         0 /* vmo_offset */, flags, "vmo");
    EXPECT_OK(mapping_result.status_value());
    status = mapping_result->mapping->MapRange(0, kPageSize, true, false);
    EXPECT_OK(status);

    arch_mmu_flags_t query_flags = 0;
    status = gpa->arch_aspace().Query(0, nullptr, &query_flags);
    EXPECT_OK(status);
    EXPECT_EQ(flags, query_flags);

    // Cleanup the mapping for next iteration.
    status = mapping_result->mapping->Destroy();
    EXPECT_OK(status);
  }

  END_TEST;
}

static bool interrupt_bitmap() {
  BEGIN_TEST;

  hypervisor::InterruptBitmap<8> bitmap;

  uint32_t vector = UINT32_MAX;
  EXPECT_FALSE(bitmap.Get(0));
  EXPECT_FALSE(bitmap.Get(1));
  EXPECT_FALSE(bitmap.Scan(&vector));
  EXPECT_EQ(UINT32_MAX, vector);

  // Index 0.
  vector = UINT32_MAX;
  bitmap.Set(0u);
  EXPECT_TRUE(bitmap.Get(0));
  EXPECT_FALSE(bitmap.Get(1));
  EXPECT_TRUE(bitmap.Scan(&vector));
  EXPECT_EQ(0u, vector);

  vector = UINT32_MAX;
  bitmap.Clear(0u, 1u);
  EXPECT_FALSE(bitmap.Get(0u));
  EXPECT_FALSE(bitmap.Get(1u));
  EXPECT_FALSE(bitmap.Scan(&vector));
  EXPECT_EQ(UINT32_MAX, vector);

  // Index 1.
  vector = UINT32_MAX;
  bitmap.Set(1u);
  EXPECT_FALSE(bitmap.Get(0u));
  EXPECT_TRUE(bitmap.Get(1u));
  EXPECT_TRUE(bitmap.Scan(&vector));
  EXPECT_EQ(1u, vector);

  vector = UINT32_MAX;
  bitmap.Clear(1u, 2u);
  EXPECT_FALSE(bitmap.Get(0u));
  EXPECT_FALSE(bitmap.Get(1u));
  EXPECT_FALSE(bitmap.Scan(&vector));
  EXPECT_EQ(UINT32_MAX, vector);

  // Clear
  bitmap.Set(0u);
  bitmap.Set(1u);
  bitmap.Set(2u);
  bitmap.Set(3u);
  bitmap.Clear(1u, 3u);
  EXPECT_TRUE(bitmap.Get(0u));
  EXPECT_FALSE(bitmap.Get(1u));
  EXPECT_FALSE(bitmap.Get(2u));
  EXPECT_TRUE(bitmap.Get(3u));

  END_TEST;
}

static bool trap_map_insert_trap_intersecting() {
  BEGIN_TEST;

  hypervisor::TrapMap trap_map;
  // Add traps:
  // 1. [10, 19]
  // 2. [20, 29]
  // 3. [35, 5]
  EXPECT_EQ(ZX_OK, trap_map.InsertTrap(ZX_GUEST_TRAP_MEM, 10, 10, nullptr, 0).status_value());
  EXPECT_EQ(ZX_OK, trap_map.InsertTrap(ZX_GUEST_TRAP_MEM, 20, 10, nullptr, 0).status_value());
  EXPECT_EQ(ZX_OK, trap_map.InsertTrap(ZX_GUEST_TRAP_MEM, 35, 5, nullptr, 0).status_value());
  // Trap at [0, 10] intersects with trap 1.
  EXPECT_EQ(ZX_ERR_ALREADY_EXISTS,
            trap_map.InsertTrap(ZX_GUEST_TRAP_MEM, 0, 11, nullptr, 0).status_value());
  // Trap at [10, 19] intersects with trap 1.
  EXPECT_EQ(ZX_ERR_ALREADY_EXISTS,
            trap_map.InsertTrap(ZX_GUEST_TRAP_MEM, 10, 10, nullptr, 0).status_value());
  // Trap at [11, 18] intersects with trap 1.
  EXPECT_EQ(ZX_ERR_ALREADY_EXISTS,
            trap_map.InsertTrap(ZX_GUEST_TRAP_MEM, 11, 8, nullptr, 0).status_value());
  // Trap at [15, 24] intersects with trap 1 and trap 2.
  EXPECT_EQ(ZX_ERR_ALREADY_EXISTS,
            trap_map.InsertTrap(ZX_GUEST_TRAP_MEM, 15, 10, nullptr, 0).status_value());
  // Trap at [30, 39] intersects with trap 3.
  EXPECT_EQ(ZX_ERR_ALREADY_EXISTS,
            trap_map.InsertTrap(ZX_GUEST_TRAP_MEM, 30, 10, nullptr, 0).status_value());
  // Trap at [36, 40] intersects with trap 3.
  EXPECT_EQ(ZX_ERR_ALREADY_EXISTS,
            trap_map.InsertTrap(ZX_GUEST_TRAP_MEM, 36, 5, nullptr, 0).status_value());

  // Add a trap at the beginning.
  EXPECT_EQ(ZX_OK, trap_map.InsertTrap(ZX_GUEST_TRAP_MEM, 0, 10, nullptr, 0).status_value());
  // In the gap.
  EXPECT_EQ(ZX_OK, trap_map.InsertTrap(ZX_GUEST_TRAP_MEM, 30, 5, nullptr, 0).status_value());
  // And at the end.
  EXPECT_EQ(ZX_OK, trap_map.InsertTrap(ZX_GUEST_TRAP_MEM, 40, 10, nullptr, 0).status_value());

  END_TEST;
}

static bool trap_map_insert_trap_out_of_range() {
  BEGIN_TEST;

  hypervisor::TrapMap trap_map;
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE,
            trap_map.InsertTrap(ZX_GUEST_TRAP_MEM, 0, 0, nullptr, 0).status_value());
  EXPECT_EQ(
      ZX_ERR_OUT_OF_RANGE,
      trap_map.InsertTrap(ZX_GUEST_TRAP_MEM, UINT32_MAX, UINT64_MAX, nullptr, 0).status_value());
#ifdef ARCH_X86
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE,
            trap_map.InsertTrap(ZX_GUEST_TRAP_IO, 0, UINT32_MAX, nullptr, 0).status_value());
#endif  // ARCH_X86

  END_TEST;
}

// Use the function name as the test name
#define HYPERVISOR_UNITTEST(fname) UNITTEST(#fname, fname)

UNITTEST_START_TESTCASE(hypervisor)
HYPERVISOR_UNITTEST(guest_physical_aspace_unmap_range)
HYPERVISOR_UNITTEST(guest_physical_aspace_unmap_range_outside_of_mapping)
HYPERVISOR_UNITTEST(guest_physical_aspace_unmap_range_multiple_mappings)
HYPERVISOR_UNITTEST(guest_physical_aspace_unmap_range_sub_region)
HYPERVISOR_UNITTEST(guest_phyiscal_address_space_single_vmo_multiple_mappings)
HYPERVISOR_UNITTEST(guest_physical_aspace_page_fault)
HYPERVISOR_UNITTEST(guest_physical_aspace_map_interrupt_controller)
HYPERVISOR_UNITTEST(guest_physical_aspace_uncached)
HYPERVISOR_UNITTEST(guest_physical_aspace_uncached_device)
HYPERVISOR_UNITTEST(guest_physical_aspace_write_combining)
HYPERVISOR_UNITTEST(guest_physical_aspace_protect)
HYPERVISOR_UNITTEST(guest_physical_aspace_query)
HYPERVISOR_UNITTEST(interrupt_bitmap)
HYPERVISOR_UNITTEST(trap_map_insert_trap_intersecting)
HYPERVISOR_UNITTEST(trap_map_insert_trap_out_of_range)
UNITTEST_END_TESTCASE(hypervisor, "hypervisor", "Hypervisor unit tests.")
