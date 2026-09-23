// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fake-resource/resource.h>
#include <lib/pci/pciroot.h>
#include <lib/pci/root_host.h>
#include <lib/zx/eventpair.h>
#include <lib/zx/resource.h>
#include <lib/zx/result.h>
#include <zircon/limits.h>
#include <zircon/syscalls.h>

#include <limits>
#include <memory>

#include <gtest/gtest.h>
#include <region-alloc/region-alloc.h>

#include "src/lib/testing/predicates/status.h"

class PciRootHostTests : public testing::Test {
 protected:
  void SetUp() final {
    ASSERT_OK(
        fake_resource_create(ZX_RSRC_KIND_SYSTEM, fake_msi_resource_.reset_and_get_address()));
    ASSERT_OK(fake_resource_create(ZX_RSRC_KIND_MMIO, fake_mmio_resource_.reset_and_get_address()));
    ASSERT_OK(
        fake_resource_create(ZX_RSRC_KIND_IOPORT, fake_ioport_resource_.reset_and_get_address()));
    page_size_ = zx_system_get_page_size();
    root_host_ =
        std::make_unique<PciRootHost>(fake_msi_resource_.borrow(), fake_mmio_resource_.borrow(),
                                      fake_ioport_resource_.borrow(), PCI_ADDRESS_SPACE_IO);
  }

  PciRootHost& root_host() { return *root_host_; }
  size_t page_size() const { return page_size_; }

 private:
  size_t page_size_;
  zx::resource fake_msi_resource_;
  zx::resource fake_mmio_resource_;
  zx::resource fake_ioport_resource_;
  std::unique_ptr<PciRootHost> root_host_;
};

// The allocators backing the RootHost have their own tests inside the source
// directory of region-alloc, so there's no need to implement region rango
// tests in this suite. The resource reaping is the important detail to test
TEST_F(PciRootHostTests, ResourceAllocationLifecycle) {
  const zx_paddr_t kRangeStart = 0x1000;
  const size_t kRangeSize = 0x4000;
  ASSERT_OK(root_host().Mmio64().AddRegion({0, 0x100000000}));
  {
    zx::resource res1, res2, res3;
    zx::eventpair endpoint1, endpoint2, endpoint3;
    // Allocate at a given position.
    ASSERT_OK(root_host().AllocateMmio64Window(kRangeStart, kRangeSize, &res1, &endpoint1));
    // That position should not work.
    ASSERT_EQ(ZX_ERR_NOT_FOUND,
              root_host()
                  .AllocateMmio64Window(kRangeStart, kRangeSize, &res2, &endpoint2)
                  .status_value());
    // But an allocation of the same size with no base should.
    ASSERT_OK(root_host().AllocateMmio64Window(0, kRangeSize, &res3, &endpoint3));
  }
  // The allocate regions should be cleared out, so the specific range is free again.
  zx::resource res;
  zx::eventpair endpoint;
  ASSERT_OK(root_host().AllocateMmio64Window(kRangeStart, kRangeSize, &res, &endpoint));
}

TEST_F(PciRootHostTests, Mcfg) {
  const McfgAllocation in_mcfg = {
      .address = 0x100000000,
      .pci_segment = 1,
      .start_bus_number = 0,
      .end_bus_number = 64,
  };
  McfgAllocation out_mcfg{};
  ASSERT_EQ(ZX_ERR_NOT_FOUND, root_host().GetSegmentMcfgAllocation(in_mcfg.pci_segment, &out_mcfg));
  root_host().mcfgs().push_back(in_mcfg);
  ASSERT_EQ(ZX_OK, root_host().GetSegmentMcfgAllocation(in_mcfg.pci_segment, &out_mcfg));
  ASSERT_EQ(in_mcfg.address, out_mcfg.address);
  ASSERT_EQ(in_mcfg.pci_segment, out_mcfg.pci_segment);
  ASSERT_EQ(in_mcfg.start_bus_number, out_mcfg.start_bus_number);
  ASSERT_EQ(in_mcfg.end_bus_number, out_mcfg.end_bus_number);
}

TEST_F(PciRootHostTests, MsiAllocationTest) {
  const uint32_t irq_cnt = 8;
  zx::msi msi = {};
  msi_allocation_info_t info = {};
  ASSERT_OK(root_host().AllocateMsi(irq_cnt, &msi, &info));
  ASSERT_EQ(info.irq_count, irq_cnt);
}

constexpr size_t kU32Max = std::numeric_limits<uint32_t>::max();
constexpr size_t kU64Max = std::numeric_limits<uint64_t>::max();
TEST_F(PciRootHostTests, MmioHelperTests) {
  struct {
    ralloc_region_t r;
    size_t u32_cnt;
    size_t u64_cnt;
  } kValidBoundaries[] = {
      // 1 byte edge boundaries for both of the types
      {.r = {.base = kU32Max, .size = 1}, .u32_cnt = 1, .u64_cnt = 0},
      {.r = {.base = 1, .size = kU32Max}, .u32_cnt = 1, .u64_cnt = 0},
      {.r = {.base = 0xFFFF'FFF0, .size = 16}, .u32_cnt = 1, .u64_cnt = 0},
      {.r = {.base = kU32Max / 2, .size = 32}, .u32_cnt = 1, .u64_cnt = 0},
      // Add one entry within 64 bit bounds, right at 4GiB.
      {.r = {.base = 0x1'0000'0000, .size = page_size()}, .u32_cnt = 0, .u64_cnt = 1},
      {.r = {.base = kU64Max - page_size(), .size = page_size()}, .u32_cnt = 0, .u64_cnt = 1}};

  for (auto& tc : kValidBoundaries) {
    printf("Test case: {%#lx, %#lx}, %zu, %zu}\n", tc.r.base, tc.r.size, tc.u32_cnt, tc.u64_cnt);
    EXPECT_OK(root_host().AddMmioRange(tc.r.base, tc.r.size));
    EXPECT_EQ(tc.u32_cnt, root_host().Mmio32().AvailableRegionCount());
    EXPECT_EQ(tc.u64_cnt, root_host().Mmio64().AvailableRegionCount());
    root_host().Mmio32().Reset();
    root_host().Mmio64().Reset();
  }
}

TEST_F(PciRootHostTests, MmioHelperBoundaryViolations) {
  struct ralloc_region_t kTestCases[] = {
      // size is zero
      {.base = 0u, .size = 0u},
      // size is greater than u32 max, but overflow result is 0
      {.base = 0u, .size = 0x1'0000'0000},
      // same as above but twice overflowed
      {.base = 0u, .size = 0x2'0000'0000},
      // overflow from both parameters for u32
      {.base = kU32Max, .size = kU32Max},
      {.base = kU32Max, .size = 2},
      {.base = 2, .size = kU32Max},
      // region spans from < 4GB to > 4GB
      {.base = kU32Max, .size = 2},
      {.base = 0, .size = kU64Max},
      // Overflow 64 bit address space
      {.base = 2, .size = kU64Max},
      {.base = kU64Max, .size = 2},
  };

  for (auto& tc : kTestCases) {
    printf("Test case: {%#lx, %#lx}\n", tc.base, tc.size);
    EXPECT_EQ(root_host().AddMmioRange(tc.base, tc.size).status_value(), ZX_ERR_INVALID_ARGS);
    EXPECT_EQ(0u, root_host().Mmio32().AvailableRegionCount());
    EXPECT_EQ(0u, root_host().Mmio64().AvailableRegionCount());
  }
}
