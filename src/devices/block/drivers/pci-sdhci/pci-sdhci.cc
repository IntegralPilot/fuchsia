// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "pci-sdhci.h"

/// Exports the driver using the `FUCHSIA_DRIVER_EXPORT2` macro for `fdf::DriverBase2`.
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_offers.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/logging/cpp/logger.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/param.h>
#include <unistd.h>
#include <zircon/status.h>

#include <bind/fuchsia/cpp/bind.h>

#define HOST_CONTROL1_OFFSET 0x28
#define SDHCI_EMMC_HW_RESET (1 << 12)

namespace sdhci {

// PciSdhci::Start implements the DFv2 driver start hook.
// It connects to parent services and sets up child nodes.
zx::result<> PciSdhci::Start(fdf::DriverContext context) {
  auto pci_client_end = context.incoming().Connect<fuchsia_hardware_pci::Service::Device>();
  if (pci_client_end.is_error()) {
    FDF_LOG(ERROR, "Failed to connect to PCI: %s", pci_client_end.status_string());
    return pci_client_end.take_error();
  }
  pci_ = ddk::Pci(std::move(pci_client_end.value()));

  zx_status_t status = pci_.SetBusMastering(true);
  if (status != ZX_OK) {
    FDF_LOG(ERROR, "Failed to set bus mastering: %s", zx_status_get_string(status));
    return zx::error(status);
  }

  fuchsia_hardware_sdhci::Service::InstanceHandler handler({
      .device =
          bindings_.CreateHandler(this, driver_dispatcher()->get(), fidl::kIgnoreBindingClosure),
  });

  zx::result<> result = outgoing()->AddService<fuchsia_hardware_sdhci::Service>(std::move(handler));
  if (result.is_error()) {
    FDF_LOG(ERROR, "Failed to add sdhci fidl service: %s", result.status_string());
    return result.take_error();
  }

  // Offer the service to child nodes using `fdf::MakeOffer2`.
  std::vector<fuchsia_driver_framework::Offer> offers = {
      fdf::MakeOffer2<fuchsia_hardware_sdhci::Service>(),
  };

  // Add the child node using `NodeProperty2` and `MakeProperty2`.
  std::vector<fuchsia_driver_framework::NodeProperty2> properties = {
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.sdhci.Service"),
  };
  zx::result<fidl::ClientEnd<fuchsia_driver_framework::NodeController>> child_result =
      AddChild("pci-sdhci", properties, offers);
  if (child_result.is_error()) {
    FDF_LOG(ERROR, "Failed to add child node: %s", child_result.status_string());
    return child_result.take_error();
  }
  node_controller_ = std::move(child_result.value());

  return zx::ok();
}

void PciSdhci::GetInterrupt(fdf::Arena& arena, GetInterruptCompleter::Sync& completer) {
  // select irq mode
  fuchsia_hardware_pci::InterruptMode mode = fuchsia_hardware_pci::InterruptMode::kDisabled;
  zx_status_t status = pci_.ConfigureInterruptMode(1, &mode);
  if (status != ZX_OK) {
    FDF_LOG(ERROR, "Error setting irq mode: %s", zx_status_get_string(status));
    completer.buffer(arena).ReplyError(status);
    return;
  }

  // get irq handle
  zx::interrupt interrupt;
  status = pci_.MapInterrupt(0, &interrupt);
  if (status != ZX_OK) {
    FDF_LOG(ERROR, "Error getting irq handle: %s", zx_status_get_string(status));
    completer.buffer(arena).ReplyError(status);
    return;
  }

  completer.buffer(arena).ReplySuccess(std::move(interrupt));
}

void PciSdhci::GetSdhciMmio(fdf::Arena& arena, GetSdhciMmioCompleter::Sync& completer) {
  if (!mmio_.has_value()) {
    zx_status_t status = pci_.MapMmio(0u, ZX_CACHE_POLICY_UNCACHED_DEVICE, &mmio_);
    if (status != ZX_OK) {
      FDF_LOG(ERROR, "Error mapping register window: %s", zx_status_get_string(status));
      completer.buffer(arena).ReplyError(status);
      return;
    }
  }
  auto offset = mmio_->get_offset();
  zx::vmo vmo;
  zx_status_t status = mmio_->get_vmo()->duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo);
  if (status != ZX_OK) {
    completer.buffer(arena).ReplyError(status);
    return;
  }

  completer.buffer(arena).ReplySuccess(std::move(vmo), offset);
}

void PciSdhci::GetCqhciMmio(fdf::Arena& arena, GetCqhciMmioCompleter::Sync& completer) {
  completer.buffer(arena).ReplyError(ZX_ERR_NOT_SUPPORTED);
}

void PciSdhci::GetBti(GetBtiRequestView request, fdf::Arena& arena,
                      GetBtiCompleter::Sync& completer) {
  if (!bti_.is_valid()) {
    zx_status_t status = pci_.GetBti(request->index, &bti_);
    if (status != ZX_OK) {
      completer.buffer(arena).ReplyError(status);
      return;
    }
  }

  zx::bti bti;
  zx_status_t status = bti_.duplicate(ZX_RIGHT_SAME_RIGHTS, &bti);
  if (status != ZX_OK) {
    completer.buffer(arena).ReplyError(status);
    return;
  }

  completer.buffer(arena).ReplySuccess(std::move(bti));
}

void PciSdhci::GetBaseClock(fdf::Arena& arena, GetBaseClockCompleter::Sync& completer) {
  completer.buffer(arena).Reply(0);
}

void PciSdhci::GetQuirks(fdf::Arena& arena, GetQuirksCompleter::Sync& completer) {
  completer.buffer(arena).Reply(fuchsia_hardware_sdhci::Quirk::kStripResponseCrcPreserveOrder |
                                    fuchsia_hardware_sdhci::Quirk::kNoHs400EnhancedStrobe,
                                0);
}

void PciSdhci::HwReset(fdf::Arena& arena, HwResetCompleter::Sync& completer) {
  if (!mmio_.has_value()) {
    completer.buffer(arena).Reply();
    return;
  }
  uint32_t val = mmio_->Read32(HOST_CONTROL1_OFFSET);
  val |= SDHCI_EMMC_HW_RESET;
  mmio_->Write32(val, HOST_CONTROL1_OFFSET);
  // minimum is 1us but wait 9us for good measure
  zx_nanosleep(zx_deadline_after(ZX_USEC(9)));
  val &= ~SDHCI_EMMC_HW_RESET;
  mmio_->Write32(val, HOST_CONTROL1_OFFSET);
  // minimum is 200us but wait 300us for good measure
  zx_nanosleep(zx_deadline_after(ZX_USEC(300)));
  completer.buffer(arena).Reply();
}

void PciSdhci::VendorConfigureBus(VendorConfigureBusRequestView request, fdf::Arena& arena,
                                  VendorConfigureBusCompleter::Sync& completer) {
  completer.buffer(arena).ReplyError(ZX_ERR_STOP);
}

void PciSdhci::VendorPerformTuning(VendorPerformTuningRequestView request, fdf::Arena& arena,
                                   VendorPerformTuningCompleter::Sync& completer) {
  completer.buffer(arena).ReplyError(ZX_ERR_STOP);
}

}  // namespace sdhci

FUCHSIA_DRIVER_EXPORT2(sdhci::PciSdhci);
