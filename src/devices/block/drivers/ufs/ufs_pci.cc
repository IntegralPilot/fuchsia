// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/block/drivers/ufs/ufs_pci.h"

#include <fidl/fuchsia.hardware.pci/cpp/wire.h>
#include <lib/driver/component/cpp/driver_export.h>
#include <lib/driver/logging/cpp/logger.h>

namespace ufs {

zx::result<> UfsPci::InitResources() {
  auto pci_client_end = driver_incoming()->Connect<fuchsia_hardware_pci::Service::Device>("pci");
  if (!pci_client_end.is_ok()) {
    fdf::error("Failed to connect to PCI device service: {}", pci_client_end);
    return pci_client_end.take_error();
  }
  pci_ = fidl::WireSyncClient<fuchsia_hardware_pci::Device>(*std::move(pci_client_end));

  // Map register window.
  {
    const auto result = pci_->GetBar(0);
    if (!result.ok()) {
      fdf::error("Call to GetBar failed: {}", result.status_string());
      return zx::error(result.status());
    }
    if (result->is_error()) {
      fdf::error("GetBar failed: {}", zx_status_get_string(result->error_value()));
      return zx::error(result->error_value());
    }

    if (!result->value()->result.result.is_vmo()) {
      fdf::error("PCI BAR is not an MMIO BAR.");
      return zx::error(ZX_ERR_WRONG_TYPE);
    }
    mmio_buffer_vmo_ = std::move(result->value()->result.result.vmo());
    mmio_buffer_size_ = result->value()->result.size;
  }

  // UFS host controller is bus master
  {
    const auto result = pci_->SetBusMastering(true);
    if (!result.ok()) {
      fdf::error("Call to SetBusMastering failed: {}", result.status_string());
      return zx::error(result.status());
    }
    if (result->is_error()) {
      fdf::error("SetBusMastering failed: {}", zx_status_get_string(result->error_value()));
      return zx::error(result->error_value());
    }
  }

  // Request 1 interrupt of any mode.
  {
    const auto result = pci_->GetInterruptModes();
    if (!result.ok()) {
      fdf::error("Call to GetInterruptModes failed: {}", result.status_string());
      return zx::error(result.status());
    }
    if (result->modes.msix_count > 0) {
      irq_mode_ = fuchsia_hardware_pci::InterruptMode::kMsiX;
    } else if (result->modes.msi_count > 0) {
      irq_mode_ = fuchsia_hardware_pci::InterruptMode::kMsi;
    } else if (result->modes.has_legacy) {
      irq_mode_ = fuchsia_hardware_pci::InterruptMode::kLegacy;
    } else {
      fdf::error("No interrupt modes are supported.");
      return zx::error(ZX_ERR_NOT_SUPPORTED);
    }
    fdf::debug("Interrupt mode: {}", static_cast<uint8_t>(irq_mode_));
  }
  {
    const auto result = pci_->SetInterruptMode(irq_mode_, 1);
    if (!result.ok()) {
      fdf::error("Call to SetInterruptMode failed: {}", result.status_string());
      return zx::error(result.status());
    }
    if (result->is_error()) {
      fdf::error("SetInterruptMode failed: {}", zx_status_get_string(result->error_value()));
      return zx::error(result->error_value());
    }
  }

  // Get irq handle.
  {
    const auto result = pci_->MapInterrupt(0);
    if (!result.ok()) {
      fdf::error("Call to MapInterrupt failed: {}", result.status_string());
      return zx::error(result.status());
    }
    if (result->is_error()) {
      fdf::error("MapInterrupt failed: {}", zx_status_get_string(result->error_value()));
      return zx::error(result->error_value());
    }
    irq_ = std::move(result->value()->interrupt);
  }

  // Get bti handle.
  {
    const auto result = pci_->GetBti(0);
    if (!result.ok()) {
      fdf::error("Call to GetBti failed: {}", result.status_string());
      return zx::error(result.status());
    }
    if (result->is_error()) {
      fdf::error("GetBti failed: {}", zx_status_get_string(result->error_value()));
      return zx::error(result->error_value());
    }
    bti_ = std::move(result->value()->bti);
  }

  return zx::ok();
}

zx_status_t UfsPci::StopResources() {
  if (pci_.is_valid()) {
    const auto result = pci_->SetBusMastering(false);
    if (!result.ok()) {
      fdf::error("Call to SetBusMastering failed: {}", result.status_string());
      return result.status();
    }
    if (result->is_error()) {
      fdf::error("SetBusMastering failed: {}", zx_status_get_string(result->error_value()));
      return result->error_value();
    }
  }
  return ZX_OK;
}

zx::result<> UfsPci::InitQuirk() {
  fuchsia_hardware_pci::wire::DeviceInfo info;
  const auto result = pci_->GetDeviceInfo();
  if (!result.ok()) {
    fdf::error("Failed to get PCI device info: {}", result.status_string());
    return zx::error(result.status());
  }
  info = result->info;

  // Check that the current environment is QEMU.
  // Vendor ID = 0x1b36: Red Hat, Inc
  // Device ID = 0x0013: QEMU UFS Host Controller
  constexpr uint16_t kRedHatVendorId = 0x1b36;
  constexpr uint16_t kQemuUfsHostController = 0x0013;
  constexpr uint16_t kIntelVendorId = 0x8086;
  if ((info.vendor_id == kRedHatVendorId) && (info.device_id == kQemuUfsHostController)) {
    qemu_quirk_ = true;
  } else if (info.vendor_id == kIntelVendorId) {
    intel_quirk_ = true;
  }
  fdf::info("PCI device info: Vendor ID = 0x{:x}, Device ID = 0x{:x}", info.vendor_id,
            info.device_id);

  return zx::ok();
}

void UfsPci::OnIrqComplete() {
  if (irq_mode_ == fuchsia_hardware_pci::InterruptMode::kLegacy) {
    const fidl::WireResult result = pci_->AckInterrupt();
    if (!result.ok()) {
      fdf::error("Call to AckInterrupt failed: {}", result.status_string());
      return;
    }
    if (result->is_error()) {
      fdf::error("AckInterrupt failed: {}", zx_status_get_string(result->error_value()));
      return;
    }
  }
}

}  // namespace ufs
