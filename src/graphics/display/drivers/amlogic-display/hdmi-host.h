// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_HDMI_HOST_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_HDMI_HOST_H_

#include <lib/driver/incoming/cpp/namespace.h>
#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/zx/result.h>
#include <zircon/types.h>

#include <cstdint>
#include <memory>

#include "src/graphics/display/drivers/amlogic-display/hdmi-transmitter.h"
#include "src/graphics/display/lib/api-types/cpp/display-timing.h"

namespace amlogic_display {

enum viu_type {
  VIU_ENCL = 0,
  VIU_ENCI,
  VIU_ENCP,
  VIU_ENCT,
};

struct pll_param {
  uint32_t mode;
  uint32_t viu_channel;
  uint32_t viu_type;

  int64_t hdmi_pll_vco_output_frequency_hz;
  int32_t output_divider1;
  int32_t output_divider2;
  int32_t output_divider3;

  // TODO(https://fxbug.dev/320616654): Support fractional divider ratios.
  int hdmi_clock_tree_vid_pll_divider;
  int32_t video_clock1_divider;
  int32_t hdmi_transmitter_pixel_clock_divider;
  int32_t encp_clock_divider;
  int32_t enci_clock_divider;
};

// 10-bit YCbCr value of a color in the Rec. ITU-R BT.709-6 color space.
struct YCbCrColor {
  int16_t y;
  int16_t cb;
  int16_t cr;
};

// HdmiHost has access to the amlogic/designware HDMI block and controls its
// operation. It also handles functions and keeps track of data that the
// amlogic/designware block does not need to know about, including clock
// calculations (which may move out of the host after https://fxbug.dev/42148166 is resolved),
// VPU and HHI register handling, HDMI parameters, etc.
class HdmiHost {
 public:
  // `hdmi_transmitter` must not be null.
  //
  // `vpu_mmio` is the VPU MMIO register region. It must be a valid MMIO buffer.
  //
  // `hhi_mmio` is the HHI (HIU) MMIO register region. It must be a valid MMIO
  // buffer.
  //
  // `gpio_mux_mmio` is the GPIO Multiplexer (PERIPHS_REGS) MMIO register
  // region. It must be a valid MMIO buffer.
  //
  // The VPU, HIU and PERIPHS_REGS register regions are defined in Section 8.1
  // "Memory Map" of the AMLogic A311D datasheet.
  HdmiHost(std::unique_ptr<HdmiTransmitter> hdmi_transmitter, fdf::MmioBuffer vpu_mmio,
           fdf::MmioBuffer hhi_mmio, fdf::MmioBuffer gpio_mux_mmio);

  HdmiHost(const HdmiHost&) = delete;
  HdmiHost(HdmiHost&&) = delete;
  HdmiHost& operator=(const HdmiHost&) = delete;
  HdmiHost& operator=(HdmiHost&&) = delete;

  static zx::result<std::unique_ptr<HdmiHost>> Create(fdf::Namespace& incoming);

  zx_status_t HostOn();
  void HostOff();

  // Configures the HDMI clock, encoder and physical layer to given `timing`.
  //
  // Returns ZX_OK and configures the video output module iff the display
  // timing and clock of `timing` is supported.
  zx_status_t ModeSet(const display::DisplayTiming& timing);

  zx::result<fbl::Vector<uint8_t>> ReadExtendedEdid();

  // Returns true iff a display timing is supported by the display engine driver
  // and can be used in a display configuration.
  bool IsDisplayTimingSupported(const display::DisplayTiming& timing) const;

  // Configures the video encoder to replace the output pixels of the Video
  // Input Unit (VIU) with the given `color` iff `enabled` is true.
  void ReplaceEncoderPixelColorWithColor(bool enabled, YCbCrColor color);

 private:
  void ConfigurePll(const pll_param& pll_params);
  void ConfigEncoder(const display::DisplayTiming& timings);
  void ConfigPhy();

  void ConfigureHpllClkOut(int64_t hdmi_pll_vco_output_frequency_hz);
  // TODO(https://fxbug.dev/320616654): Support fractional divider ratios.
  void ConfigureHdmiClockTree(int divider_ratio);
  void WaitForPllLocked();

  std::unique_ptr<HdmiTransmitter> hdmi_transmitter_;

  fdf::MmioBuffer vpu_mmio_;
  fdf::MmioBuffer hhi_mmio_;
  fdf::MmioBuffer gpio_mux_mmio_;
};

}  // namespace amlogic_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_HDMI_HOST_H_
