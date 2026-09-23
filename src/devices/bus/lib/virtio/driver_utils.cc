// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/virtio/backends/pci.h>
#include <lib/virtio/driver_utils.h>
#include <lib/zx/result.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#include <memory>

#include "src/graphics/display/lib/driver-framework-migration-utils/logging/zxlogf.h"

namespace virtio {

zx::result<std::pair<zx::bti, std::unique_ptr<virtio::Backend>>> GetBtiAndBackend(
    fidl::ClientEnd<fuchsia_hardware_pci::Device> pci) {
  if (!pci.is_valid()) {
    zxlogf(ERROR, "pci client invalid");
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  fidl::Result info = fidl::Call(pci)->GetDeviceInfo();
  if (info.is_error()) {
    return zx::error(info.error_value().status());
  }

  fidl::Result bti = fidl::Call(pci)->GetBti(0);
  if (bti.is_error()) {
    if (bti.error_value().is_domain_error()) {
      return zx::error(bti.error_value().domain_error());
    }
    return zx::error(bti.error_value().framework_error().status());
  }

  // Due to the similarity between Virtio 0.9.5 legacy devices and Virtio 1.0
  // transitional devices we need to check whether modern capabilities exist.
  // If no vendor capabilities are found then we will default to the legacy
  // interface.
  std::unique_ptr<virtio::Backend> backend = nullptr;
  fidl::Result result =
      fidl::Call(pci)->GetCapabilities(fuchsia_hardware_pci::CapabilityId::kVendor);
  bool is_modern = result.is_ok() && !result->offsets().empty();
  if (is_modern) {
    backend = std::make_unique<virtio::PciModernBackend>(std::move(pci), info->info());
  } else {
    backend = std::make_unique<virtio::PciLegacyBackend>(std::move(pci), info->info());
  }
  zxlogf(TRACE, "virtio %02x:%02x.%1x using %s PCI backend", info->info().bus_id(),
         info->info().dev_id(), info->info().func_id(), (is_modern) ? "modern" : "legacy");

  zx_status_t status = backend->Bind();
  if (status != ZX_OK) {
    return zx::error(status);
  }

  return zx::ok(std::make_pair(std::move(bti->bti()), std::move(backend)));
}

}  // namespace virtio
