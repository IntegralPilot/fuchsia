// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_I2C_CONTROLLERS_I2C_BUS_VISITOR_I2C_BUS_VISITOR_H_
#define LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_I2C_CONTROLLERS_I2C_BUS_VISITOR_I2C_BUS_VISITOR_H_

#include <fidl/fuchsia.hardware.i2c.businfo/cpp/fidl.h>
#include <lib/driver/devicetree/visitors/driver-visitor.h>
#include <lib/driver/devicetree/visitors/property-parser.h>

#include <cstdint>
#include <string_view>

namespace i2c_bus_dt {

class I2cBusVisitor : public fdf_devicetree::Visitor {
 public:
  zx::result<> FinalizeNode(fdf_devicetree::Node& node) override;

  zx::result<> Visit(fdf_devicetree::Node& node,
                     const devicetree::PropertyDecoder& decoder) override;

 private:
  struct I2cController {
    std::vector<fuchsia_hardware_i2c_businfo::I2CChannel> channels;
    uint32_t bus_id;
  };

  // Create new instance of I2cController, returns error if one already exists for the node_name.
  zx::result<> CreateController(std::string node_name);

  void AddChildNodeSpec(fdf_devicetree::ChildNode& child, uint32_t global_id,
                        std::string_view name);

  zx::result<> ParseChild(I2cController& controller, fdf_devicetree::Node& parent,
                          fdf_devicetree::ChildNode& child);

  bool is_match(fdf_devicetree::Node& node);

  // Mapping of devicetree node name to i2c controller struct.
  std::map<std::string, I2cController> i2c_controllers_;
  uint32_t bus_id_counter_ = 0;
  uint32_t channel_id_counter_ = 0;
};

}  // namespace i2c_bus_dt

#endif  // LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_I2C_CONTROLLERS_I2C_BUS_VISITOR_I2C_BUS_VISITOR_H_
