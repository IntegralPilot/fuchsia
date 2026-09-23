// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef PLATFORM_PCI_DEVICE_H
#define PLATFORM_PCI_DEVICE_H

#include <lib/magma/platform/platform_handle.h>
#include <lib/magma/platform/platform_interrupt.h>
#include <lib/magma/platform/platform_mmio.h>
#include <lib/magma/util/dlog.h>

#include <memory>

namespace magma {

class PlatformPciDevice {
 public:
  virtual ~PlatformPciDevice() { MAGMA_DLOG("PlatformPciDevice dtor"); }

  virtual void* GetDeviceHandle() = 0;

  virtual std::unique_ptr<PlatformHandle> GetBusTransactionInitiator() {
    MAGMA_DLOG("GetBusTransactionInitiator unimplemented");
    return nullptr;
  }

  virtual bool ReadPciConfig16(uint64_t addr, uint16_t* value) {
    MAGMA_DLOG("ReadPciConfig16 unimplemented");
    return false;
  }

  virtual std::unique_ptr<PlatformMmio> CpuMapPciMmio(unsigned int pci_bar) {
    MAGMA_DLOG("CpuMapPciMmio unimplemented");
    return nullptr;
  }

  virtual std::unique_ptr<PlatformInterrupt> RegisterInterrupt() {
    MAGMA_DLOG("RegisterInterrupt unimplemented");
    return nullptr;
  }

  static std::unique_ptr<PlatformPciDevice> Create(void* device_handle);
};

}  // namespace magma

#endif  // PLATFORM_PCI_DEVICE_H
