// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_DESIGNWARE_HDMI_HDMI_TRANSMITTER_CONTROLLER_IMPL_H_
#define SRC_GRAPHICS_DISPLAY_LIB_DESIGNWARE_HDMI_HDMI_TRANSMITTER_CONTROLLER_IMPL_H_

#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/zx/result.h>

#include <cstdint>

#include <fbl/vector.h>

#include "src/graphics/display/lib/api-types/cpp/display-timing.h"
#include "src/graphics/display/lib/designware-hdmi/color-param.h"
#include "src/graphics/display/lib/designware-hdmi/hdmi-transmitter-controller.h"
#include "src/graphics/display/lib/edid/edid.h"

namespace designware_hdmi {

// The implementation of the DesignWare Cores HDMI 2.0 TX Controller (with
// HDCP).
class HdmiTransmitterControllerImpl : public HdmiTransmitterController {
 public:
  // `controller_mmio` is the MMIO region for the HDMI Transmitter Controller
  // registers, as defined in Section 6 "Register Descriptions" (pages 185-508)
  // of Synopsys DesignWare Cores HDMI Transmitter Controller Databook, v2.12a,
  // dated April 2016.
  explicit HdmiTransmitterControllerImpl(fdf::MmioBuffer controller_mmio)
      : controller_mmio_(std::move(controller_mmio)) {}
  ~HdmiTransmitterControllerImpl() override = default;

  zx_status_t InitHw() override;

  zx::result<fbl::Vector<uint8_t>> ReadExtendedEdid() override;

  void ConfigHdmitx(const ColorParam& color_param, const display::DisplayTiming& mode,
                    const hdmi_param_tx& p) override;
  void SetupInterrupts() override;
  void Reset() override;
  void SetupScdc(bool is4k) override;
  void ResetFc() override;
  void SetFcScramblerCtrl(bool is4k) override;

  void PrintRegisters() override;

 private:
  static constexpr int kMaxAttemptCountForPollForDdcCommandDone = 5;

  void WriteReg(uint32_t addr, uint8_t data) { controller_mmio_.Write8(data, addr); }
  uint8_t ReadReg(uint32_t addr) { return controller_mmio_.Read8(addr); }

  void PrintReg(const char* name, uint32_t address);

  void ScdcWrite(uint8_t addr, uint8_t val);
  uint8_t ScdcRead(uint8_t addr);

  // `index` must be non-negative and less than `edid::kMaxEdidBlockCount`.
  zx::result<> ReadEdidBlock(int index, std::span<uint8_t, edid::kBlockSize> edid_block);

  // Returns true and clears the DDC controller interrupt iff the DDC command
  // is done within the `kMaxAttemptCountForPollForDdcCommandDone` polls.
  bool PollForDdcCommandDone();

  void ConfigCsc(const ColorParam& color_param);

  fdf::MmioBuffer controller_mmio_;
};

}  // namespace designware_hdmi

#endif  // SRC_GRAPHICS_DISPLAY_LIB_DESIGNWARE_HDMI_HDMI_TRANSMITTER_CONTROLLER_IMPL_H_
