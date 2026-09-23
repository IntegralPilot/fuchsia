// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_ML_DRIVERS_AML_NNA_AML_NNA_H_
#define SRC_DEVICES_ML_DRIVERS_AML_NNA_AML_NNA_H_

#include <fidl/fuchsia.hardware.registers/cpp/wire.h>
#include <fidl/fuchsia.hardware.vsi/cpp/wire.h>
#include <lib/driver/compat/cpp/device_server.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/mmio/cpp/mmio.h>
#include <lib/driver/platform-device/cpp/pdev.h>
#include <zircon/fidl.h>

#include <soc/aml-common/aml-power-domain.h>
#include <soc/aml-common/aml-registers.h>

constexpr uint32_t kNnaPowerDomainLegacy = 0;
constexpr uint32_t kNnaPowerDomain = 1;

namespace aml_nna {

class AmlNnaDriver : public fdf::DriverBase2,
                     public fidl::WireServer<fuchsia_hardware_vsi::PowerParent> {
 public:
  struct NnaPowerDomainBlock {
    // Power Domain MMIO.
    uint32_t domain_power_sleep_offset;
    uint32_t domain_power_iso_offset;
    // Set power state (1 = power off)
    uint32_t domain_power_sleep_bits;
    // Set control output signal isolation (1 = set isolation)
    uint32_t domain_power_iso_bits;

    // Memory PD MMIO.
    uint32_t hhi_mem_pd_reg0_offset;
    uint32_t hhi_mem_pd_reg1_offset;

    // Reset MMIO.
    uint32_t reset_level2_offset;
  };

  // Each offset is the byte offset of the register in their respective mmio region.
  struct NnaBlock {
    // For the new chips from Amlogic, smc already supports the control of power domain
    // So A5 uses smc to manage the NN power.
    uint32_t nna_power_version;
    union {
      struct NnaPowerDomainBlock nna_regs;  // Access with mmio read/write.
      uint32_t nna_domain_id;               // Access with smc call.
    };
    // Hiu MMIO.
    uint32_t clock_control_offset;
    uint32_t clock_core_control_bits;
    uint32_t clock_axi_control_bits;
  };

  static constexpr std::string_view kDriverName = "aml_nna";
  static constexpr std::string_view kChildNodeName = "aml-nna-child";
  static constexpr std::string_view kPlatformDeviceParentName = "pdev";
  static constexpr std::string_view kResetRegisterParentName = "register-reset";
  static constexpr uint32_t kHiuMmioIndex = 1;
  static constexpr uint32_t kPowerDomainMmioIndex = 2;
  static constexpr uint32_t kMemoryDomainMmioIndex = 3;

  explicit AmlNnaDriver() : fdf::DriverBase2(kDriverName) {}

  // fdf::DriverBase2 implementation.
  zx::result<> Start(fdf::DriverContext context) override;

  zx_status_t PowerDomainControl(bool turn_on);

 private:
  void Reset(ResetCompleter::Sync& completer) override;

  zx::result<> Reset();

  std::optional<fdf::MmioBuffer> hiu_mmio_;
  std::optional<fdf::MmioBuffer> power_mmio_;
  std::optional<fdf::MmioBuffer> memory_pd_mmio_;
  NnaBlock nna_block_;

  // Control PowerDomain
  zx::resource smc_monitor_;

  fidl::WireSyncClient<fuchsia_hardware_registers::Device> reset_register_;

  fidl::ClientEnd<fuchsia_driver_framework::NodeController> child_;
  compat::SyncInitializedDeviceServer compat_server_;
};

}  // namespace aml_nna

#endif  // SRC_DEVICES_ML_DRIVERS_AML_NNA_AML_NNA_H_
