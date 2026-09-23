// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef PLATFORM_DEVICE_H
#define PLATFORM_DEVICE_H

#include <lib/magma/platform/platform_buffer.h>
#include <lib/magma/platform/platform_handle.h>
#include <lib/magma/platform/platform_interrupt.h>
#include <lib/magma/platform/platform_mmio.h>
#include <lib/magma/util/dlog.h>
#include <lib/magma/util/status.h>

#include <chrono>
#include <memory>

namespace magma {

class PlatformDevice {
 public:
  // See zircon/syscalls/profile.h
  enum Priority {
    kPriorityLowest = 0,
    kPriorityLow = 8,
    kPriorityDefault = 16,
    kPriorityHigher = 20,
    kPriorityHigh = 24,
    kPriorityHighest = 31
  };

  virtual ~PlatformDevice() { MAGMA_DLOG("PlatformDevice dtor"); }

  virtual void* GetDeviceHandle() = 0;

  virtual uint32_t GetMmioCount() const = 0;

  virtual std::unique_ptr<PlatformHandle> GetBusTransactionInitiator() const = 0;

  // Map an MMIO listed at |index| in the MDI for this device.
  virtual std::unique_ptr<PlatformMmio> CpuMapMmio(unsigned int index) {
    MAGMA_DLOG("CpuMapMmio unimplemented");
    return nullptr;
  }

  virtual std::unique_ptr<PlatformBuffer> GetMmioBuffer(unsigned int index) {
    MAGMA_DLOG("GetMmioBuffer unimplemented");
    return nullptr;
  }

  // Register an interrupt listed at |index| in the MDI for this device.
  virtual std::unique_ptr<PlatformInterrupt> RegisterInterrupt(unsigned int index) {
    MAGMA_DLOG("RegisterInterrupt unimplemented");
    return nullptr;
  }

  // Ownership of |device_handle| is *not* transferred to the PlatformDevice.
  static std::unique_ptr<PlatformDevice> Create(void* device_handle);
};

}  // namespace magma

#endif  // PLATFORM_DEVICE_H
