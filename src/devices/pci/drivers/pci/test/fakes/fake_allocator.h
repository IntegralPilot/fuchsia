// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_DEVICES_PCI_DRIVERS_PCI_TEST_FAKES_FAKE_ALLOCATOR_H_
#define SRC_DEVICES_PCI_DRIVERS_PCI_TEST_FAKES_FAKE_ALLOCATOR_H_

#include <fuchsia/hardware/pciroot/cpp/banjo.h>
#include <lib/fake-resource/resource.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <zircon/types.h>

#include <memory>
#include <optional>
#include <vector>

#include "src/devices/pci/drivers/pci/allocation.h"

namespace pci {

// Normally we would track the allocations and assert on issues during
// cleanup, but presently with an IsolatedDevmgr we don't have a way
// to cleanly tear down the FakeBusDriver, so no dtors on anything
// will be called anyway.
class FakeAllocation : public PciAllocation {
 public:
  FakeAllocation(pci_address_space_t type, std::optional<zx_paddr_t> base, size_t size)
      : PciAllocation(type, zx::resource(ZX_HANDLE_INVALID)),
        base_((base.has_value()) ? *base : 0),
        size_(size) {
    zxlogf(DEBUG, "fake allocation created [%#lx, %#lx)", base_, base_ + size);
  }
  zx_paddr_t base() const final { return base_; }
  size_t size() const final { return size_; }
  zx::result<zx::vmo> CreateVmo() const final {
    zx::vmo vmo;
    const zx_status_t status = zx::vmo::create(size_, 0, &vmo);
    if (status != ZX_OK) {
      return zx::error(status);
    }
    return zx::ok(std::move(vmo));
  }

  zx::result<zx::resource> CreateResource() const final {
    zx_handle_t handle = ZX_HANDLE_INVALID;
    zx_rsrc_kind_t kind =
        (type() == PCI_ADDRESS_SPACE_MEMORY) ? ZX_RSRC_KIND_MMIO : ZX_RSRC_KIND_IOPORT;
    ZX_DEBUG_ASSERT(fake_resource_create(kind, &handle) == ZX_OK);
    auto resource = zx::resource(handle);
    return zx::ok(std::move(resource));
  }

 private:
  zx_paddr_t base_;
  size_t size_;
};

struct AllocationLogEntry {
  size_t size;
  bool succeeded;

  bool operator==(const AllocationLogEntry&) const = default;
};

// This fake will fulfill any allocation so long as it isn't configured to fail by calling
// |FailNextAllocation|.
class FakeAllocator : public PciAllocator {
 public:
  explicit FakeAllocator(pci_address_space_t type) : PciAllocator(type) {}
  // Arms the allocator to fail the next allocation. If |assigned_only| is true
  // (default), only fails allocations that request a specific base address.
  void FailNextAllocation(bool assigned_only = true) {
    if (assigned_only) {
      fail_next_assigned_ = true;
    } else {
      fail_next_any_ = true;
    }
  }
  zx::result<std::unique_ptr<PciAllocation>> Allocate(std::optional<zx_paddr_t> in_base,
                                                      size_t size) final {
    if (fail_next_any_ || (fail_next_assigned_ && in_base.has_value())) {
      fail_next_any_ = false;
      fail_next_assigned_ = false;
      allocation_log_.push_back({.size = size, .succeeded = false});
      return zx::error(ZX_ERR_NOT_FOUND);
    }

    // In a normal reallocation use the requested base, but in a forced it
    // should align to the size so that's a convenient placeholder.
    const zx_paddr_t base = (in_base.has_value()) ? *in_base : size;
    auto allocation = std::unique_ptr<PciAllocation>(new FakeAllocation(type(), base, size));
    allocation_log_.push_back({.size = size, .succeeded = true});
    return zx::ok(std::move(allocation));
  }

  zx_status_t SetParentAllocation(std::unique_ptr<PciAllocation> alloc) final {
    (void)alloc.release();
    return ZX_OK;
  }

  const std::vector<AllocationLogEntry>& allocation_log() const { return allocation_log_; }

 private:
  bool fail_next_assigned_ = false;
  bool fail_next_any_ = false;
  std::vector<AllocationLogEntry> allocation_log_;
};

}  // namespace pci

#endif  // SRC_DEVICES_PCI_DRIVERS_PCI_TEST_FAKES_FAKE_ALLOCATOR_H_
