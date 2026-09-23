// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_SERIAL_SERIAL_PORT_SERIAL_PORT_VISITOR_H_
#define LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_SERIAL_SERIAL_PORT_SERIAL_PORT_VISITOR_H_

#include <lib/driver/devicetree/manager/visitor.h>
#include <lib/driver/devicetree/visitors/property-parser.h>

namespace serial_port_visitor_dt {

class SerialPortVisitor : public fdf_devicetree::Visitor {
 public:
  static constexpr char kSerialport[] = "serial-port";
  static constexpr char kUarts[] = "uarts";
  static constexpr char kUartCells[] = "#uart-cells";
  static constexpr char kUartNames[] = "uart-names";

  SerialPortVisitor();
  zx::result<> Visit(fdf_devicetree::Node& node,
                     const devicetree::PropertyDecoder& decoder) override;

  zx::result<> FinalizeNode(fdf_devicetree::Node& node) override;

 private:
  struct UartController {
    uint32_t serial_class;
  };

  // Return an existing or a new instance of UartController.
  UartController& GetController(fdf_devicetree::NodeID node_id);

  zx::result<> ParseSerialPort(fdf_devicetree::Node& node,
                               fdf_devicetree::ParsedProperties& properties);

  zx::result<> ParseReferenceChild(fdf_devicetree::Node& node,
                                   fdf_devicetree::ParsedProperties& properties);

  zx::result<> AddChildNodeSpec(fdf_devicetree::Node& child, uint32_t serial_class,
                                std::optional<std::string> uart_name);

  std::unique_ptr<fdf_devicetree::PropertyParser> parser_;
  std::map<fdf_devicetree::NodeID, UartController> uart_controllers_;
};

}  // namespace serial_port_visitor_dt

#endif  // LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_SERIAL_SERIAL_PORT_SERIAL_PORT_VISITOR_H_
