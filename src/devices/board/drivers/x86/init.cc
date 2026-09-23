// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/ddk/debug.h>
#include <limits.h>

#include <acpica/acpi.h>

#include "acpi-private.h"
#include "dev.h"
#include "errors.h"
#include "src/devices/board/drivers/x86/x86_config.h"
#include "src/devices/board/lib/acpi/acpi.h"
#include "src/devices/board/lib/acpi/util.h"
#include "x86.h"

namespace x86 {

bool use_hardware_iommu(zx_device_t* dev) {
  zx::vmo config_vmo;
  if (device_get_config_vmo(dev, config_vmo.reset_and_get_address()) != ZX_OK) {
    return false;
  }
  auto config = x86_config::Config::CreateFromVmo(std::move(config_vmo));
  return config.use_hardware_iommu();
}

zx_status_t X86::EarlyAcpiInit() {
  ZX_DEBUG_ASSERT(!acpica_initialized_);
  // First initialize the ACPI subsystem.
  zx_status_t status = acpi_->InitializeAcpi().zx_status_value();
  if (status != ZX_OK) {
    return status;
  }
  acpica_initialized_ = true;
  return ZX_OK;
}

zx_status_t X86::EarlyInit() {
  zx_status_t status = EarlyAcpiInit();
  if (status != ZX_OK) {
    return status;
  }

  zx::unowned_resource iommu_resource(get_iommu_resource(parent()));
  // Now initialize the IOMMU manager. Any failures in setting it up we consider non-fatal and do
  // not propagate.
  status = iommu_manager_.Init(std::move(iommu_resource), use_hardware_iommu(parent()));
  if (status != ZX_OK) {
    zxlogf(INFO, "acpi: Failed to initialize IOMMU manager: %s", zx_status_get_string(status));
  }
  return ZX_OK;
}

}  // namespace x86
