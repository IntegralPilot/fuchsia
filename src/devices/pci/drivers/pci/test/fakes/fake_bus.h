// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_DEVICES_PCI_DRIVERS_PCI_TEST_FAKES_FAKE_BUS_H_
#define SRC_DEVICES_PCI_DRIVERS_PCI_TEST_FAKES_FAKE_BUS_H_

#include <fuchsia/hardware/pciroot/cpp/banjo.h>
#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/zx/msi.h>
#include <lib/zx/resource.h>

#include <hwreg/bitfields.h>

#include "src/devices/pci/drivers/pci/bus.h"
#include "src/devices/pci/drivers/pci/test/fakes/fake_pciroot.h"

namespace pci {

class FakeBus : public BusDeviceInterface {
 public:
  explicit FakeBus(uint8_t bus_start, uint8_t bus_end, bool is_extended, uint16_t segment_group = 0)
      : pciroot_(bus_start, bus_end, is_extended, segment_group) {}

  zx_status_t LinkDevice(fbl::RefPtr<pci::Device> device) final {
    fbl::AutoLock devices_lock(&devices_lock_);
    return devices_.try_emplace(device->config()->bdf(), std::move(device)).second
               ? ZX_OK
               : ZX_ERR_ALREADY_EXISTS;
  }

  zx_status_t UnlinkDevice(pci::Device* device) final {
    fbl::AutoLock devices_lock(&devices_lock_);
    return devices_.erase(device->config()->bdf()) > 0 ? ZX_OK : ZX_ERR_NOT_FOUND;
  }

  zx_status_t AllocateMsi(uint32_t count, zx::msi* msi, msi_allocation_info_t* out_info) final {
    fbl::AutoLock devices_lock(&devices_lock_);
    // Using fake MSIs supplied by lib/fake-msi
    zx_status_t status = zx::msi::allocate(*zx::unowned_resource(ZX_HANDLE_INVALID), count, msi);
    if (status != ZX_OK) {
      return status;
    }
    zx_info_msi_t msi_info;
    status = msi->get_info(ZX_INFO_MSI, &msi_info, sizeof(msi_info), nullptr, nullptr);
    if (status != ZX_OK) {
      return status;
    }
    out_info->target_addr = msi_info.target_addr;
    out_info->target_data = msi_info.target_data;
    out_info->irq_count = msi_info.num_irq;
    return ZX_OK;
  }

  zx_status_t GetMsiHandle(const zx::msi& allocation, uint32_t options, uint16_t msi_id,
                           const zx::vmo& cfg_vmo, uint64_t cfg_offset,
                           zx::interrupt* out_interrupt) final {
    return zx::msi::create(allocation, options, msi_id, cfg_vmo, cfg_offset, out_interrupt);
  }

  zx_status_t GetBti(const pci::Device* /*device*/, uint32_t /*index*/, zx::bti* /*bti*/) final {
    fbl::AutoLock devices_lock(&devices_lock_);
    return ZX_ERR_NOT_SUPPORTED;
  }

  uint16_t GetSegmentGroup() final { return pciroot_.info().segment_group; }

  zx_status_t AddToSharedIrqList(pci::Device* device, uint32_t vector,
                                 zx::unowned_interrupt /*irq_handle*/) final {
    return ZX_OK;
  }
  zx_status_t RemoveFromSharedIrqList(pci::Device* device, uint32_t vector) final { return ZX_OK; }

  pci::Device& get_device(pci_bdf_t bdf) { return *devices_.at(bdf); }

  // For use with Devices that need to link to a Bus.
  BusDeviceInterface* bdi() { return static_cast<BusDeviceInterface*>(this); }

  const pci::DeviceTree& devices() { return devices_; }
  FakePciroot& pciroot() { return pciroot_; }

 private:
  fbl::Mutex devices_lock_;
  pci::DeviceTree devices_;
  FakePciroot pciroot_;
};

}  // namespace pci

#endif  // SRC_DEVICES_PCI_DRIVERS_PCI_TEST_FAKES_FAKE_BUS_H_
