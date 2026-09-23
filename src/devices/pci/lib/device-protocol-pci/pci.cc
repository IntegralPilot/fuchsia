// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.pci/cpp/wire_types.h>
#include <lib/device-protocol/pci.h>
#include <lib/driver/mmio/cpp/mmio.h>

namespace ddk {

namespace fpci = fuchsia_hardware_pci;

zx_status_t Pci::GetDeviceInfo(fpci::wire::DeviceInfo* out_info) const {
  auto result = client_->GetDeviceInfo();
  if (!result.ok()) {
    return result.status();
  }
  *out_info = result.value().info;
  return ZX_OK;
}

zx_status_t Pci::GetBar(fidl::AnyArena& arena, uint32_t bar_id, fpci::wire::Bar* out_result) const {
  auto result = client_.buffer(arena)->GetBar(bar_id);
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

zx_status_t Pci::SetBusMastering(bool enabled) const {
  auto result = client_->SetBusMastering(enabled);
  if (!result.ok()) {
    return result.status();
  }
  if (result->is_error()) {
    return result->error_value();
  }
  return ZX_OK;
}

zx_status_t Pci::ResetDevice() const {
  auto result = client_->ResetDevice();
  if (!result.ok()) {
    return result.status();
  }
  if (result->is_error()) {
    return result->error_value();
  }
  return ZX_OK;
}

zx_status_t Pci::AckInterrupt() const {
  auto result = client_->AckInterrupt();
  if (!result.ok()) {
    return result.status();
  }
  if (result->is_error()) {
    return result->error_value();
  }
  return ZX_OK;
}

zx_status_t Pci::MapInterrupt(uint32_t which_irq, zx::interrupt* out_interrupt) const {
  auto result = client_->MapInterrupt(which_irq);
  if (!result.ok()) {
    return result.status();
  }
  if (result->is_error()) {
    return result->error_value();
  }
  *out_interrupt = std::move(result->value()->interrupt);
  return ZX_OK;
}

void Pci::GetInterruptModes(fpci::wire::InterruptModes* out_modes) const {
  auto result = client_->GetInterruptModes();
  if (!result.ok()) {
    return;
  }
  *out_modes = result.value().modes;
}

zx_status_t Pci::SetInterruptMode(fpci::InterruptMode mode, uint32_t requested_irq_count) const {
  auto result = client_->SetInterruptMode(mode, requested_irq_count);
  if (!result.ok()) {
    return result.status();
  }
  if (result->is_error()) {
    return result->error_value();
  }
  return ZX_OK;
}

zx_status_t Pci::ReadConfig8(uint16_t offset, uint8_t* out_value) const {
  auto result = client_->ReadConfig8(offset);
  if (!result.ok()) {
    return result.status();
  }
  if (result->is_error()) {
    return result->error_value();
  }
  *out_value = result->value()->value;
  return ZX_OK;
}

zx_status_t Pci::ReadConfig8(fpci::Config offset, uint8_t* out_value) const {
  return ReadConfig8(static_cast<uint16_t>(offset), out_value);
}

zx_status_t Pci::ReadConfig16(uint16_t offset, uint16_t* out_value) const {
  auto result = client_->ReadConfig16(offset);
  if (!result.ok()) {
    return result.status();
  }
  if (result->is_error()) {
    return result->error_value();
  }
  *out_value = result->value()->value;
  return ZX_OK;
}

zx_status_t Pci::ReadConfig16(fpci::Config offset, uint16_t* out_value) const {
  return ReadConfig16(static_cast<uint16_t>(offset), out_value);
}

zx_status_t Pci::ReadConfig32(uint16_t offset, uint32_t* out_value) const {
  auto result = client_->ReadConfig32(offset);
  if (!result.ok()) {
    return result.status();
  }
  if (result->is_error()) {
    return result->error_value();
  }
  *out_value = result->value()->value;
  return ZX_OK;
}

zx_status_t Pci::ReadConfig32(fpci::Config offset, uint32_t* out_value) const {
  return ReadConfig32(static_cast<uint16_t>(offset), out_value);
}

zx_status_t Pci::WriteConfig8(uint16_t offset, uint8_t value) const {
  auto result = client_->WriteConfig8(offset, value);
  if (!result.ok()) {
    return result.status();
  }
  if (result->is_error()) {
    return result->error_value();
  }
  return ZX_OK;
}

zx_status_t Pci::WriteConfig16(uint16_t offset, uint16_t value) const {
  auto result = client_->WriteConfig16(offset, value);
  if (!result.ok()) {
    return result.status();
  }
  if (result->is_error()) {
    return result->error_value();
  }
  return ZX_OK;
}

zx_status_t Pci::WriteConfig32(uint16_t offset, uint32_t value) const {
  auto result = client_->WriteConfig32(offset, value);
  if (!result.ok()) {
    return result.status();
  }
  if (result->is_error()) {
    return result->error_value();
  }
  return ZX_OK;
}

zx_status_t Pci::GetFirstCapability(fpci::CapabilityId id, uint8_t* out_offset) const {
  auto result = client_->GetCapabilities(id);
  if (!result.ok()) {
    return result.status();
  }

  if (result.value().offsets.size() == 0) {
    return ZX_ERR_NOT_FOUND;
  }

  *out_offset = result.value().offsets[0];
  return ZX_OK;
}

zx_status_t Pci::GetNextCapability(fpci::CapabilityId id, uint8_t start_offset,
                                   uint8_t* out_offset) const {
  auto result = client_->GetCapabilities(id);
  if (!result.ok()) {
    return result.status();
  }
  fidl::VectorView<uint8_t> offsets = result.value().offsets;
  for (uint64_t i = 0; i < offsets.size() - 1; i++) {
    if (offsets[i] == start_offset) {
      *out_offset = offsets[i + 1];
      return ZX_OK;
    }
  }
  return ZX_ERR_NOT_FOUND;
}

zx_status_t Pci::GetFirstExtendedCapability(fpci::ExtendedCapabilityId id,
                                            uint16_t* out_offset) const {
  auto result = client_->GetExtendedCapabilities(id);
  if (!result.ok()) {
    return result.status();
  }

  if (result.value().offsets.size() == 0) {
    return ZX_ERR_NOT_FOUND;
  }

  *out_offset = result.value().offsets[0];
  return ZX_OK;
}

zx_status_t Pci::GetNextExtendedCapability(fpci::ExtendedCapabilityId id, uint16_t start_offset,
                                           uint16_t* out_offset) const {
  auto result = client_->GetExtendedCapabilities(id);
  if (!result.ok()) {
    return result.status();
  }
  auto offsets = result.value().offsets;
  for (uint64_t i = 0; i < offsets.size() - 1; i++) {
    if (offsets[i] == start_offset) {
      *out_offset = offsets[i + 1];
      return ZX_OK;
    }
  }
  return ZX_ERR_NOT_FOUND;
}

zx_status_t Pci::GetBti(uint32_t index, zx::bti* out_bti) const {
  auto result = client_->GetBti(index);
  if (!result.ok()) {
    return result.status();
  }
  if (result->is_error()) {
    return result->error_value();
  }
  *out_bti = std::move(result->value()->bti);
  return ZX_OK;
}

zx_status_t Pci::MapMmio(uint32_t index, uint32_t cache_policy,
                         std::optional<fdf::MmioBuffer>* mmio) const {
  fidl::Arena arena;
  fpci::wire::Bar bar;
  zx_status_t status = GetBar(arena, index, &bar);
  if (status != ZX_OK) {
    return status;
  }

  if (!bar.result.is_vmo()) {
    return ZX_ERR_WRONG_TYPE;
  }
  size_t vmo_size;
  status = bar.result.vmo().get_size(&vmo_size);
  if (status != ZX_OK) {
    return status;
  }

  zx::result<fdf::MmioBuffer> result =
      fdf::MmioBuffer::Create(0, vmo_size, std::move(bar.result.vmo()), cache_policy);
  if (result.is_ok()) {
    *mmio = std::move(result.value());
  }
  return result.status_value();
}

zx_status_t Pci::ConfigureInterruptMode(uint32_t requested_irq_count,
                                        fpci::InterruptMode* out_mode) const {
  // NOTE: Any changes to this method should likely also be reflected in the C
  // version, pci_configure_interrupt_mode. These two implementations are
  // temporarily coexisting while we migrate PCI from Banjo to FIDL. Eventually
  // the C version will go away.
  //
  // TODO(https://fxbug.dev/42182407): Remove this notice once PCI over Banjo is removed.
  if (requested_irq_count == 0) {
    return ZX_ERR_INVALID_ARGS;
  }

  fpci::wire::InterruptModes modes;
  GetInterruptModes(&modes);
  std::pair<fpci::InterruptMode, uint32_t> pairs[] = {
      {fpci::InterruptMode::kMsiX, modes.msix_count},
      {fpci::InterruptMode::kMsi, modes.msi_count},
      {fpci::InterruptMode::kLegacy, modes.has_legacy}};
  for (auto& [mode, irq_cnt] : pairs) {
    if (irq_cnt >= requested_irq_count) {
      zx_status_t status = SetInterruptMode(mode, requested_irq_count);
      if (status == ZX_OK) {
        if (out_mode) {
          *out_mode = fpci::InterruptMode{mode};
        }
        return status;
      }
    }
  }
  return ZX_ERR_NOT_SUPPORTED;
}

pci_device_info_t convert_device_info_to_banjo(const fuchsia_hardware_pci::wire::DeviceInfo& info) {
  pci_device_info_t out_info{};
  out_info.vendor_id = info.vendor_id;
  out_info.device_id = info.device_id;
  out_info.base_class = info.base_class;
  out_info.sub_class = info.sub_class;
  out_info.program_interface = info.program_interface;
  out_info.revision_id = info.revision_id;
  out_info.bus_id = info.bus_id;
  out_info.dev_id = info.dev_id;
  out_info.func_id = info.func_id;
  return out_info;
}

pci_interrupt_modes_t convert_interrupt_modes_to_banjo(
    const fuchsia_hardware_pci::wire::InterruptModes& modes) {
  pci_interrupt_modes_t out_modes{};
  out_modes.has_legacy = modes.has_legacy;
  out_modes.msi_count = modes.msi_count;
  out_modes.msix_count = modes.msix_count;
  return out_modes;
}

pci_bar_t convert_bar_to_banjo(fuchsia_hardware_pci::wire::Bar bar) {
  pci_bar_t out_bar{};
  out_bar.bar_id = bar.bar_id;
  out_bar.size = bar.size;
  if (bar.result.is_io()) {
    out_bar.type = PCI_BAR_TYPE_IO;
    out_bar.result.io.address = bar.result.io().address;
    out_bar.result.io.resource = bar.result.io().resource.release();
  } else {
    out_bar.type = PCI_BAR_TYPE_MMIO;
    out_bar.result.vmo = bar.result.vmo().release();
  }
  return out_bar;
}

}  // namespace ddk
