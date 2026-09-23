// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "display-panel-visitor.h"

#include <lib/ddk/metadata.h>
#include <lib/device-protocol/display-panel.h>
#include <lib/driver/devicetree/visitors/registration.h>
#include <lib/driver/logging/cpp/logger.h>

#include <regex>

namespace display_panel_visitor_dt {

DisplayPanelVisitor::DisplayPanelVisitor() {
  fdf_devicetree::Properties properties = {};
  properties.emplace_back(
      std::make_unique<fdf_devicetree::Uint32Property>(kPanelType, /* required */ false));
  parser_ = std::make_unique<fdf_devicetree::PropertyParser>(std::move(properties));
}

bool DisplayPanelVisitor::IsMatch(const std::string& node_name) {
  // Check that it contains "display" and optionally contains a unit address (eg:
  // display@ffaa0000).
  std::regex name_regex(".*display(@.*)?$");
  return std::regex_match(node_name, name_regex);
}

zx::result<> DisplayPanelVisitor::Visit(fdf_devicetree::Node& node,
                                        const devicetree::PropertyDecoder& decoder) {
  if (!IsMatch(node.name())) {
    return zx::ok();
  }

  zx::result parser_output = parser_->Parse(node);
  if (parser_output.is_error()) {
    fdf::error("Display panel visitor parse failed for node '{}' : {}", node.name(), parser_output);

    return parser_output.take_error();
  }

  auto panel_type_prop = parser_output->Get<uint32_t>(kPanelType);
  if (!panel_type_prop) {
    return zx::ok();
  }

  display::PanelType panel_type = static_cast<display::PanelType>(*panel_type_prop);

  fuchsia_hardware_platform_bus::Metadata display_panel_metadata{{
      .id = std::to_string(DEVICE_METADATA_DISPLAY_PANEL_TYPE),
      .data = std::vector<uint8_t>(
          reinterpret_cast<const uint8_t*>(&panel_type),
          reinterpret_cast<const uint8_t*>(&panel_type) + sizeof(display::PanelType)),
  }};

  node.AddMetadata(display_panel_metadata);

  fdf::debug("Display panel info - type({}) added to node '{}'", static_cast<uint32_t>(panel_type),
             node.name());

  return zx::ok();
}

}  // namespace display_panel_visitor_dt

REGISTER_DEVICETREE_VISITOR(display_panel_visitor_dt::DisplayPanelVisitor);
