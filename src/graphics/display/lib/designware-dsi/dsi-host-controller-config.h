// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_DESIGNWARE_DSI_DSI_HOST_CONTROLLER_CONFIG_H_
#define SRC_GRAPHICS_DISPLAY_LIB_DESIGNWARE_DSI_DSI_HOST_CONTROLLER_CONFIG_H_

#include "src/graphics/display/lib/designware-dsi/dphy-interface-config.h"
#include "src/graphics/display/lib/designware-dsi/dpi-interface-config.h"
#include "src/graphics/display/lib/designware-dsi/dsi-packet-handler-config.h"

namespace designware_dsi {

struct DsiHostControllerConfig {
  // Must be valid for valid instances.
  DphyInterfaceConfig dphy_interface_config;

  // Must be valid and compatible with the D-PHY lane byte transmission rate
  // specified in `dphy_interface_config`, for valid instances.
  DpiInterfaceConfig dpi_interface_config;

  // Must be valid and compatible with the `color_component_mapping` specified
  // in `dpi_interface_config`, for valid instances.
  DsiPacketHandlerConfig dsi_packet_handler_config;

  constexpr bool IsValid() const;
};

constexpr bool DsiHostControllerConfig::IsValid() const {
  if (!dphy_interface_config.IsValid()) {
    return false;
  }

  constexpr int kBitsPerByte = 8;
  const int64_t dphy_data_lane_bits_per_second =
      dphy_interface_config.high_speed_mode_data_lane_bits_per_second();
  const int64_t dphy_data_lane_bytes_per_second = dphy_data_lane_bits_per_second / kBitsPerByte;
  if (!dpi_interface_config.IsValid(dphy_data_lane_bytes_per_second)) {
    return false;
  }

  if (!dsi_packet_handler_config.IsValid(dpi_interface_config.color_component_mapping)) {
    return false;
  }

  return true;
}

}  // namespace designware_dsi

#endif  // SRC_GRAPHICS_DISPLAY_LIB_DESIGNWARE_DSI_DSI_HOST_CONTROLLER_CONFIG_H_
