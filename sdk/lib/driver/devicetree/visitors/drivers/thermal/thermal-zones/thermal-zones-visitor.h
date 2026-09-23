// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_THERMAL_THERMAL_ZONES_THERMAL_ZONES_VISITOR_H_
#define LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_THERMAL_THERMAL_ZONES_THERMAL_ZONES_VISITOR_H_

#include <fidl/fuchsia.hardware.trippoint/cpp/fidl.h>
#include <lib/driver/devicetree/manager/visitor.h>
#include <lib/driver/devicetree/visitors/property-parser.h>

#include <optional>

namespace thermal_zones_visitor_dt {

class ThermalZonesVisitor : public fdf_devicetree::Visitor {
 public:
  static constexpr char kThermalSensorReference[] = "thermal-sensors";
  static constexpr char kThermalSensorCells[] = "#thermal-sensor-cells";
  static constexpr char kThermalZones[] = "thermal-zones";
  static constexpr char kTrips[] = "trips";
  static constexpr char kTemperature[] = "temperature";
  static constexpr char kType[] = "type";

  ThermalZonesVisitor();
  zx::result<> Visit(fdf_devicetree::Node& node,
                     const devicetree::PropertyDecoder& decoder) override;

  zx::result<> FinalizeNode(fdf_devicetree::Node& node) override;

 private:
  struct ThermalSensor {
    std::optional<fuchsia_hardware_trippoint::TripDeviceMetadata> trip_metadata;
  };

  bool is_match(fdf_devicetree::Node& node);

  // Return an existing or a new instance of ThermalSensor.
  ThermalSensor& GetSensor(fdf_devicetree::Phandle phandle);

  zx::result<> ParseTrips(fdf_devicetree::Node& node, fdf_devicetree::ReferenceNode reference);

  std::unique_ptr<fdf_devicetree::PropertyParser> thermal_sensor_parser_;
  std::unique_ptr<fdf_devicetree::PropertyParser> trips_parser_;
  // Mapping of thermal sensor phandle to its info.
  std::map<fdf_devicetree::Phandle, ThermalSensor> thermal_sensors_;
};

}  // namespace thermal_zones_visitor_dt

#endif  // LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_THERMAL_THERMAL_ZONES_THERMAL_ZONES_VISITOR_H_
