// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_LCD_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_LCD_H_

#include <fidl/fuchsia.hardware.gpio/cpp/wire.h>
#include <lib/device-protocol/display-panel.h>
#include <lib/driver/incoming/cpp/namespace.h>
#include <lib/zx/result.h>

#include <cstdint>
#include <memory>

#include "src/graphics/display/drivers/amlogic-display/panel-config.h"
#include "src/graphics/display/lib/designware-dsi/dsi-host-controller.h"

namespace amlogic_display {

// An Lcd controls the panel attached to a MIPI-DSI endpoint.
class Lcd {
 public:
  // Factory method intended for production use.
  //
  // `panel_config` must not be null and must outlive the `Lcd` instance.
  //
  // `designware_dsi_host_controller` must not be null and must outlive the
  // `Lcd` instance.
  //
  // `enabled` is true iff the driver adopts an already initialized panel.
  //
  // Creating an Lcd instance doesn't change the hardware state, and is
  // therefore safe to use when adopting a device previously initialized by
  // the bootloader or another driver.
  static zx::result<std::unique_ptr<Lcd>> Create(
      fdf::Namespace& incoming, display::PanelType panel_type, const PanelConfig* panel_config,
      designware_dsi::DsiHostController* designware_dsi_host_controller, bool enabled);

  // Production code should prefer using the `Create()` factory method.
  //
  // `panel_config` must not be null and must outlive the `Lcd` instance.
  //
  // `designware_dsi_host_controller` must not be null and must outlive the
  // `Lcd` instance.
  //
  // `lcd_reset_gpio` must be a valid GPIO pin.
  explicit Lcd(display::PanelType panel_type, const PanelConfig* panel_config,
               designware_dsi::DsiHostController* designware_dsi_host_controller,
               fidl::ClientEnd<fuchsia_hardware_gpio::Gpio> lcd_reset_gpio, bool enabled);

  Lcd(const Lcd&) = delete;
  Lcd& operator=(const Lcd&) = delete;

  // Turn the panel on
  zx::result<> Enable();

  // Turn the panel off
  zx::result<> Disable();

 private:
  // Decodes and performs the Amlogic-specific display initialization command
  // sequence stored in `encoded_commands` which is a packed buffer of all
  // encoded commands.
  //
  // The Amlogic-specific display initialization commands are defined in:
  // Amlogic MIPI DSI Panel Tuning User Guide, Version 0.1 (Google internal),
  // Section 3.2.6 "Init table config", page 19.
  zx::result<> PerformDisplayInitCommandSequence(std::span<const uint8_t> encoded_commands);

  display::PanelType panel_type_;
  const PanelConfig& panel_config_;

  designware_dsi::DsiHostController& designware_dsi_host_controller_;
  fidl::WireSyncClient<fuchsia_hardware_gpio::Gpio> lcd_reset_gpio_;

  bool enabled_ = false;
};

}  // namespace amlogic_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_LCD_H_
