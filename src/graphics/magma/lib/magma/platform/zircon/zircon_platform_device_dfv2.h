// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_MAGMA_LIB_MAGMA_PLATFORM_ZIRCON_ZIRCON_PLATFORM_DEVICE_DFV2_H_
#define SRC_GRAPHICS_MAGMA_LIB_MAGMA_PLATFORM_ZIRCON_ZIRCON_PLATFORM_DEVICE_DFV2_H_

#include <fidl/fuchsia.hardware.platform.device/cpp/wire.h>
#include <lib/magma/platform/platform_device.h>

namespace magma {

class ZirconPlatformDeviceDfv2 final : public PlatformDevice {
 public:
  explicit ZirconPlatformDeviceDfv2(
      fidl::WireSyncClient<fuchsia_hardware_platform_device::Device> pdev)
      : pdev_(std::move(pdev)) {}

  void* GetDeviceHandle() override { return &pdev_; }

  std::unique_ptr<PlatformHandle> GetBusTransactionInitiator() const override;

  std::unique_ptr<PlatformMmio> CpuMapMmio(unsigned int index) override;

  uint32_t GetMmioCount() const override;

  std::unique_ptr<PlatformBuffer> GetMmioBuffer(unsigned int index) override;

  std::unique_ptr<PlatformInterrupt> RegisterInterrupt(unsigned int index) override;

  fidl::WireSyncClient<fuchsia_hardware_platform_device::Device>& fidl() { return pdev_; }

 private:
  fidl::WireSyncClient<fuchsia_hardware_platform_device::Device> pdev_;
};

}  // namespace magma

#endif  // SRC_GRAPHICS_MAGMA_LIB_MAGMA_PLATFORM_ZIRCON_ZIRCON_PLATFORM_DEVICE_DFV2_H_
