// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "thermal-zones-visitor.h"

#include <lib/driver/devicetree/visitors/registration.h>
#include <lib/driver/logging/cpp/logger.h>

namespace thermal_zones_visitor_dt {

ThermalZonesVisitor::ThermalZonesVisitor() {
  fdf_devicetree::Properties thermal_sensor_properties = {};
  thermal_sensor_properties.emplace_back(std::make_unique<fdf_devicetree::ReferenceProperty>(
      kThermalSensorReference, kThermalSensorCells, /* required */ false));
  thermal_sensor_parser_ =
      std::make_unique<fdf_devicetree::PropertyParser>(std::move(thermal_sensor_properties));

  fdf_devicetree::Properties trips_properties = {};
  trips_properties.emplace_back(
      std::make_unique<fdf_devicetree::Uint32Property>(kTemperature, true));
  trips_properties.emplace_back(std::make_unique<fdf_devicetree::StringProperty>(kType, true));
  trips_parser_ = std::make_unique<fdf_devicetree::PropertyParser>(std::move(trips_properties));
}

ThermalZonesVisitor::ThermalSensor& ThermalZonesVisitor::GetSensor(
    fdf_devicetree::Phandle phandle) {
  const auto [sensor_iter, success] = thermal_sensors_.insert({phandle, ThermalSensor()});
  return sensor_iter->second;
}

bool ThermalZonesVisitor::is_match(fdf_devicetree::Node& node) {
  return node.parent() && (node.parent().name() == "thermal-zones") &&
         (node.name().find("thermal") != std::string::npos);
}

zx::result<> ThermalZonesVisitor::Visit(fdf_devicetree::Node& node,
                                        const devicetree::PropertyDecoder& decoder) {
  if (!is_match(node)) {
    return zx::ok();
  }

  auto parser_output = thermal_sensor_parser_->Parse(node);
  if (parser_output.is_error()) {
    fdf::error("Thermal zones visitor parse failed for node '{}' : {}", node.name(), parser_output);

    return parser_output.take_error();
  }

  auto thermal_sensors = parser_output->Get<fdf_devicetree::References>(kThermalSensorReference);
  if (!thermal_sensors) {
    return zx::ok();
  }
  if (thermal_sensors->size() != 1) {
    fdf::error("Thermal sensor reference in node '{}' can only reference 1 sensor, actual: {}.",
               node.name(), thermal_sensors->size());

    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  for (auto& child : node.children()) {
    if (child.name() == "trips") {
      zx::result result = ParseTrips(*child.GetNode(), thermal_sensors->at(0).reference_node());
      if (result.is_error()) {
        return result.take_error();
      }
    }
  }

  return zx::ok();
}

zx::result<> ThermalZonesVisitor::ParseTrips(fdf_devicetree::Node& node,
                                             fdf_devicetree::ReferenceNode reference) {
  auto& sensor = GetSensor(*reference.phandle());
  sensor.trip_metadata = fuchsia_hardware_trippoint::TripDeviceMetadata();
  for (auto& child : node.children()) {
    zx::result trips_output = trips_parser_->Parse(*child.GetNode());
    if (trips_output.is_error()) {
      fdf::error("Failed to parse trips for node '{}' : {}", node.name(), trips_output);

      return trips_output.take_error();
    }

    if (trips_output->Get<std::string>(kType) == "critical") {
      sensor.trip_metadata->critical_temp_celsius() =
          static_cast<float>(*trips_output->Get<uint32_t>(kTemperature)) / 1000.0f;
      fdf::debug("Set critical temperature to '{:.2f}' for sensor '{}'",
                 sensor.trip_metadata->critical_temp_celsius(), reference.name());
    }
  }
  return zx::ok();
}

zx::result<> ThermalZonesVisitor::FinalizeNode(fdf_devicetree::Node& node) {
  if (!node.phandle() || thermal_sensors_.find(*node.phandle()) == thermal_sensors_.end()) {
    // Not a thermal sensor node.
    return zx::ok();
  }

  auto sensor = thermal_sensors_.find(*node.phandle());
  ZX_ASSERT_MSG(sensor != thermal_sensors_.end(), "Thermal sensor should be found in the list.");

  if (sensor->second.trip_metadata) {
    auto persisted_metadata = fidl::Persist(*sensor->second.trip_metadata);
    if (persisted_metadata.is_error()) {
      fdf::error("Failed to persist Trip Point Metadata: {}",
                 persisted_metadata.error_value().status());

      return zx::error(persisted_metadata.error_value().status());
    }

    fuchsia_hardware_platform_bus::Metadata metadata{{
        .id = fuchsia_hardware_trippoint::TripDeviceMetadata::kSerializableName,
        .data = std::move(persisted_metadata.value()),
    }};

    node.AddMetadata(std::move(metadata));
    fdf::debug("Trip point metadata added to node '{}'", node.name());
  }
  return zx::ok();
}

}  // namespace thermal_zones_visitor_dt

REGISTER_DEVICETREE_VISITOR(thermal_zones_visitor_dt::ThermalZonesVisitor);
