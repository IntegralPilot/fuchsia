// Copyright 2024 Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "zircon_platform_device_dfv2.h"

#include <lib/magma/platform/platform_mmio.h>
#include <lib/magma/util/dlog.h>
#include <lib/magma/util/short_macros.h>
#include <lib/zx/bti.h>
#include <lib/zx/vmo.h>
#include <zircon/process.h>

#include "zircon_platform_handle.h"
#include "zircon_platform_interrupt.h"
#include "zircon_platform_mmio.h"

namespace magma {

std::unique_ptr<PlatformHandle> ZirconPlatformDeviceDfv2::GetBusTransactionInitiator() const {
  auto res = pdev_->GetBtiById(0);
  if (!res.ok()) {
    DMESSAGE("failed to get bus transaction initiator: %s", res.status_string());
    return std::make_unique<ZirconPlatformHandle>(zx::handle());
  }
  if (!res->is_ok()) {
    DMESSAGE("failed to get bus transaction initiator: %d", res->error_value());
    return std::make_unique<ZirconPlatformHandle>(zx::handle());
  }
  return std::make_unique<ZirconPlatformHandle>(std::move(res.value()->bti));
}

std::unique_ptr<PlatformMmio> ZirconPlatformDeviceDfv2::CpuMapMmio(unsigned int index) {
  auto res = pdev_->GetMmioById(index);
  if (!res.ok()) {
    DMESSAGE("failed to get mmio: %s", res.status_string());
    return nullptr;
  }
  if (!res->is_ok()) {
    DMESSAGE("failed to get mmio: %d", res->error_value());
    return nullptr;
  }

  size_t offset = 0;
  size_t size = 0;
  zx::vmo vmo;
  if (res->value()->has_offset()) {
    offset = res->value()->offset();
  }
  if (res->value()->has_size()) {
    size = res->value()->size();
  }
  if (res->value()->has_vmo()) {
    vmo = std::move(res->value()->vmo());
  }

  auto mmio_buffer =
      fdf::MmioBuffer::Create(offset, size, std::move(vmo), ZX_CACHE_POLICY_UNCACHED_DEVICE);
  if (!mmio_buffer.is_ok()) {
    DMESSAGE("Failed to make mmio buffer %s", mmio_buffer.status_string());
    return nullptr;
  }

  std::unique_ptr<magma::ZirconPlatformMmio> mmio(
      new magma::ZirconPlatformMmio(std::move(mmio_buffer.value())));
  return mmio;
}

uint32_t ZirconPlatformDeviceDfv2::GetMmioCount() const {
  auto res = pdev_->GetNodeDeviceInfo();
  if (!res.ok()) {
    DMESSAGE("failed to get mmio count: %s", res.status_string());
    return 0;
  }
  if (!res->is_ok()) {
    DMESSAGE("failed to get mmio count: %d", res->error_value());
    return 0;
  }
  return res->value()->mmio_count();
}

std::unique_ptr<PlatformBuffer> ZirconPlatformDeviceDfv2::GetMmioBuffer(unsigned int index) {
  auto res = pdev_->GetMmioById(index);
  if (!res.ok()) {
    DMESSAGE("failed to get mmio: %s", res.status_string());
    return nullptr;
  }
  if (!res->is_ok()) {
    DMESSAGE("failed to get mmio: %d", res->error_value());
    return nullptr;
  }
  zx::vmo vmo;
  if (res->value()->has_vmo()) {
    vmo = std::move(res->value()->vmo());
  }
  return magma::PlatformBuffer::Import(std::move(vmo));
}

std::unique_ptr<PlatformInterrupt> ZirconPlatformDeviceDfv2::RegisterInterrupt(unsigned int index) {
  auto res = pdev_->GetInterruptById(index, 0);
  if (!res.ok()) {
    DMESSAGE("failed to register interrupt: %s", res.status_string());
    return nullptr;
  }
  if (!res->is_ok()) {
    DMESSAGE("failed to register interrupt: %d", res->error_value());
    return nullptr;
  }

  return std::make_unique<ZirconPlatformInterrupt>(zx::handle(std::move(res->value()->irq)));
}

}  // namespace magma
