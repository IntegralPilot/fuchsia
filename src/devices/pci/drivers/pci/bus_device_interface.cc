// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include <zircon/errors.h>
#include <zircon/status.h>

#include <algorithm>

#include "fbl/auto_lock.h"
#include "lib/zx/vmo.h"
#include "src/devices/pci/drivers/pci/bus.h"

namespace pci {

zx_status_t Bus::LinkDevice(fbl::RefPtr<pci::Device> device) {
  fbl::AutoLock _(&devices_lock_);
  return devices_.try_emplace(device->config()->bdf(), std::move(device)).second
             ? ZX_OK
             : ZX_ERR_ALREADY_EXISTS;
}

zx_status_t Bus::UnlinkDevice(pci::Device* device) {
  fbl::AutoLock _(&devices_lock_);
  ZX_DEBUG_ASSERT(device);
  return devices_.erase(device->config()->bdf()) > 0 ? ZX_OK : ZX_ERR_NOT_FOUND;
}

zx_status_t Bus::AllocateMsi(uint32_t count, zx::msi* msi, msi_allocation_info_t* out_info) {
  fbl::AutoLock _(&devices_lock_);
  return pciroot().AllocateMsi(count, false, msi, out_info);
}

zx_status_t Bus::GetMsiHandle(const zx::msi& allocation, uint32_t options, uint16_t msi_id,
                              const zx::vmo& cfg_vmo, uint64_t cfg_offset,
                              zx::interrupt* out_interrupt) {
  fbl::AutoLock devices_lock(&devices_lock_);

  zx::msi dup_allocation;
  zx_status_t status = allocation.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup_allocation);
  if (status != ZX_OK) {
    return status;
  }

  zx::vmo dup_cfg_vmo;
  if (cfg_vmo.is_valid()) {
    status = cfg_vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup_cfg_vmo);
    if (status != ZX_OK) {
      return status;
    }
  }

  return pciroot().GetMsiHandle(std::move(dup_allocation), options, msi_id, std::move(dup_cfg_vmo),
                                cfg_offset, out_interrupt);
}

zx_status_t Bus::GetBti(const pci::Device* device, uint32_t index, zx::bti* bti) {
  if (!device) {
    return ZX_ERR_INVALID_ARGS;
  }
  fbl::AutoLock devices_lock(&devices_lock_);
  return pciroot().GetBti(device->packed_addr(), index, bti);
}

zx_status_t Bus::AddToSharedIrqList(pci::Device* device, uint32_t vector,
                                    zx::unowned_interrupt irq_handle) {
  ZX_DEBUG_ASSERT(vector);
  fbl::AutoLock _(&devices_lock_);

  auto result = shared_irqs_.find(vector);
  if (result == shared_irqs_.end()) {
    return ZX_ERR_BAD_STATE;
  }

  auto& shared_vector = result->second;
  auto& list = shared_vector->list;
  if (std::any_of(list.begin(), list.end(),
                  [device](const auto& entry) { return entry->device == device; })) {
    return ZX_ERR_ALREADY_EXISTS;
  }

  auto shared_dev = std::make_unique<SharedDevice>();
  shared_dev->device = device;
  shared_dev->wait.set_object(irq_handle->get());
  shared_dev->wait.set_trigger(ZX_VIRTUAL_INTERRUPT_UNTRIGGERED);
  shared_dev->wait.set_handler([device](async_dispatcher_t* dispatcher, async::Wait* wait,
                                        zx_status_t status, const zx_packet_signal_t* signal) {
    HandleDeviceLegacyIrqUntriggered(dispatcher, wait, status, signal, device);
  });
  list.push_back(std::move(shared_dev));
  zxlogf(TRACE, "[%s] inserted into list for vector %#x", device->config()->addr(), vector);
  return ZX_OK;
}

uint16_t Bus::GetSegmentGroup() { return info_.segment_group; }

zx_status_t Bus::RemoveFromSharedIrqList(pci::Device* device, uint32_t vector) {
  ZX_DEBUG_ASSERT(vector);
  fbl::AutoLock _(&devices_lock_);

  auto result = shared_irqs_.find(vector);
  if (result == shared_irqs_.end()) {
    return ZX_ERR_BAD_STATE;
  }

  auto& shared_vector = result->second;
  const auto removed_device = std::erase_if(
      shared_vector->list, [device](const auto& entry) { return entry->device == device; });
  if (!removed_device) {
    return ZX_ERR_NOT_FOUND;
  }

  zxlogf(TRACE, "[%s] removed from vector %#x list", device->config()->addr(), vector);
  return ZX_OK;
}

}  // namespace pci
