// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_PCI_SDHCI_PCI_SDHCI_H_
#define SRC_DEVICES_BLOCK_DRIVERS_PCI_SDHCI_PCI_SDHCI_H_

#include <fidl/fuchsia.hardware.sdhci/cpp/driver/fidl.h>
#include <lib/device-protocol/pci.h>
/// Under DFv2, we inherit from `fdf::DriverBase2`.
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/fidl_driver/cpp/server.h>

#include <optional>

namespace sdhci {

/// `PciSdhci` implements the SDHCI device protocol.
/// It inherits from `fdf::DriverBase2` which provides the DFv2 lifecycle.
class PciSdhci final : public fdf::DriverBase2,
                       public fdf::WireServer<fuchsia_hardware_sdhci::Device> {
 public:
  /// Constructor for default-constructible `fdf::DriverBase2`.
  PciSdhci() : fdf::DriverBase2("pci-sdhci") {}

  /// Starts the driver using the `DriverContext` which provides access to
  /// incoming services via `context.incoming()`.
  zx::result<> Start(fdf::DriverContext context) override;

  // fuchsia.hardware.sdhci/Device protocol implementation
  void GetInterrupt(fdf::Arena& arena, GetInterruptCompleter::Sync& completer) override;
  void GetSdhciMmio(fdf::Arena& arena, GetSdhciMmioCompleter::Sync& completer) override;
  void GetCqhciMmio(fdf::Arena& arena, GetCqhciMmioCompleter::Sync& completer) override;
  void GetBti(GetBtiRequestView request, fdf::Arena& arena,
              GetBtiCompleter::Sync& completer) override;
  void GetBaseClock(fdf::Arena& arena, GetBaseClockCompleter::Sync& completer) override;
  void GetQuirks(fdf::Arena& arena, GetQuirksCompleter::Sync& completer) override;
  void HwReset(fdf::Arena& arena, HwResetCompleter::Sync& completer) override;
  void VendorConfigureBus(VendorConfigureBusRequestView request, fdf::Arena& arena,
                          VendorConfigureBusCompleter::Sync& completer) override;
  void VendorPerformTuning(VendorPerformTuningRequestView request, fdf::Arena& arena,
                           VendorPerformTuningCompleter::Sync& completer) override;

 private:
  ddk::Pci pci_;

  std::optional<fdf::MmioBuffer> mmio_;
  zx::bti bti_;

  fidl::ClientEnd<fuchsia_driver_framework::NodeController> node_controller_;
  fdf::ServerBindingGroup<fuchsia_hardware_sdhci::Device> bindings_;
};

}  // namespace sdhci

#endif  // SRC_DEVICES_BLOCK_DRIVERS_PCI_SDHCI_PCI_SDHCI_H_
