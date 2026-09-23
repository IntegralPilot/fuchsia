// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_VOUT_HDMI_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_VOUT_HDMI_H_

#include <lib/driver/incoming/cpp/namespace.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/zx/result.h>

#include <memory>
#include <optional>

#include <fbl/vector.h>

#include "src/graphics/display/drivers/amlogic-display/hdmi-host.h"
#include "src/graphics/display/drivers/amlogic-display/vout.h"
#include "src/graphics/display/lib/api-types/cpp/display-timing.h"
#include "src/graphics/display/lib/api-types/cpp/mode-id.h"
#include "src/graphics/display/lib/edid/edid.h"

namespace amlogic_display {

class VoutHdmi : public Vout {
 public:
  static zx::result<std::unique_ptr<VoutHdmi>> Create(fdf::Namespace& incoming, inspect::Node node,
                                                      uint8_t visual_debug_level);

  VoutHdmi(std::unique_ptr<HdmiHost> hdmi_host, inspect::Node node, uint8_t visual_debug_level);

  VoutType type() const override { return VoutType::kHdmi; }
  bool SupportsHotplugDetection() const override;

  AddedDisplayInfo CreateAddedDisplayInfo(display::DisplayId display_id) override;
  zx::result<> UpdateStateOnDisplayConnected() override;
  void DisplayDisconnected() override;
  std::optional<display::Mode> GetDisplayMode(display::ModeId mode_id) const override;
  zx::result<> SubmitConfiguration(display::ModeId mode_id) override;
  zx::result<> PowerOff() override;
  zx::result<> PowerOn() override;
  zx::result<> SetFrameVisibility(bool frame_visible) override;
  void Dump() override;

 private:
  inspect::Node node_;
  std::unique_ptr<HdmiHost> hdmi_host_;
  std::optional<edid::Edid> edid_;
  fbl::Vector<display::DisplayTiming> timings_;
  display::ModeId current_mode_id_ = display::kInvalidModeId;
  uint8_t visual_debug_level_;
};

}  // namespace amlogic_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_VOUT_HDMI_H_
