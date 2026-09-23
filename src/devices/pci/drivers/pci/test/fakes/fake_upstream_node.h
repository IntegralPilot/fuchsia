// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_PCI_DRIVERS_PCI_TEST_FAKES_FAKE_UPSTREAM_NODE_H_
#define SRC_DEVICES_PCI_DRIVERS_PCI_TEST_FAKES_FAKE_UPSTREAM_NODE_H_

#include "src/devices/pci/drivers/pci/ref_counted.h"
#include "src/devices/pci/drivers/pci/test/fakes/fake_allocator.h"
#include "src/devices/pci/drivers/pci/upstream_node.h"

namespace pci {

class FakeUpstreamNode : public UpstreamNode {
 public:
  FakeUpstreamNode(Type type, uint32_t mbus_id)
      : UpstreamNode(type, mbus_id),
        pf_mmio_alloc_(PCI_ADDRESS_SPACE_MEMORY),
        mmio_alloc_(PCI_ADDRESS_SPACE_MEMORY),
#ifdef __x86_64__
        pio_alloc_(PCI_ADDRESS_SPACE_IO)
#elif defined(__aarch64__) || defined(__riscv)
        pio_alloc_(PCI_ADDRESS_SPACE_MEMORY)
#endif
  {
  }

  PciAllocator& pf_mmio_regions() final { return pf_mmio_alloc_; }
  PciAllocator& mmio_regions() final { return mmio_alloc_; }
  PciAllocator& pio_regions() final { return pio_alloc_; }

  FakeAllocator& fake_pf_mmio_regions() { return pf_mmio_alloc_; }
  FakeAllocator& fake_mmio_regions() { return mmio_alloc_; }
  FakeAllocator& fake_pio_regions() { return pio_alloc_; }

  void ConfigureDownstreamDevices() final { UpstreamNode::ConfigureDownstreamDevices(); }
  void UnplugDownstream() final { UpstreamNode::UnplugDownstream(); }
  void DisableDownstream() final { UpstreamNode::DisableDownstream(); }
  zx_status_t SetBusMasteringUpstream(bool) final { return ZX_OK; }

  PCI_IMPLEMENT_REFCOUNTED;

 private:
  FakeAllocator pf_mmio_alloc_;
  FakeAllocator mmio_alloc_;
  FakeAllocator pio_alloc_;
};

}  // namespace pci

#endif  // SRC_DEVICES_PCI_DRIVERS_PCI_TEST_FAKES_FAKE_UPSTREAM_NODE_H_
