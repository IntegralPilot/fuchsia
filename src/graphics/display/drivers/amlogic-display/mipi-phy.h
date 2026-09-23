// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_MIPI_PHY_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_MIPI_PHY_H_

#include <fidl/fuchsia.hardware.platform.device/cpp/wire.h>
#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/zx/result.h>

#include <cstdint>

#include "src/graphics/display/lib/designware-dsi/dsi-host-controller.h"

namespace amlogic_display {

class MipiPhy {
 public:
  // Factory method intended for production use.
  //
  // `platform_device` must be valid.
  //
  // `designware_dsi_host_controller` must be non-null and outlive the `MipiPhy`
  // instance.
  //
  // `enabled` is true iff the driver adopts an already initialized MIPI D-PHY
  // controller.
  //
  // Creating an MipiPhy instance doesn't change the hardware state, and is
  // therefore safe to use when adopting a device previously initialized by
  // the bootloader or another driver.
  static zx::result<std::unique_ptr<MipiPhy>> Create(
      fidl::UnownedClientEnd<fuchsia_hardware_platform_device::Device> platform_device,
      designware_dsi::DsiHostController* designware_dsi_host_controller, bool enabled);

  // Production code should prefer using the `Create()` factory method.
  //
  // `d_phy_mmio` is the MIPI D-PHY controller register MMIO region (also known
  // as "MIPI_DSI_PHY" in the "Memory Map" Section of Amlogic datasheets).
  // It must be a valid MMIO buffer.
  //
  // `designware_dsi_host_controller` must be non-null and outlive the `MipiPhy`
  // instance.
  //
  // `dsiimpl` must be valid.
  explicit MipiPhy(fdf::MmioBuffer d_phy_mmio,
                   designware_dsi::DsiHostController* designware_dsi_host_controller, bool enabled);

  MipiPhy(const MipiPhy&) = delete;
  MipiPhy& operator=(const MipiPhy&) = delete;

  // This function enables and starts up the Mipi Phy
  zx::result<> Startup();
  // This function stops Mipi Phy
  void Shutdown();
  zx::result<> PhyCfgLoad(int64_t dphy_data_lane_bits_per_second);
  void Dump();
  uint32_t GetDataLaneByteRateToEscapeClockFrequencyRatio() const { return dsi_phy_cfg_.lp_tesc; }

 private:
  // This structure holds the timing parameters used for MIPI D-PHY
  // This can be moved later on to MIPI D-PHY specific header if need be
  struct DsiPhyConfig {
    uint32_t lp_tesc;
    uint32_t lp_lpx;
    uint32_t lp_ta_sure;
    uint32_t lp_ta_go;
    uint32_t lp_ta_get;
    uint32_t hs_exit;
    uint32_t hs_trail;
    uint32_t hs_zero;
    uint32_t hs_prepare;
    uint32_t clk_trail;
    uint32_t clk_post;
    uint32_t clk_zero;
    uint32_t clk_prepare;
    uint32_t clk_pre;
    uint32_t init;
    uint32_t wakeup;
  };

  void PhyInit();

  fdf::MmioBuffer dsi_phy_mmio_;
  DsiPhyConfig dsi_phy_cfg_;
  designware_dsi::DsiHostController& designware_dsi_host_controller_;

  bool phy_enabled_ = false;
};

}  // namespace amlogic_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_MIPI_PHY_H_
