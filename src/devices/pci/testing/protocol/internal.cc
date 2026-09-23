// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/pci/testing/protocol/internal.h"

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async-loop/loop.h>
#include <lib/async/cpp/task.h>
#include <lib/fake-resource/resource.h>
#include <lib/pci/constants.h>
#include <lib/stdcompat/bit.h>
#include <lib/sync/completion.h>
#include <zircon/syscalls/resource.h>

namespace pci {

namespace fpci = fuchsia_hardware_pci;

__EXPORT FakePciProtocolInternal::FakePciProtocolInternal() {
  // Reserve space ahead of time to ensure the vectors do not re-allocate so
  // references provided to callers stay valid.
  msi_interrupts_.reserve(MSI_MAX_VECTORS);
  msix_interrupts_.reserve(MSIX_MAX_VECTORS);
  reset();
}

zx_status_t FakePciProtocolInternal::FakePciProtocolInternal::PciGetBar(uint32_t bar_id,
                                                                        pci_bar_t* out_res) {
  if (!out_res) {
    return ZX_ERR_INVALID_ARGS;
  }

  if (bar_id >= PCI_DEVICE_BAR_COUNT) {
    return ZX_ERR_INVALID_ARGS;
  }

  auto& bar = bars_[bar_id];
  if (!bar.vmo.has_value()) {
    return ZX_ERR_NOT_FOUND;
  }

  out_res->bar_id = bar_id;
  out_res->size = bar.size;
  out_res->type = bar.type;
  if (bar.type == PCI_BAR_TYPE_MMIO) {
    out_res->result.vmo = bar.vmo->get();
  } else {
    out_res->result.io.address = 0;
    zx_handle_t fake_ioport_resource;
    fake_resource_create(ZX_RSRC_KIND_IOPORT, &fake_ioport_resource);
    char name[] = "fake IO";
    return zx_resource_create(fake_ioport_resource, ZX_RSRC_KIND_IOPORT, out_res->result.io.address,
                              out_res->size, name, sizeof(name), &out_res->result.io.resource);
  }
  return ZX_OK;
}

zx_status_t FakePciProtocolInternal::PciAckInterrupt() {
  return (irq_mode_ == fpci::InterruptMode::kLegacy) ? ZX_OK : ZX_ERR_BAD_STATE;
}

zx_status_t FakePciProtocolInternal::PciMapInterrupt(uint32_t which_irq,
                                                     zx::interrupt* out_handle) {
  if (!out_handle) {
    return ZX_ERR_INVALID_ARGS;
  }

  switch (irq_mode_) {
    case fpci::InterruptMode::kLegacy:
    case fpci::InterruptMode::kLegacyNoack:
      if (which_irq > 0) {
        return ZX_ERR_INVALID_ARGS;
      }
      return legacy_interrupt_->duplicate(ZX_RIGHT_SAME_RIGHTS, out_handle);
    case fpci::InterruptMode::kMsi:
      if (which_irq >= msi_interrupts_.size()) {
        return ZX_ERR_INVALID_ARGS;
      }
      return msi_interrupts_[which_irq].duplicate(ZX_RIGHT_SAME_RIGHTS, out_handle);
    case fpci::InterruptMode::kMsiX:
      if (which_irq >= msix_interrupts_.size()) {
        return ZX_ERR_INVALID_ARGS;
      }
      return msix_interrupts_[which_irq].duplicate(ZX_RIGHT_SAME_RIGHTS, out_handle);
    default:
      break;
  }

  return ZX_ERR_BAD_STATE;
}

void FakePciProtocolInternal::PciGetInterruptModes(fpci::wire::InterruptModes* out_modes) {
  fuchsia_hardware_pci::wire::InterruptModes modes{};
  if (legacy_interrupt_) {
    modes.has_legacy = true;
  }
  if (!msi_interrupts_.empty()) {
    // MSI interrupts are only supported in powers of 2.
    modes.msi_count = static_cast<uint8_t>((msi_interrupts_.size() <= 1)
                                               ? msi_interrupts_.size()
                                               : fbl::round_down(msi_interrupts_.size(), 2u));
  }
  if (!msix_interrupts_.empty()) {
    modes.msix_count = static_cast<uint16_t>(msix_interrupts_.size());
  }
  *out_modes = modes;
}

zx_status_t FakePciProtocolInternal::PciSetInterruptMode(fpci::InterruptMode mode,
                                                         uint32_t requested_irq_count) {
  if (!AllMappedInterruptsFreed()) {
    return ZX_ERR_BAD_STATE;
  }

  switch (mode) {
    case fpci::InterruptMode::kLegacy:
    case fpci::InterruptMode::kLegacyNoack:
      if (requested_irq_count > 1) {
        return ZX_ERR_INVALID_ARGS;
      }

      if (legacy_interrupt_) {
        irq_mode_ = mode;
        irq_cnt_ = 1;
      }
      return ZX_OK;
    case fpci::InterruptMode::kMsi:
      if (msi_interrupts_.empty()) {
        break;
      }
      if (!cpp20::has_single_bit(requested_irq_count) || requested_irq_count > MSI_MAX_VECTORS) {
        return ZX_ERR_INVALID_ARGS;
      }
      if (msi_interrupts_.size() < requested_irq_count) {
        return ZX_ERR_INVALID_ARGS;
      }
      irq_mode_ = fpci::InterruptMode::kMsi;
      irq_cnt_ = requested_irq_count;
      return ZX_OK;
    case fpci::InterruptMode::kMsiX:
      if (msix_interrupts_.empty()) {
        break;
      }
      if (requested_irq_count > MSIX_MAX_VECTORS) {
        return ZX_ERR_INVALID_ARGS;
      }

      if (msix_interrupts_.size() < requested_irq_count) {
        return ZX_ERR_INVALID_ARGS;
      }
      irq_mode_ = fpci::InterruptMode::kMsiX;
      irq_cnt_ = requested_irq_count;
      return ZX_OK;
    default:
      break;
  }

  return ZX_ERR_NOT_SUPPORTED;
}

zx_status_t FakePciProtocolInternal::PciSetBusMastering(bool enable) {
  bus_master_en_ = enable;
  return ZX_OK;
}

zx_status_t FakePciProtocolInternal::PciResetDevice() {
  reset_cnt_++;
  return ZX_OK;
}

zx_status_t FakePciProtocolInternal::PciGetDeviceInfo(pci_device_info_t* out_info) {
  ZX_ASSERT(out_info);
  out_info->vendor_id = info_.vendor_id;
  out_info->device_id = info_.device_id;
  out_info->base_class = info_.base_class;
  out_info->sub_class = info_.sub_class;
  out_info->program_interface = info_.program_interface;
  out_info->revision_id = info_.revision_id;
  out_info->bus_id = info_.bus_id;
  out_info->dev_id = info_.dev_id;
  out_info->func_id = info_.func_id;
  return ZX_OK;
}

zx_status_t FakePciProtocolInternal::PciGetFirstCapability(uint8_t id, uint8_t* out_offset) {
  return CommonCapabilitySearch(id, std::nullopt, out_offset);
}

zx_status_t FakePciProtocolInternal::PciGetNextCapability(uint8_t id, uint8_t offset,
                                                          uint8_t* out_offset) {
  return CommonCapabilitySearch(id, offset, out_offset);
}

zx_status_t FakePciProtocolInternal::PciGetFirstExtendedCapability(uint16_t id,
                                                                   uint16_t* out_offset) {
  return ZX_ERR_NOT_SUPPORTED;
}

zx_status_t FakePciProtocolInternal::PciGetNextExtendedCapability(uint16_t id, uint16_t offset,
                                                                  uint16_t* out_offset) {
  return ZX_ERR_NOT_SUPPORTED;
}

zx_status_t FakePciProtocolInternal::PciGetBti(uint32_t index, zx::bti* out_bti) {
  if (!out_bti) {
    return ZX_ERR_INVALID_ARGS;
  }
  return bti_.duplicate(ZX_RIGHT_SAME_RIGHTS, out_bti);
}

__EXPORT void FakePciProtocolInternal::AddCapabilityInternal(uint8_t capability_id,
                                                             uint8_t position, uint8_t size) {
  ZX_ASSERT_MSG(
      capability_id > 0 &&
          capability_id <= fidl::ToUnderlying(fpci::CapabilityId::kFlatteningPortalBridge),
      "FakePciProtocol Error: capability_id must be non-zero and <= %#x (capability_id = %#x).",
      fidl::ToUnderlying(fpci::CapabilityId::kFlatteningPortalBridge), capability_id);
  ZX_ASSERT_MSG(position >= PCI_CONFIG_HEADER_SIZE && position + size < pci::kBaseConfigSize,
                "FakePciProtocolError: capability must fit the range [%#x, %#x] (capability = "
                "[%#x, %#x]).",
                PCI_CONFIG_HEADER_SIZE, pci::kBaseConfigSize - 1, position, position + size - 1);

  // We need to update the next pointer of the previous capability, or the
  // original header capabilities pointer if this is the first.
  uint8_t next_ptr = fidl::ToUnderlying(fpci::Config::kCapabilitiesPtr);
  if (!capabilities().empty()) {
    for (auto& cap : capabilities()) {
      ZX_ASSERT_MSG(!(position <= cap.position && position + size > cap.position) &&
                        !(position >= cap.position && position < cap.position + cap.size),
                    "FakePciProtocol Error: New capability overlaps with a previous capability "
                    "[%#x, %#x] (new capability id = %#x @ [%#x, %#x]).",
                    cap.position, cap.position + cap.size - 1, capability_id, position,
                    position + size - 1);
    }
    next_ptr = capabilities()[capabilities().size() - 1].position + 1;
  }

  config().write(&capability_id, position, sizeof(capability_id));
  config().write(&position, next_ptr, sizeof(position));
  capabilities().push_back({.id = capability_id, .position = position, .size = size});
  // Not fast, but not as error prone as doing it by hand on insertion with
  // capability cyles being a possibility.
  std::sort(capabilities().begin(), capabilities().end());
}

__EXPORT void FakePciProtocolInternal::AddCapabilityInternal(
    fuchsia_hardware_pci::CapabilityId capability_id, uint8_t position, uint8_t size) {
  return AddCapabilityInternal(fidl::ToUnderlying(capability_id), position, size);
}

__EXPORT zx::interrupt& FakePciProtocolInternal::AddInterrupt(fpci::InterruptMode mode) {
  ZX_ASSERT_MSG(!(mode == fpci::InterruptMode::kLegacy && legacy_interrupt_),
                "FakePciProtocol Error: Legacy interrupt mode only supports 1 interrupt.");
  ZX_ASSERT_MSG(!(mode == fpci::InterruptMode::kMsi && msi_interrupts_.size() == MSI_MAX_VECTORS),
                "FakePciProtocol Error: MSI interrupt mode only supports up to %u interrupts.",
                MSI_MAX_VECTORS);

  zx::interrupt interrupt{};
  zx_status_t status = zx::interrupt::create(*zx::unowned_resource(ZX_HANDLE_INVALID), 0,
                                             ZX_INTERRUPT_VIRTUAL, &interrupt);
  ZX_ASSERT_MSG(status == ZX_OK, kFakePciInternalError);

  switch (mode) {
    case fpci::InterruptMode::kLegacy:
      legacy_interrupt_ = std::move(interrupt);
      return *legacy_interrupt_;
    case fpci::InterruptMode::kMsi:
      msi_interrupts_.insert(msi_interrupts_.end(), std::move(interrupt));
      msi_count_++;
      return msi_interrupts_[msi_count_ - 1];
    case fpci::InterruptMode::kMsiX:
      msix_interrupts_.insert(msix_interrupts_.end(), std::move(interrupt));
      msix_count_++;
      return msix_interrupts_[msix_count_ - 1];
    default:
      break;
  }

  ZX_PANIC("%s", kFakePciInternalError);
}

__EXPORT fpci::wire::DeviceInfo FakePciProtocolInternal::SetDeviceInfoInternal(
    fpci::wire::DeviceInfo new_info) {
  config().write(&new_info.vendor_id, fidl::ToUnderlying(fpci::Config::kVendorId),
                 sizeof(info().vendor_id));
  config().write(&new_info.device_id, fidl::ToUnderlying(fpci::Config::kDeviceId),
                 sizeof(info().device_id));
  config().write(&new_info.revision_id, fidl::ToUnderlying(fpci::Config::kRevisionId),
                 sizeof(info().revision_id));
  config().write(&new_info.base_class, fidl::ToUnderlying(fpci::Config::kClassCodeBase),
                 sizeof(info().base_class));
  config().write(&new_info.sub_class, fidl::ToUnderlying(fpci::Config::kClassCodeSub),
                 sizeof(info().sub_class));
  config().write(&new_info.program_interface, fidl::ToUnderlying(fpci::Config::kClassCodeIntr),
                 sizeof(info().program_interface));
  info_ = new_info;

  return new_info;
}

__EXPORT void FakePciProtocolInternal::reset() {
  legacy_interrupt_ = std::nullopt;
  msi_interrupts_.clear();
  msi_count_ = 0;
  msix_interrupts_.clear();
  msix_count_ = 0;
  irq_mode_ = fpci::InterruptMode::kDisabled;

  bars_ = {};
  capabilities_.clear();

  bus_master_en_ = std::nullopt;
  reset_cnt_ = 0;
  info() = {};

  zx_status_t status = zx::vmo::create(pci::kBaseConfigSize, /*options=*/0, &config());
  ZX_ASSERT(status == ZX_OK);
  status = fake_bti_create(bti_.reset_and_get_address());
  ZX_ASSERT(status == ZX_OK);
}

__EXPORT bool FakePciProtocolInternal::AllMappedInterruptsFreed() {
  zx_info_handle_count_t info;
  for (auto& interrupts : {&msix_interrupts_, &msi_interrupts_}) {
    for (auto& interrupt : *interrupts) {
      zx_status_t status =
          interrupt.get_info(ZX_INFO_HANDLE_COUNT, &info, sizeof(info), nullptr, nullptr);
      ZX_ASSERT_MSG(status == ZX_OK, "%s status %d", kFakePciInternalError, status);

      if (info.handle_count > 1) {
        return false;
      }
    }
  }
  return true;
}

__EXPORT zx_status_t FakePciProtocolInternal::CommonCapabilitySearch(uint8_t id,
                                                                     std::optional<uint8_t> offset,
                                                                     uint8_t* out_offset) {
  if (!out_offset) {
    return ZX_ERR_INVALID_ARGS;
  }

  for (auto& cap : capabilities_) {
    // Skip until we've caught up to last one found if one was provided.
    if (offset && cap.position <= offset) {
      continue;
    }

    if (cap.id == id) {
      *out_offset = cap.position;
      return ZX_OK;
    }
  }

  return ZX_ERR_NOT_FOUND;
}

void RunAsync(async::Loop& loop, fit::closure f) {
  sync_completion_t completion;
  async::PostTask(loop.dispatcher(), [&] {
    f();
    sync_completion_signal(&completion);
  });
  sync_completion_wait(&completion, ZX_TIME_INFINITE);
}

}  // namespace pci
