// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_HDMI_TRANSMITTER_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_HDMI_TRANSMITTER_H_

#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/zx/resource.h>
#include <lib/zx/result.h>
#include <zircon/compiler.h>

#include <cstdint>
#include <memory>

#include <fbl/mutex.h>
#include <fbl/vector.h>

#include "src/graphics/display/lib/api-types/cpp/display-timing.h"
#include "src/graphics/display/lib/designware-hdmi/color-param.h"
#include "src/graphics/display/lib/designware-hdmi/hdmi-transmitter-controller.h"

namespace amlogic_display {

// The top-level integration logic of the HDMI transmitter in the Amlogic
// display engine. It coordinates the top-level logic (TOP), the Synopsys
// Designware Core HDMI Controller IP and the HDMI physical layer (PHY).
class HdmiTransmitter {
 public:
  // `designware_controller` must not be null.
  //
  // `hdmitx_top_level_mmio` is the top-level register sub-region of the HDMITX
  // MMIO register region.
  //
  // The HDMITX register region is defined in Section 8.1 "Memory Map" of
  // the AMLogic A311D datasheet. The sub-region is defined in Section
  // 10.2.3.43 "HDMITX Top-Level and HDMI TX Controller IP Register Access" of
  // the AMLogic A311D datasheet.
  //
  // `hdmitx_top_level_mmio` must be a valid MMIO buffer.
  //
  // `silicon_provider_service_smc` is the secure monitor call (SMC) resource
  // for the silicon-provider service calls. It must be valid unless
  // `HdmiTransmitter` is used for tests.
  //
  // TODO(https://fxbug.dev/42074342): Currently fake SMC resource objects are not yet
  // supported. Once fake SMC is supported, we should enforce
  // `silicon_provider_service_smc`  to be always valid.
  HdmiTransmitter(std::unique_ptr<designware_hdmi::HdmiTransmitterController> designware_controller,
                  fdf::MmioBuffer hdmitx_top_level_mmio, zx::resource silicon_provider_service_smc);

  ~HdmiTransmitter() = default;

  HdmiTransmitter(const HdmiTransmitter&) = delete;
  HdmiTransmitter(HdmiTransmitter&&) = delete;
  HdmiTransmitter& operator=(const HdmiTransmitter&) = delete;
  HdmiTransmitter& operator=(HdmiTransmitter&&) = delete;

  zx::result<> Reset();
  zx::result<> ModeSet(const display::DisplayTiming& timing,
                       const designware_hdmi::ColorParam& color);

  zx::result<fbl::Vector<uint8_t>> ReadExtendedEdid();

  void PrintRegisters();

 private:
  void WriteTopLevelReg(uint32_t addr, uint32_t val);
  uint32_t ReadTopLevelReg(uint32_t addr);

  void PrintRegister(const char* register_name, uint32_t register_address);
  void PrintTopLevelRegisters();

  // Issues a secure monitor call to ask the secure monitor to initialize
  // HDCP 1.4 engine.
  zx::result<> InitializeHdcp14();

  fbl::Mutex dw_lock_;
  std::unique_ptr<designware_hdmi::HdmiTransmitterController> designware_controller_
      __TA_GUARDED(dw_lock_);

  fdf::MmioBuffer hdmitx_top_level_mmio_;

  zx::resource silicon_provider_service_smc_;
};

}  // namespace amlogic_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_HDMI_TRANSMITTER_H_
