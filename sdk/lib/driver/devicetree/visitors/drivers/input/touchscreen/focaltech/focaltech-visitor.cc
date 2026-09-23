// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "focaltech-visitor.h"

#include <fidl/fuchsia.hardware.input.focaltech/cpp/fidl.h>
#include <lib/ddk/metadata.h>
#include <lib/driver/devicetree/visitors/registration.h>
#include <lib/driver/logging/cpp/logger.h>

namespace focaltech_visitor_dt {

FocaltechVisitor::FocaltechVisitor()
    : DriverVisitor(
          {"focaltech,ft3x27", "focaltech,ft6336", "focaltech,ft5726", "focaltech,ft5336"}) {
  fdf_devicetree::Properties properties = {};
  properties.emplace_back(
      std::make_unique<fdf_devicetree::StringProperty>(kCompatible, /* required */ true));
  properties.emplace_back(
      std::make_unique<fdf_devicetree::BoolProperty>(kNeedsFirmware, /* required */ false));
  parser_ = std::make_unique<fdf_devicetree::PropertyParser>(std::move(properties));
}

zx::result<> FocaltechVisitor::DriverVisit(fdf_devicetree::Node& node,
                                           const devicetree::PropertyDecoder& decoder) {
  auto parser_output = parser_->Parse(node);
  if (parser_output.is_error()) {
    fdf::error("Focaltech visitor failed for node '{}' : {}", node.name(), parser_output);

    return parser_output.take_error();
  }

  fuchsia_hardware_input_focaltech::Metadata device_info;
  auto compatible = parser_output->Get<std::string>(kCompatible);
  if (*compatible == "focaltech,ft3x27") {
    device_info.device_id() = fuchsia_hardware_input_focaltech::DeviceId::kFt3X27;
  } else if (*compatible == "focaltech,ft6336") {
    device_info.device_id() = fuchsia_hardware_input_focaltech::DeviceId::kFt6336;
  } else if (*compatible == "focaltech,ft5726") {
    device_info.device_id() = fuchsia_hardware_input_focaltech::DeviceId::kFt5726;
  } else if (*compatible == "focaltech,ft5336") {
    device_info.device_id() = fuchsia_hardware_input_focaltech::DeviceId::kFt5336;
  } else {
    fdf::info("Unsupported device type '{}' in node '{}'. Not adding focaltech metadata.",
              *compatible, node.name());

    return zx::ok();
  }

  device_info.needs_firmware() = parser_output->Get<bool>(kNeedsFirmware);

  fit::result persisted_metadata = fidl::Persist(device_info);
  if (persisted_metadata.is_error()) {
    fdf::error("Failed to persist focaltech metadata: {}",
               persisted_metadata.error_value().FormatDescription());

    return zx::error(persisted_metadata.error_value().status());
  }

  fuchsia_hardware_platform_bus::Metadata focaltech_metadata(
      {{.id = fuchsia_hardware_input_focaltech::Metadata::kSerializableName,
        .data = std::move(persisted_metadata.value())}});

  node.AddMetadata(std::move(focaltech_metadata));

  fdf::debug("Added focaltech metadata to node '{}'", node.name());

  return zx::ok();
}

}  // namespace focaltech_visitor_dt

REGISTER_DEVICETREE_VISITOR(focaltech_visitor_dt::FocaltechVisitor);
