// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "serial-port-visitor.h"

#include <fidl/fuchsia.hardware.serial/cpp/fidl.h>
#include <lib/ddk/metadata.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/devicetree/visitors/registration.h>
#include <lib/driver/logging/cpp/logger.h>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/serial/cpp/bind.h>

namespace serial_port_visitor_dt {

SerialPortVisitor::SerialPortVisitor() {
  fdf_devicetree::Properties properties = {};
  properties.emplace_back(
      std::make_unique<fdf_devicetree::Uint32ArrayProperty>(kSerialport, /* required */ false));
  properties.emplace_back(std::make_unique<fdf_devicetree::ReferenceProperty>(
      kUarts, kUartCells, /* required */ false));
  properties.emplace_back(
      std::make_unique<fdf_devicetree::StringListProperty>(kUartNames, /* required */ false));
  parser_ = std::make_unique<fdf_devicetree::PropertyParser>(std::move(properties));
}

zx::result<> SerialPortVisitor::Visit(fdf_devicetree::Node& node,
                                      const devicetree::PropertyDecoder& decoder) {
  auto parser_output = parser_->Parse(node);
  if (parser_output.is_error()) {
    fdf::error("Serial port visitor parse failed for node '{}' : {}", node.name(), parser_output);

    return parser_output.take_error();
  }

  zx::result<> result = ParseSerialPort(node, *parser_output);
  if (!result.is_ok()) {
    return result;
  }

  return zx::ok();
}

zx::result<> SerialPortVisitor::ParseSerialPort(fdf_devicetree::Node& node,
                                                fdf_devicetree::ParsedProperties& properties) {
  auto serial_port = properties.Get<std::vector<uint32_t>>(kSerialport);
  if (!serial_port) {
    return zx::ok();
  }

  if (serial_port->size() != 3) {
    fdf::error("Node '{}' has invalid serial port property size {}, expected 3.", node.name(),
               serial_port->size());

    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  auto& controller = GetController(node.id());
  controller.serial_class = (*serial_port)[0];

  fuchsia_hardware_serial::SerialPortInfo serial_port_info = {};
  serial_port_info.serial_class() = static_cast<fuchsia_hardware_serial::Class>((*serial_port)[0]);
  serial_port_info.serial_vid() = (*serial_port)[1];
  serial_port_info.serial_pid() = (*serial_port)[2];

  fit::result encoded = fidl::Persist(serial_port_info);
  if (encoded.is_error()) {
    fdf::error("Failed to encode serial metadata: {}", encoded.error_value().FormatDescription());

    return zx::error(encoded.error_value().status());
  }

  fdf::debug("Added serial port metadata (class={}, vid={}, pid={}) to node '{}'",
             static_cast<uint32_t>(serial_port_info.serial_class()), serial_port_info.serial_vid(),
             serial_port_info.serial_pid(), node.name());

  fuchsia_hardware_platform_bus::Metadata metadata = {{
      .id = fuchsia_hardware_serial::SerialPortInfo::kSerializableName,
      .data = *std::move(encoded),
  }};
  node.AddMetadata(metadata);

  return zx::ok();
}

SerialPortVisitor::UartController& SerialPortVisitor::GetController(
    fdf_devicetree::NodeID node_id) {
  if (!uart_controllers_.contains(node_id)) {
    uart_controllers_[node_id] = UartController();
  }
  return uart_controllers_[node_id];
}

zx::result<> SerialPortVisitor::ParseReferenceChild(fdf_devicetree::Node& node,
                                                    fdf_devicetree::ParsedProperties& properties) {
  auto uarts = properties.Get<fdf_devicetree::References>(kUarts);
  if (!uarts) {
    return zx::ok();
  }

  auto uart_names = properties.Get<std::vector<std::string>>(kUartNames);
  if (uart_names && uart_names->size() > uarts->size()) {
    fdf::error("Node '{}' has {} uart entries but has {} uart names.", node.name(), uarts->size(),
               uart_names->size());

    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  for (uint32_t index = 0; index < uarts->size(); index++) {
    auto& reference = (*uarts)[index];
    if (uart_controllers_.contains(reference.reference_node().id())) {
      auto& controller = uart_controllers_[reference.reference_node().id()];
      std::optional<std::string> name;
      if (uart_names && index < uart_names->size()) {
        name = (*uart_names)[index];
      }
      zx::result<> result = AddChildNodeSpec(node, controller.serial_class, name);
      if (!result.is_ok()) {
        return result;
      }
    } else {
      fdf::error("Node '{}' has a invalid uarts property.", node.name());
    }
  }

  return zx::ok();
}

zx::result<> SerialPortVisitor::AddChildNodeSpec(fdf_devicetree::Node& child, uint32_t serial_class,
                                                 std::optional<std::string> uart_name) {
  auto uart_node = fuchsia_driver_framework::ParentSpec2{{
      .bind_rules =
          {
              fdf::MakeAcceptBindRule(bind_fuchsia::SERIAL_CLASS, serial_class),
              fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.serial.Service"),
              // TODO(https://fxbug.dev/467370573): Temporary workaround for a composite issue.
              // Remove this once the composite issue is resolved.
              fdf::MakeRejectBindRule(bind_fuchsia_serial::NAME, "bt-passthrough-hci"),
          },
      .properties =
          {
              fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.serial.Service"),
          },
  }};

  if (uart_name) {
    uart_node.properties().push_back(fdf::MakeProperty2(bind_fuchsia_serial::NAME, *uart_name));
  }

  child.AddNodeSpec(uart_node);
  fdf::debug("Added uart node spec with class {} name '{}' to node '{}'", serial_class,
             uart_name.value_or(""), child.name());

  return zx::ok();
}

zx::result<> SerialPortVisitor::FinalizeNode(fdf_devicetree::Node& node) {
  zx::result parser_output = parser_->Parse(node);
  if (parser_output.is_error()) {
    fdf::error("Serial port visitor parse failed for node '{}' : {}", node.name(), parser_output);

    return parser_output.take_error();
  }

  zx::result<> result = ParseReferenceChild(node, *parser_output);
  if (!result.is_ok()) {
    return result;
  }

  return zx::ok();
}

}  // namespace serial_port_visitor_dt

REGISTER_DEVICETREE_VISITOR(serial_port_visitor_dt::SerialPortVisitor);
