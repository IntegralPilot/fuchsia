// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "pci-fidl.h"

#include <lib/driver/mmio/cpp/mmio-buffer.h>

#include <memory>

#include "zircon/errors.h"

namespace fpci = fuchsia_hardware_pci::wire;

struct iwl_pci_fidl {
  fidl::WireSyncClient<fuchsia_hardware_pci::Device> client_;
  std::optional<fdf::MmioBuffer> mmio;
};

void iwl_pci_ack_interrupt(const struct iwl_pci_fidl* fidl) {
  auto result = fidl->client_->AckInterrupt();
}

zx_status_t iwl_pci_read_config16(const struct iwl_pci_fidl* fidl, uint16_t offset,
                                  uint16_t* out_value) {
  auto result = fidl->client_->ReadConfig16(offset);
  if (!result.ok()) {
    return result.status();
  }
  if (result->is_error()) {
    return result->error_value();
  }
  *out_value = result->value()->value;
  return ZX_OK;
}

zx_status_t iwl_pci_get_device_info(const struct iwl_pci_fidl* fidl, pci_device_info_t* out_info) {
  auto result = fidl->client_->GetDeviceInfo();
  if (!result.ok()) {
    return result.status();
  }

  // Convert from fidl to internal struct
  auto info = result.value().info;
  out_info->vendor_id = info.vendor_id;
  out_info->device_id = info.device_id;
  out_info->base_class = info.base_class;
  out_info->sub_class = info.sub_class;
  out_info->program_interface = info.program_interface;
  out_info->revision_id = info.revision_id;
  out_info->bus_id = info.bus_id;
  out_info->dev_id = info.dev_id;
  out_info->func_id = info.func_id;

  return ZX_OK;
}

zx_status_t iwl_pci_get_bti(const struct iwl_pci_fidl* fidl, uint32_t index, zx_handle_t* out_bti) {
  auto result = fidl->client_->GetBti(index);
  if (!result.ok()) {
    return result.status();
  }
  if (result->is_error()) {
    return result->error_value();
  }
  *out_bti = result->value()->bti.release();
  return ZX_OK;
}

void iwl_pci_get_interrupt_modes(const struct iwl_pci_fidl* fidl,
                                 pci_interrupt_modes_t* out_modes) {
  auto result = fidl->client_->GetInterruptModes();
  if (!result.ok()) {
    return;
  }

  // Convert from fidl to internal struct
  auto modes = result.value().modes;
  out_modes->has_legacy = modes.has_legacy;
  out_modes->msi_count = modes.msi_count;
  out_modes->msix_count = modes.msix_count;
}

zx_status_t iwl_pci_set_interrupt_mode(const struct iwl_pci_fidl* fidl, pci_interrupt_mode_t mode,
                                       uint32_t requested_irq_count) {
  // Convert from internally defined types to FIDL types
  fpci::InterruptMode mode_;
  switch (mode) {
    case PCI_INTERRUPT_MODE_DISABLED:
      mode_ = fuchsia_hardware_pci::InterruptMode::kDisabled;
      break;
    case PCI_INTERRUPT_MODE_LEGACY:
      mode_ = fuchsia_hardware_pci::InterruptMode::kLegacy;
      break;
    case PCI_INTERRUPT_MODE_MSI:
      mode_ = fuchsia_hardware_pci::InterruptMode::kMsi;
      break;
    case PCI_INTERRUPT_MODE_MSI_X:
      mode_ = fuchsia_hardware_pci::InterruptMode::kMsiX;
      break;
    default:
      return ZX_ERR_INVALID_ARGS;
  }

  auto result = fidl->client_->SetInterruptMode(mode_, requested_irq_count);
  if (!result.ok()) {
    return result.status();
  }
  if (result->is_error()) {
    return result->error_value();
  }
  return ZX_OK;
}

zx_status_t iwl_pci_set_bus_mastering(const struct iwl_pci_fidl* fidl, bool enabled) {
  auto result = fidl->client_->SetBusMastering(enabled);
  if (!result.ok()) {
    return result.status();
  }
  if (result->is_error()) {
    return result->error_value();
  }
  return ZX_OK;
}

zx_status_t iwl_pci_map_interrupt(const struct iwl_pci_fidl* fidl, uint32_t which_irq,
                                  zx_handle_t* out_interrupt) {
  auto result = fidl->client_->MapInterrupt(which_irq);
  if (!result.ok()) {
    return result.status();
  }
  if (result->is_error()) {
    return result->error_value();
  }
  *out_interrupt = result->value()->interrupt.release();
  return ZX_OK;
}

zx_status_t iwl_pci_write_config8(const struct iwl_pci_fidl* fidl, uint16_t offset, uint8_t value) {
  auto result = fidl->client_->WriteConfig8(offset, value);
  if (!result.ok()) {
    return result.status();
  }
  if (result->is_error()) {
    return result->error_value();
  }
  return ZX_OK;
}

zx_status_t iwl_get_bar_internal(const struct iwl_pci_fidl* fidl, fidl::AnyArena& arena,
                                 uint32_t bar_id, fuchsia_hardware_pci::wire::Bar* out_result) {
  auto result = fidl->client_.buffer(arena)->GetBar(bar_id);
  if (!result.ok()) {
    return result.status();
  }
  if (result->is_error()) {
    return result->error_value();
  }
  *out_result = std::move(result->value()->result);
  if (out_result->result.is_io()) {
    zx_status_t status = zx_ioports_request(out_result->result.io().resource.get(),
                                            static_cast<uint16_t>(out_result->result.io().address),
                                            static_cast<uint32_t>(out_result->size));
    return status;
  } else {
    return ZX_OK;
  }
}

zx_status_t iwl_pci_map_mmio_internal(const struct iwl_pci_fidl* fidl, uint32_t index,
                                      uint32_t cache_policy, zx::vmo* out_vmo) {
  fidl::Arena arena;
  fuchsia_hardware_pci::wire::Bar bar;
  zx_status_t status = iwl_get_bar_internal(fidl, arena, index, &bar);
  if (status != ZX_OK) {
    return status;
  }

  if (bar.result.is_io()) {
    return ZX_ERR_WRONG_TYPE;
  }

  *out_vmo = std::move(bar.result.vmo());
  return ZX_OK;
}

zx_status_t iwl_pci_map_bar_buffer(struct iwl_pci_fidl* fidl, uint32_t index, uint32_t cache_policy,
                                   MMIO_PTR void** buffer) {
  zx::vmo vmo;
  zx_status_t status = iwl_pci_map_mmio_internal(fidl, index, cache_policy, &vmo);
  if (status != ZX_OK) {
    return status;
  }

  size_t vmo_size;
  status = vmo.get_size(&vmo_size);
  if (status != ZX_OK) {
    return status;
  }

  zx::result<fdf::MmioBuffer> mmio =
      fdf::MmioBuffer::Create(0, vmo_size, std::move(vmo), cache_policy);
  if (mmio.is_error()) {
    return mmio.error_value();
  }

  fidl->mmio = std::move(mmio.value());
  *buffer = fidl->mmio->get();
  return ZX_OK;
}

void iwl_pci_connect_fragment_protocol_with_client(
    fidl::ClientEnd<fuchsia_hardware_pci::Device> client_end, struct iwl_pci_fidl** fidl) {
  (*fidl) = new struct iwl_pci_fidl;
  (*fidl)->client_ = fidl::WireSyncClient(std::move(client_end));
}

void iwl_pci_free(struct iwl_pci_fidl* fidl) { delete fidl; }
