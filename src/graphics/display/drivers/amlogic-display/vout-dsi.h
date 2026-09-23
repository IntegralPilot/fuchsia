// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_VOUT_DSI_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_VOUT_DSI_H_

#include <lib/device-protocol/display-panel.h>
#include <lib/driver/incoming/cpp/namespace.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/zx/result.h>

#include <memory>
#include <optional>

#include "src/graphics/display/drivers/amlogic-display/clock.h"
#include "src/graphics/display/drivers/amlogic-display/dsi-host.h"
#include "src/graphics/display/drivers/amlogic-display/panel-config.h"
#include "src/graphics/display/drivers/amlogic-display/vout.h"

namespace amlogic_display {

class VoutDsi : public Vout {
 public:
  static zx::result<std::unique_ptr<VoutDsi>> Create(fdf::Namespace& incoming,
                                                     display::PanelType panel_type,
                                                     inspect::Node node);

  static zx::result<std::unique_ptr<VoutDsi>> CreateForTesting(display::PanelType panel_type);

  VoutDsi(std::unique_ptr<DsiHost> dsi_host, std::unique_ptr<Clock> dsi_clock,
          const PanelConfig* panel_config, inspect::Node node);

  VoutType type() const override { return VoutType::kDsi; }
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
  std::unique_ptr<DsiHost> dsi_host_;
  std::unique_ptr<Clock> clock_;
  const PanelConfig& panel_config_;
  display::ModeAndId mode_and_id_;
};

}  // namespace amlogic_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_VOUT_DSI_H_
