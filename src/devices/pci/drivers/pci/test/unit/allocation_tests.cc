// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/hardware/pciroot/cpp/banjo.h>
#include <lib/zx/result.h>
#include <zircon/limits.h>
#include <zircon/syscalls.h>

#include <memory>
#include <optional>

#include <gtest/gtest.h>

#include "src/devices/pci/drivers/pci/allocation.h"
#include "src/devices/pci/drivers/pci/test/fakes/fake_allocator.h"
#include "src/devices/pci/drivers/pci/test/fakes/fake_pciroot.h"
#include "src/lib/testing/predicates/status.h"

namespace pci {
namespace {

FakePciroot* RetrieveFakeFromClient(const ddk::PcirootProtocolClient& client) {
  pciroot_protocol_t proto;
  client.GetProto(&proto);
  return static_cast<FakePciroot*>(proto.ctx);
}

// Tests that GetAddressSpace / FreeAddressSpace are equally called when
// allocations using PcirootProtocol are created and freed through
// PciRootAllocation and PciRegionAllocation dtors.
TEST(PciAllocationTest, BalancedAllocation) {
  FakePciroot pciroot;
  ddk::PcirootProtocolClient client(pciroot.proto());
  FakePciroot* fake_impl = RetrieveFakeFromClient(client);
  PciRootAllocator root_alloc(client, PCI_ADDRESS_SPACE_MEMORY, false);
  {
    auto alloc1 = root_alloc.Allocate(std::nullopt, zx_system_get_page_size());
    EXPECT_TRUE(alloc1.is_ok());
    EXPECT_EQ(1u, fake_impl->allocation_eps().size());
    auto alloc2 = root_alloc.Allocate(1024u, zx_system_get_page_size());
    EXPECT_TRUE(alloc2.is_ok());
    EXPECT_EQ(2u, fake_impl->allocation_eps().size());
  }

  // TODO(https://fxbug.dev/42108122): Rework this with the new eventpair model of GetAddressSpace
  // EXPECT_EQ(0, fake_impl->allocation_cnt());
}

TEST(PciAllocationTest, RootNaturalAlignment) {
  FakePciroot pciroot;
  ddk::PcirootProtocolClient client(pciroot.proto());
  PciRootAllocator root_alloc(client, PCI_ADDRESS_SPACE_MEMORY, false);

  // Allocate the front 1024 bytes.
  auto alloc1 = root_alloc.Allocate(std::nullopt, 0x400);
  ASSERT_TRUE(alloc1.is_ok());

  // Attempt to allocate 8192, which needs to be naturally aligned to the same
  // size. It cannot start at 1024 where the previous allocation ended.
  size_t alloc2_sz = 8192;
  auto alloc2 = root_alloc.Allocate(std::nullopt, alloc2_sz);
  ASSERT_TRUE(alloc2.is_ok());
  EXPECT_TRUE(alloc2->base() % alloc2_sz == 0);
  EXPECT_EQ(alloc2->size(), alloc2_sz);
}

TEST(PciAllocationTest, RegionNaturalAlignment) {
  FakePciroot pciroot;
  ddk::PcirootProtocolClient client(pciroot.proto());
  PciRootAllocator root_alloc(client, PCI_ADDRESS_SPACE_MEMORY, false);

  auto result = root_alloc.Allocate(std::nullopt, 0x1000000);
  std::unique_ptr<PciAllocation> root_allocation = std::move(result.value());
  PciRegionAllocator pci_allocator;
  pci_allocator.SetParentAllocation(std::move(root_allocation));

  // Allocate the front 1024 bytes.
  auto alloc1 = pci_allocator.Allocate(std::nullopt, 0x400);
  ASSERT_TRUE(alloc1.is_ok());

  // Attempt to allocate 16K, which needs to be naturally aligned to the same
  // size. It cannot start at 1024 where the previous allocation ended.
  size_t alloc2_sz = 0x10000;
  auto alloc2 = pci_allocator.Allocate(std::nullopt, alloc2_sz);
  ASSERT_TRUE(alloc2.is_ok());
  EXPECT_EQ(alloc2->size(), alloc2_sz);
  EXPECT_EQ(alloc2->base() & (alloc2_sz - 1), 0u);
}

TEST(PciAllocationTest, ZeroSize) {
  FakePciroot pciroot;
  ddk::PcirootProtocolClient client(pciroot.proto());
  PciRootAllocator root_alloc(client, PCI_ADDRESS_SPACE_MEMORY, false);

  ASSERT_TRUE(root_alloc.Allocate(0, 0).is_error());
}

// Since test allocations lack a valid resource they should fail when
// CreateVMObject is called
TEST(PciAllocationTest, VmoCreationFailure) {
  FakePciroot pciroot;
  ddk::PcirootProtocolClient client(pciroot.proto());

  zx::vmo vmo;
  PciRootAllocator root(client, PCI_ADDRESS_SPACE_MEMORY, false);
  PciAllocator* root_ptr = &root;
  auto alloc = root_ptr->Allocate(std::nullopt, zx_system_get_page_size());
  EXPECT_TRUE(alloc.is_ok());
  EXPECT_OK(alloc->CreateVmo().status_value());
}

// Ensure that all allocator and allocation types report the correct address
// space type even as they're passed to downstream allocators.
void AllocationTypeHelper(pci_address_space_t type) {
  FakePciroot pciroot;
  ddk::PcirootProtocolClient client(pciroot.proto());

  PciRootAllocator root_allocator(client, type, false);
  EXPECT_EQ(root_allocator.type(), type);

  size_t page_size = zx_system_get_page_size();
  auto root_result = root_allocator.Allocate(std::nullopt, page_size * 4);
  ASSERT_OK(root_result.status_value());
  ASSERT_EQ(root_result->type(), type);

  PciRegionAllocator region_allocator;
  ASSERT_OK(region_allocator.SetParentAllocation(std::move(root_result.value())));
  ASSERT_EQ(region_allocator.type(), type);

  auto region_allocator_result = region_allocator.Allocate(std::nullopt, page_size);
  ASSERT_OK(region_allocator_result.status_value());
  ASSERT_EQ(region_allocator_result->type(), type);
}

TEST(PciAllocationTest, IoType) {
  EXPECT_NO_FATAL_FAILURE(AllocationTypeHelper(PCI_ADDRESS_SPACE_IO));
}

TEST(PciAllocationTest, MmioType) {
  EXPECT_NO_FATAL_FAILURE(AllocationTypeHelper(PCI_ADDRESS_SPACE_MEMORY));
}

// A PciRegionAllocator has no type until it is given a backing allocation and should assert.
TEST(PciAllocationTest, RegionTypeNone) {
  PciRegionAllocator allocator;
  EXPECT_EQ(allocator.type(), PCI_ADDRESS_SPACE_NONE);

  pci_address_space_t type = PCI_ADDRESS_SPACE_MEMORY;
  auto allocation = std::make_unique<FakeAllocation>(type, std::nullopt, 1024);
  ASSERT_OK(allocator.SetParentAllocation(std::move(allocation)));
  EXPECT_EQ(allocator.type(), type);
}

}  // namespace
}  // namespace pci
