// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "pci-bus.h"

#include <endian.h>
#include <lib/driver/component/cpp/driver_base.h>

namespace ahci {

PciBus::~PciBus() {}

zx_status_t PciBus::RegRead(size_t offset, uint32_t* val_out) {
  *val_out = le32toh(mmio_->Read32(offset));
  return ZX_OK;
}

zx_status_t PciBus::RegWrite(size_t offset, uint32_t val) {
  mmio_->Write32(htole32(val), offset);
  return ZX_OK;
}

zx_status_t PciBus::Configure() {
  if (!pci_.is_valid()) {
    fdf::error("Invalid client to PCI device service.");
    return ZX_ERR_NOT_SUPPORTED;
  }

  // Map register window.
  {
    const auto result = pci_->GetBar(5);
    if (!result.ok()) {
      fdf::error("Call to GetBar failed: {}", result.status_string());
      return result.status();
    }
    if (result->is_error()) {
      fdf::error("GetBar failed: {}", zx_status_get_string(result->error_value()));
      return result->error_value();
    }

    if (!result->value()->result.result.is_vmo()) {
      fdf::error("PCI BAR is not an MMIO BAR.");
      return ZX_ERR_WRONG_TYPE;
    }
    auto mmio = fdf::MmioBuffer::Create(0, result->value()->result.size,
                                        std::move(result->value()->result.result.vmo()),
                                        ZX_CACHE_POLICY_UNCACHED_DEVICE);
    if (mmio.is_error()) {
      fdf::error("Failed to map PCI register window: {}", mmio);
      return mmio.status_value();
    }
    mmio_ = *std::move(mmio);
  }

  fuchsia_hardware_pci::wire::DeviceInfo config;
  {
    const auto result = pci_->GetDeviceInfo();
    if (!result.ok()) {
      fdf::error("Call to GetDeviceInfo failed: {}", result.status_string());
      return result.status();
    }
    config = result->info;
  }

  // TODO: move this to SATA.
  if (config.sub_class != 0x06 && config.base_class == 0x01) {  // SATA
    fdf::error("Device class 0x{:x} unsupported", config.sub_class);
    return ZX_ERR_NOT_SUPPORTED;
  }

  // FIXME intel devices need to set SATA port enable at config + 0x92
  // ahci controller is bus master
  {
    const auto result = pci_->SetBusMastering(true);
    if (!result.ok()) {
      fdf::error("Call to SetBusMastering failed: {}", result.status_string());
      return result.status();
    }
    if (result->is_error()) {
      fdf::error("SetBusMastering failed: {}", zx_status_get_string(result->error_value()));
      return result->error_value();
    }
  }

  // Request 1 interrupt of any mode.
  {
    const auto result = pci_->GetInterruptModes();
    if (!result.ok()) {
      fdf::error("Call to GetInterruptModes failed: {}", result.status_string());
      return result.status();
    }
    if (result->modes.msix_count > 0) {
      irq_mode_ = fuchsia_hardware_pci::InterruptMode::kMsiX;
    } else if (result->modes.msi_count > 0) {
      irq_mode_ = fuchsia_hardware_pci::InterruptMode::kMsi;
    } else if (result->modes.has_legacy) {
      irq_mode_ = fuchsia_hardware_pci::InterruptMode::kLegacy;
    } else {
      fdf::error("No interrupt modes are supported.");
      return ZX_ERR_NOT_SUPPORTED;
    }
  }
  {
    const auto result = pci_->SetInterruptMode(irq_mode_, 1);
    if (!result.ok()) {
      fdf::error("Call to SetInterruptMode failed: {}", result.status_string());
      return result.status();
    }
    if (result->is_error()) {
      fdf::error("SetInterruptMode failed: {}", zx_status_get_string(result->error_value()));
      return result->error_value();
    }
  }

  // Get bti handle.
  {
    const auto result = pci_->GetBti(0);
    if (!result.ok()) {
      fdf::error("Call to GetBti failed: {}", result.status_string());
      return result.status();
    }
    if (result->is_error()) {
      fdf::error("GetBti failed: {}", zx_status_get_string(result->error_value()));
      return result->error_value();
    }
    bti_ = std::move(result->value()->bti);
  }

  // Get irq handle.
  {
    const auto result = pci_->MapInterrupt(0);
    if (!result.ok()) {
      fdf::error("Call to MapInterrupt failed: {}", result.status_string());
      return result.status();
    }
    if (result->is_error()) {
      fdf::error("MapInterrupt failed: {}", zx_status_get_string(result->error_value()));
      return result->error_value();
    }
    irq_ = std::move(result->value()->interrupt);
  }
  return ZX_OK;
}

zx_status_t PciBus::DmaBufferInit(std::unique_ptr<dma_buffer::ContiguousBuffer>* buffer_out,
                                  size_t size, zx_paddr_t* phys_out, void** virt_out) {
  // Allocate memory for the command list, FIS receive area, command table and PRDT.
  const size_t buffer_size = fbl::round_up(size, zx_system_get_page_size());
  auto buffer_factory = dma_buffer::CreateBufferFactory();
  zx_status_t status = buffer_factory->CreateContiguous(
      bti_, buffer_size, 0, dma_buffer::CacheOptions::kEnabled, buffer_out);
  if (status != ZX_OK) {
    return status;
  }
  *phys_out = (*buffer_out)->phys();
  *virt_out = (*buffer_out)->virt();
  return ZX_OK;
}

zx_status_t PciBus::BtiPin(uint32_t options, const zx::unowned_vmo& vmo, uint64_t offset,
                           uint64_t size, zx_paddr_t* addrs, size_t addrs_count, zx::pmt* pmt_out) {
  zx_handle_t pmt;
  zx_status_t status =
      zx_bti_pin(bti_.get(), options, vmo->get(), offset, size, addrs, addrs_count, &pmt);
  if (status == ZX_OK) {
    *pmt_out = zx::pmt(pmt);
  }
  return status;
}

zx_status_t PciBus::InterruptWait() {
  if (irq_mode_ == fuchsia_hardware_pci::InterruptMode::kLegacy) {
    const auto result = pci_->AckInterrupt();
    if (!result.ok()) {
      fdf::error("Call to AckInterrupt failed: {}", result.status_string());
      return result.status();
    }
    if (result->is_error()) {
      fdf::error("AckInterrupt failed: {}", zx_status_get_string(result->error_value()));
      return result->error_value();
    }
  }

  return irq_.wait(nullptr);
}

void PciBus::InterruptCancel() { irq_.destroy(); }

}  // namespace ahci
