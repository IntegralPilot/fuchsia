// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/mmio/cpp/mmio.h>
#include <lib/pci/constants.h>
#include <lib/pci/hw.h>
#include <lib/stdcompat/bit.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/status.h>

#include <fbl/string_buffer.h>
#include <fbl/string_printf.h>

#include "src/devices/pci/drivers/pci/capabilities/msi.h"
#include "src/devices/pci/drivers/pci/device.h"

namespace pci {

namespace fpci = fuchsia_hardware_pci;

zx::result<uint32_t> Device::QueryIrqMode(fpci::InterruptMode mode) {
  const fbl::AutoLock dev_lock(&dev_lock_);
  switch (mode) {
    case fpci::InterruptMode::kLegacy:
      if (cfg_->Read(Config::kInterruptLine) != 0) {
        return zx::ok(kLegacyInterruptCount);
      }
      break;
    case fpci::InterruptMode::kMsi:
      if (caps_.msi) {
        return zx::ok(caps_.msi->vectors_avail());
      }
      break;
    case fpci::InterruptMode::kMsiX:
      if (caps_.msix) {
        return zx::ok(caps_.msix->table_size());
      }
      break;
    case fpci::InterruptMode::kDisabled:
    case fpci::InterruptMode::kLegacyNoack:
    default:
      return zx::error(ZX_ERR_INVALID_ARGS);
  }
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

pci_interrupt_modes_t Device::GetInterruptModes() {
  const fbl::AutoLock dev_lock(&dev_lock_);
  pci_interrupt_modes_t modes{};

  if (cfg_->Read(Config::kInterruptLine) != 0) {
    modes.has_legacy = true;
  }
  if (caps_.msi) {
    modes.msi_count = caps_.msi->vectors_avail();
  }
  if (caps_.msix) {
    modes.msix_count = caps_.msix->table_size();
  }
  return modes;
}

zx_status_t Device::SetIrqMode(fpci::InterruptMode mode, uint32_t irq_cnt) {
  if (mode == fpci::InterruptMode::kLegacyNoack) {
    return ZX_ERR_INVALID_ARGS;
  }
  if (mode != fpci::InterruptMode::kDisabled && irq_cnt == 0) {
    return ZX_ERR_INVALID_ARGS;
  }

  const fbl::AutoLock dev_lock(&dev_lock_);
  // Before enabling any given interrupt mode we need to ensure no existing
  // interrupts are configured. Disabling them can fail in cases downstream
  // drivers have not freed outstanding interrupt objects allocated off of
  // an MSI object.
  zx_status_t status = DisableInterrupts();
  if (status != ZX_OK) {
    return status;
  }

  status = ZX_ERR_NOT_SUPPORTED;
  switch (mode) {
    case fpci::InterruptMode::kDisabled:
      status = ZX_OK;
      break;
    case fpci::InterruptMode::kLegacy:
      status = EnableLegacy();
      break;
    case fpci::InterruptMode::kMsi:
      if (caps_.msi) {
        status = EnableMsi(irq_cnt);
      }
      break;
    case fpci::InterruptMode::kMsiX:
      if (caps_.msix) {
        status = EnableMsix(irq_cnt);
      }
      break;
    default:
      break;
  }

  return status;
}

zx_status_t Device::DisableInterrupts() {
  zx_status_t st = ZX_OK;
  switch (irqs_.mode) {
    case fpci::InterruptMode::kDisabled:
      return ZX_OK;
    case fpci::InterruptMode::kLegacy:
      st = DisableLegacy();
      break;
    case fpci::InterruptMode::kMsi:
      st = DisableMsi();
      break;
    case fpci::InterruptMode::kMsiX:
      st = DisableMsix();
      break;
    default:
      break;
  }

  if (st == ZX_OK) {
    zxlogf(DEBUG, "[%s] disabled IRQ mode %u", cfg_->addr(), fidl::ToUnderlying(irqs_.mode));
    irqs_.mode = fpci::InterruptMode::kDisabled;
  }
  return st;
}

zx::result<zx::interrupt> Device::MapInterrupt(uint32_t which_irq) {
  const fbl::AutoLock dev_lock(&dev_lock_);
  // MSI support is controlled through the capability held within the device's configuration space,
  // so the dispatcher needs access to the given device's config vmo. MSI-X needs access to the
  // table structure which is held in one of the device BARs, but a view is built ahead of time for
  // it when the MSI-X capability is initialized.
  if (irqs_.mode == fpci::InterruptMode::kDisabled) {
    return zx::error(ZX_ERR_BAD_STATE);
  }

  zx::interrupt interrupt = {};
  zx_status_t status = ZX_OK;
  switch (irqs_.mode) {
    case fpci::InterruptMode::kLegacy: {
      if (which_irq != 0) {
        return zx::error(ZX_ERR_INVALID_ARGS);
      }
      status = irqs_.legacy.duplicate(ZX_RIGHT_SAME_RIGHTS, &interrupt);
      break;
    }
    case fpci::InterruptMode::kMsi: {
      zx::unowned_vmo cfg_vmo;
      uint64_t cfg_offset = 0;

      if (strcmp(cfg_->type(), "mmio") == 0) {
        zx::result<fdf::MmioView> view_res = cfg_->get_view();
        if (!view_res.is_ok()) {
          return view_res.take_error();
        }
        cfg_vmo = view_res->get_vmo();
        cfg_offset = view_res->get_offset() + caps_.msi->base();
      }

      status = bdi_->GetMsiHandle(irqs_.msi_allocation, /*options=*/0, which_irq, *cfg_vmo,
                                  cfg_offset, &interrupt);
      break;
    }
    case fpci::InterruptMode::kMsiX: {
      auto& msix = caps_.msix;
      status = bdi_->GetMsiHandle(irqs_.msi_allocation, ZX_MSI_MODE_MSI_X, which_irq,
                                  *msix->table_vmo(), msix->table_offset(), &interrupt);
      // Disable the function level masking now that at least one interrupt exists for the device.
      if (status == ZX_OK) {
        MsixControlReg ctrl = {.value = cfg_->Read(caps_.msix->ctrl())};
        ctrl.set_function_mask(0);
        cfg_->Write(caps_.msix->ctrl(), ctrl.value);
      }
      break;
    }
    default:
      return zx::error(ZX_ERR_BAD_STATE);
  }

  if (status != ZX_OK) {
    return zx::error(status);
  }

  return zx::ok(std::move(interrupt));
}

zx_status_t Device::SignalLegacyIrq(zx_instant_boot_t timestamp) {
  return irqs_.legacy.trigger(/*options=*/0, zx::time_boot(timestamp));
}

zx_status_t Device::AckLegacyIrq() {
  if (irqs_.mode != fpci::InterruptMode::kLegacy) {
    return ZX_ERR_BAD_STATE;
  }

  // TODO(544749672): AckLegacyIrq is a no-op until fully removed.
  // Unmasking is handled automatically by the Bus monitoring
  // ZX_VIRTUAL_INTERRUPT_UNTRIGGERED.
  return ZX_OK;
}

void Device::EnableLegacyIrq() {
  ModifyCmdLocked(/*clr_bits=*/pci::kCommandIntDisable, /*set_bits=*/0);
  irqs_.legacy_disabled = false;
}

void Device::DisableLegacyIrq() {
  ModifyCmdLocked(/*clr_bits=*/0, /*set_bits=*/pci::kCommandIntDisable);
  irqs_.legacy_disabled = true;
}

zx::result<std::pair<zx::msi, msi_allocation_info_t>> Device::AllocateMsi(uint32_t irq_cnt) {
  zx::msi msi;
  msi_allocation_info_t info{};
  zx_status_t st = bdi_->AllocateMsi(irq_cnt, &msi, &info);
  if (st != ZX_OK) {
    return zx::error(st);
  }
  ZX_DEBUG_ASSERT(info.irq_count == irq_cnt);
  zxlogf(DEBUG, "[%s] allocated MSI range [%#x, %#x)", cfg_->addr(), info.target_data,
         info.target_data + info.irq_count);

  return zx::ok(std::make_pair(std::move(msi), info));
}

zx_status_t Device::EnableLegacy() {
  irqs_.legacy_vector = cfg_->Read(Config::kInterruptLine);
  if (irqs_.legacy_vector == 0) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  const zx_status_t status =
      bdi_->AddToSharedIrqList(this, irqs_.legacy_vector, irqs_.legacy.borrow());
  if (status != ZX_OK) {
    zxlogf(ERROR, "[%s] failed to add legacy irq to shared handler list %#x: %s", cfg_->addr(),
           irqs_.legacy_vector, zx_status_get_string(status));
    return status;
  }

  ModifyCmdLocked(/*clr_bits=*/pci::kCommandIntDisable, /*set_bits=*/0);
  irqs_.legacy_disabled = false;
  irqs_.mode = fpci::InterruptMode::kLegacy;
  irqs_.legacy_pin = cfg_->Read(Config::kInterruptPin);
  return ZX_OK;
}

zx_status_t Device::EnableMsi(uint32_t irq_cnt) {
  ZX_DEBUG_ASSERT(irqs_.mode == fpci::InterruptMode::kDisabled);
  ZX_DEBUG_ASSERT(!irqs_.msi_allocation);
  ZX_DEBUG_ASSERT(caps_.msi);

  if (!cpp20::has_single_bit(irq_cnt) || irq_cnt > caps_.msi->vectors_avail()) {
    zxlogf(DEBUG, "[%s] EnableMsi: bad irq count = %u, available = %u\n", cfg_->addr(), irq_cnt,
           caps_.msi->vectors_avail());
    return ZX_ERR_INVALID_ARGS;
  }

  // Bus mastering must be enabled to generate MSI messages.
  const zx_status_t status = SetBusMastering(true);
  if (status != ZX_OK) {
    zxlogf(ERROR, "[%s] Failed to enable bus mastering for MSI mode (%d)", cfg_->addr(), status);
    return status;
  }

  auto result = AllocateMsi(irq_cnt);
  if (result.is_ok()) {
    auto [alloc, info] = std::move(result.value());
    MsiControlReg ctrl = {.value = cfg_->Read(caps_.msi->ctrl())};
    cfg_->Write(caps_.msi->tgt_addr(), info.target_addr);
    cfg_->Write(caps_.msi->tgt_data(), info.target_data);
    if (ctrl.mm_capable()) {
      ctrl.set_mm_enable(MsiCapability::CountToMmc(irq_cnt));
    }
    ctrl.set_enable(1);
    cfg_->Write(caps_.msi->ctrl(), ctrl.value);

    irqs_.msi_allocation = std::move(alloc);
    irqs_.mode = fpci::InterruptMode::kMsi;
  }
  return result.status_value();
}

zx_status_t Device::EnableMsix(uint32_t irq_cnt) {
  ZX_DEBUG_ASSERT(irqs_.mode == fpci::InterruptMode::kDisabled);
  ZX_DEBUG_ASSERT(!irqs_.msi_allocation);
  ZX_DEBUG_ASSERT(caps_.msix);

  // Bus mastering must be enabled to generate MSI-X messages.
  const zx_status_t status = SetBusMastering(true);
  if (status != ZX_OK) {
    zxlogf(ERROR, "[%s] Failed to enable bus mastering for MSI-X mode (%d)", cfg_->addr(), status);
    return status;
  }

  // MSI-X supports non-pow2 counts, but the MSI allocator still allocates in
  // pow2 based blocks.
  const uint32_t irq_cnt_pow2 = cpp20::bit_ceil(irq_cnt);
  auto result = AllocateMsi(irq_cnt_pow2);
  if (result.is_ok()) {
    auto [alloc, info] = std::move(result.value());
    // Enable MSI-X, but mask off all functions until an interrupt is mapped.
    MsixControlReg ctrl = {.value = cfg_->Read(caps_.msix->ctrl())};
    ctrl.set_function_mask(1);
    ctrl.set_enable(1);
    cfg_->Write(caps_.msix->ctrl(), ctrl.value);

    irqs_.msi_allocation = std::move(alloc);
    irqs_.mode = fpci::InterruptMode::kMsiX;
  }
  return result.status_value();
}

zx_status_t Device::DisableLegacy() {
  ModifyCmdLocked(/*clr_bits=*/0, /*set_bits=*/pci::kCommandIntDisable);
  irqs_.legacy_disabled = true;

  const zx_status_t status = bdi_->RemoveFromSharedIrqList(this, irqs_.legacy_vector);
  if (status != ZX_OK) {
    zxlogf(ERROR, "[%s] failed to remove legacy irq to shared handler list %#x: %s", cfg_->addr(),
           irqs_.legacy_vector, zx_status_get_string(status));
    return status;
  }

  irqs_.legacy_vector = 0;
  return ZX_OK;
}

// In general, if a device driver tries to disable an interrupt mode while
// holding handles to individual interrupts then it's considered a bad state.
// TODO(https://fxbug.dev/42108122): Are there cases where the bus driver would want to hard disable
// IRQs even though the driver holds outstanding handles? In the event of a driver
// crash the handles will be released, but in a hard disable path they would still
// exist.
zx_status_t Device::VerifyAllMsisFreed() {
  if (!irqs_.msi_allocation) {
    return ZX_OK;
  }

  zx_info_msi_t info = {};
  const zx_status_t st =
      irqs_.msi_allocation.get_info(ZX_INFO_MSI, &info, sizeof(info), nullptr, nullptr);
  if (st != ZX_OK) {
    return st;
  }

  if (info.interrupt_count != 0) {
    return ZX_ERR_BAD_STATE;
  }

  return ZX_OK;
}

void Device::DisableMsiCommon() { irqs_.msi_allocation.reset(); }

zx_status_t Device::DisableMsi() {
  ZX_DEBUG_ASSERT(caps_.msi);
  if (const zx_status_t st = VerifyAllMsisFreed(); st != ZX_OK) {
    return st;
  }

  MsiControlReg ctrl = {.value = cfg_->Read(caps_.msi->ctrl())};
  ctrl.set_enable(0);
  cfg_->Write(caps_.msi->ctrl(), ctrl.value);

  DisableMsiCommon();
  return ZX_OK;
}

zx_status_t Device::DisableMsix() {
  ZX_DEBUG_ASSERT(caps_.msix);
  if (const zx_status_t st = VerifyAllMsisFreed(); st != ZX_OK) {
    return st;
  }

  MsixControlReg ctrl = {.value = cfg_->Read(caps_.msix->ctrl())};
  ctrl.set_function_mask(1);
  ctrl.set_enable(0);
  cfg_->Write(caps_.msix->ctrl(), ctrl.value);

  irqs_.msi_allocation.reset();
  return ZX_OK;
}

}  // namespace pci
