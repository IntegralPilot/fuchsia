// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_CLOCK_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_CLOCK_H_

#include <fidl/fuchsia.hardware.platform.device/cpp/wire.h>
#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/zx/result.h>

#include <cstdint>

#include <hwreg/mmio.h>

#include "src/graphics/display/drivers/amlogic-display/dsi.h"
#include "src/graphics/display/drivers/amlogic-display/panel-config.h"
#include "src/graphics/display/lib/api-types/cpp/display-timing.h"

namespace amlogic_display {

class Clock {
 public:
  // Factory method intended for production use.
  //
  // `platform_device` must be valid.
  //
  // Creating a Clock instance doesn't change the hardware state, and is
  // therefore safe to use when adopting a bootloader initialized device.
  static zx::result<std::unique_ptr<Clock>> Create(
      fidl::UnownedClientEnd<fuchsia_hardware_platform_device::Device> platform_device,
      bool already_enabled);

  // Production code should prefer using the `Create()` factory method.
  //
  // `vpu_mmio` is the VPU MMIO register region. It must be a valid MMIO buffer.
  //
  // `hhi_mmio` is the HHI (HIU) MMIO register region. It must be a valid MMIO
  // buffer.
  //
  // The VPU and HIU register regions are defined in Section 8.1 "Memory Map"
  // of the AMLogic A311D datasheet.
  Clock(fdf::MmioBuffer vpu_mmio, fdf::MmioBuffer hhi_mmio, bool clock_enabled);

  zx::result<> Enable(const PanelConfig& panel_config);
  void Disable();

  void SetVideoOn(bool on);

  // This is only safe to call when the clock is Enable'd.
  int64_t GetBitrate() const {
    ZX_DEBUG_ASSERT(clock_enabled_);
    return pll_cfg_.dphy_data_lane_bits_per_second;
  }

  static LcdTiming CalculateLcdTiming(const display::DisplayTiming& display_timing);
  // This function calculates the required pll configurations needed to generate
  // the desired lcd clock
  static zx::result<HdmiPllConfigForMipiDsi> GenerateHPLL(
      int64_t pixel_clock_frequency_hz, int64_t maximum_per_data_lane_bit_per_second);

 private:
  zx::result<> WaitForHdmiPllToLock();

  fdf::MmioBuffer vpu_mmio_;
  fdf::MmioBuffer hhi_mmio_;

  HdmiPllConfigForMipiDsi pll_cfg_ = {};
  LcdTiming lcd_timing_ = {};

  bool clock_enabled_;
};

}  // namespace amlogic_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_CLOCK_H_
